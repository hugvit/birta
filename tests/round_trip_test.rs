//! One review, end to end: the browser's lines go through the review library the way the
//! server writes them (under the lock, after recording any write it hasn't seen), the
//! agent's Edit is a plain file write, and every agent step is the real `birta` binary.
//! No server runs. Every process has a cleared env, `HOME` in a temp folder, the gate on
//! and `TZ=UTC`.
//!
//! The last section checks the cold-agent eval harness without ever calling an agent.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use birta::rev::Rev;
use birta::review::anchor_from_req;
use birta::review::chain::Chain;
use birta::review::doc::Doc;
use birta::review::fold;
use birta::review::log::{AppendCtx, Log, approve_ops};
use birta::review::ops::{
    self, AnchorReq, At, BlockScope, By, ByKind, Line, Op, ReplyKind, To, Verdict, Via,
};
use birta::review::state::{AnchorStatus, AskedKind, Attr, ItemStatus, ReviewState, Stage};
use birta::review::sync;
use serde_json::Value;

const LOG: &str = "pr.md.birta.jsonl";

/// An agent-drafted PR description. `we just fail`, `Ran the integration suite` and the
/// `## Stacked on` section are what the human points at; `backof` is a typo nobody asks
/// about.
const DOC: &str = "\
# Retry S3 uploads

Uploads that fail with a 503 now retry up to five times.

## Why

Nightly exports fail about once a week, and we just fail the whole job.

## Changes

- Wrap `put_object` in a retry loop
- Log each retry at warn level
- Ran the integration suite against a flaky mock

## Stacked on

- #412 storage client refactor
- #415 config loader

## Testing

- Unit tests for the backof schedule
";

fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    u64::try_from(since.as_millis()).unwrap()
}

/// `HH:MM` of epoch ms in UTC.
fn hhmm(ms: u64) -> String {
    let secs = ms / 1000 % 86_400;
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

fn human() -> By {
    By {
        kind: ByKind::Human,
        name: Some("andres".to_string()),
        via: Some(Via::Browser),
    }
}

/// A temp folder holding `home/`, `pr.md` and its review file.
struct Trip {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    /// The last human note, which a hand-back covers.
    last_human: u32,
}

impl Trip {
    /// `pr.md` with a review file made by `--drafted-by claude`.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        fs::create_dir(base.join("home")).unwrap();
        fs::write(base.join("pr.md"), DOC).unwrap();
        let trip = Self {
            _tmp: tmp,
            base,
            last_human: 0,
        };
        assert!(
            trip.log()
                .create_if_missing("pr.md", DOC, Some("claude"))
                .unwrap()
        );
        trip
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn log(&self) -> Log {
        Log::open(&self.path(LOG)).git_exclude(false)
    }

    fn read(&self, rel: &str) -> Vec<u8> {
        fs::read(self.path(rel)).unwrap()
    }

    fn text(&self) -> String {
        fs::read_to_string(self.path("pr.md")).unwrap()
    }

    /// Writes `pr.md` as an editor would, dated `mtime_ms`.
    fn write_doc(&self, text: &str, mtime_ms: u64) {
        let path = self.path("pr.md");
        fs::write(&path, text).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_millis(mtime_ms))
            .unwrap();
    }

    /// A browser op: under the lock, the doc's unrecorded write lands first, then `build`'s
    /// ops, built from the file and the disk text as they are under that lock.
    fn browser(&mut self, build: impl FnOnce(&Chain, &Doc) -> Vec<Op>) -> Vec<u32> {
        let now = now_ms();
        let mut held = self.log().hold().unwrap();
        sync::sync_external(&mut held, &self.path("pr.md"), now);
        let chain = Chain::build(held.parsed());
        let ops = build(&chain, &Doc::new(&self.text()));
        let is_note = matches!(ops.last(), Some(Op::Comment { .. }));
        let ctx = AppendCtx {
            now_ms: now,
            by: human(),
            cid: None,
        };
        let lines = held.append_with(ctx, |_, _| Ok(ops)).unwrap();
        if is_note {
            self.last_human = *lines.last().unwrap();
        }
        lines
    }

    fn note(&mut self, at: impl FnOnce(&Doc) -> At, verdict: Option<Verdict>, body: &str) -> u32 {
        let body = (!body.is_empty()).then(|| body.to_string());
        self.browser(|_, doc| {
            vec![Op::Comment {
                at: at(doc),
                verdict,
                body,
                to: None,
                kind: None,
                turn: None,
            }]
        })[0]
    }

    /// `⌘⏎`, due at once.
    fn submit(&mut self) -> u32 {
        let upto = self.last_human;
        let lines = self.browser(|_, _| {
            vec![Op::Submit {
                upto,
                not_before: now_ms(),
                hold_ms: 0,
                to: Some(To::One("*".to_string())),
                batch: None,
            }]
        });
        lines[0]
    }

    /// Approves the text on disk, due at once.
    fn approve(&mut self) -> u32 {
        *self
            .browser(|chain, doc| approve_ops(chain, &doc.text, now_ms(), 0))
            .last()
            .unwrap()
    }

    /// `birta args…` with `stdin`: exit code, stdout bytes and stderr.
    fn run_bytes(&self, args: &[&str], stdin: &str) -> (i32, Vec<u8>, String) {
        let out = Command::cargo_bin("birta")
            .unwrap()
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path("home"))
            .env("TZ", "UTC")
            .env("BIRTA_REVIEW", "1")
            .args(args)
            .current_dir(&self.base)
            .write_stdin(stdin)
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        let code = out
            .status
            .code()
            .expect("birta was killed by the test timeout");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !stderr.contains("not available in this build"),
            "birta {args:?} is still the stub: {stderr}"
        );
        (code, out.stdout, stderr)
    }

    fn run(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        let (code, stdout, stderr) = self.run_bytes(args, stdin);
        (code, String::from_utf8(stdout).expect("UTF-8"), stderr)
    }

    fn review(&self) -> (i32, String, String) {
        self.run(&["review", "pr.md", "--as", "claude"], "")
    }

    fn respond(&self, heredoc: &str) -> (i32, String, String) {
        self.run(&["respond", "pr.md", "--as", "claude"], heredoc)
    }

    /// `birta show target`: exit code and the exact bytes printed.
    fn show(&self, target: &str) -> (i32, Vec<u8>) {
        let (code, stdout, stderr) = self.run_bytes(&["show", target], "");
        assert_eq!(code, 0, "show {target}: {stderr}");
        (code, stdout)
    }

    fn lines(&self) -> Vec<Line> {
        let parsed = ops::parse(self.read(LOG));
        assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
        parsed.lines
    }

    fn lines_after(&self, n: u32) -> Vec<Line> {
        self.lines().into_iter().filter(|l| l.n > n).collect()
    }

    fn ts(&self, n: u32) -> u64 {
        let line = self.lines().into_iter().find(|l| l.n == n).unwrap();
        line.ts.unwrap()
    }

    fn claims(&self) -> Vec<Line> {
        self.lines()
            .into_iter()
            .filter(|l| matches!(l.op, Op::Claim { .. }))
            .collect()
    }

    fn fold(&self) -> ReviewState {
        let parsed = ops::parse(self.read(LOG));
        let chain = Chain::build(&parsed);
        let doc = Doc::new(&self.text());
        let state = fold::fold(&parsed, &chain, &doc).state;
        assert_eq!((state.log.bad, state.log.unknown_ops), (0, 0));
        state
    }
}

/// A note on the one place `q` occurs, as the page's selection resolves it.
fn quote(q: &str) -> impl FnOnce(&Doc) -> At {
    let q = q.to_string();
    move |doc| {
        let req = AnchorReq::Quote {
            sp: None,
            q,
            pre: String::new(),
            suf: String::new(),
            off: None,
            tl: None,
        };
        anchor_from_req(&req, doc, &doc.rev).expect("the quote resolves")
    }
}

/// A verdict on the whole section under `heading`.
fn section(heading: &str) -> impl FnOnce(&Doc) -> At {
    let heading = heading.to_string();
    move |doc| {
        let lines: Vec<&str> = doc.text.lines().collect();
        let stop = doc
            .blocks
            .stops
            .iter()
            .find(|s| lines[s.lines.0 as usize - 1] == heading)
            .expect("the heading is a stop");
        let req = AnchorReq::Block {
            sp: stop.sp.clone(),
            scope: BlockScope::Section,
        };
        anchor_from_req(&req, doc, &doc.rev).expect("the section anchor builds")
    }
}

fn doc_note() -> impl FnOnce(&Doc) -> At {
    |_| At::Doc
}

fn replaced(text: &str, old: &str, new: &str) -> String {
    assert_eq!(text.matches(old).count(), 1, "{old:?} is in the text once");
    text.replacen(old, new, 1)
}

fn is_claude(line: &Line) -> bool {
    line.by.kind == ByKind::Agent
        && line.by.name.as_deref() == Some("claude")
        && line.by.via == Some(Via::Cli)
}

/// The rev `review` tells the agent to `show` on its `next:` line.
fn shown_rev(view: &str) -> String {
    let next = view
        .lines()
        .find(|l| l.starts_with("next:"))
        .unwrap_or_else(|| panic!("no next: line in {view:?}"));
    let at = next.find("pr.md@").expect("the hint names a rev") + "pr.md@".len();
    next[at..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect()
}

#[test]
fn a_full_round_from_notes_to_an_approve() {
    let mut trip = Trip::new();

    // 1. The human walks the doc: a doc note, a quote note, a section `no` and an ask,
    //    then hands them back.
    let doc = trip.note(
        doc_note(),
        None,
        "too long overall; say how the retries back off",
    );
    let note = trip.note(quote("we just fail"), None, "say why: flaky S3 503s");
    let no = trip.note(section("## Stacked on"), Some(Verdict::No), "");
    let ask = trip.note(
        quote("Ran the integration suite"),
        Some(Verdict::Ask),
        "is 5 retries enough?",
    );
    let submit = trip.submit();
    let state = trip.fold();
    assert_eq!(
        state.phrase.stage,
        Stage::Sent,
        "step 1: {:?}",
        state.phrase
    );
    for n in [doc, note, no, ask] {
        let item = state.items.iter().find(|i| i.n == n).unwrap();
        assert_eq!(item.status, ItemStatus::Sent, "step 1: #{n}");
        assert_eq!(item.sent_in, Some(submit), "step 1: #{n}");
    }

    // 2. A save outside birta lands before the pickup; `review` records it ahead of its
    //    claim, claims the round once, and prints the round with the respond template.
    let outside = format!("{DOC}\nCloses #408.\n");
    trip.write_doc(&outside, now_ms());
    let before = trip.lines().last().unwrap().n;
    let (code, view, stderr) = trip.review();
    assert_eq!(code, 0, "step 2: {view}{stderr}");
    let claims = trip.claims();
    assert_eq!(claims.len(), 1, "step 2: one claim: {claims:#?}");
    let claim = claims[0].n;
    assert!(is_claude(&claims[0]), "step 2: {:?}", claims[0]);
    assert_eq!(
        claims[0].op,
        Op::Claim {
            round: submit,
            target: Some("pr.md".to_string())
        },
        "step 2"
    );
    let new = trip.lines_after(before);
    assert!(
        matches!(new[0].op, Op::External { .. }) && new[0].n < claim,
        "step 2: the outside save is recorded before the claim: {new:#?}"
    );
    let lines: Vec<&str> = view.lines().collect();
    assert_eq!(
        lines[0],
        format!(
            "pr.md · handed back {} by andres · 4 notes · drafted by claude",
            hhmm(trip.ts(submit))
        ),
        "step 2: {view}"
    );
    for want in [
        format!("#{note} done"),
        format!("#{note} covers \"<words you wrote>\""),
        format!("#{no} done"),
        format!("#{no} covers \"<words you wrote>\""),
        format!("#{ask} answer: <your answer>"),
        format!("#{doc} done: <one line: what you changed for this whole-doc note>"),
        format!("#{doc} covers \"<words you wrote>\""),
    ] {
        assert!(
            lines.contains(&want.as_str()),
            "step 2: {want:?} in\n{view}"
        );
    }
    let stable = view.replace(&hhmm(trip.ts(submit)), "HH:MM");
    insta::assert_snapshot!("round_one_view", stable);

    // 3. The agent's Edit, dated before its claim: nothing reaches the log yet.
    let log_before_edit = trip.read(LOG);
    let text = replaced(
        &outside,
        "five times.\n",
        "five times.\n\nRetries back off exponentially, with jitter.\n",
    );
    let text = replaced(
        &text,
        "a week, and we just fail",
        "a week on flaky S3 503s, and we just fail",
    );
    let text = replaced(
        &text,
        "## Stacked on\n\n- #412 storage client refactor\n- #415 config loader\n\n",
        "",
    );
    let edited = replaced(&text, "backof schedule", "backoff schedule");
    trip.write_doc(&edited, trip.ts(claim) - 60_000);
    assert_eq!(
        trip.read(LOG),
        log_before_edit,
        "step 3: no server, no line"
    );

    // 4. `respond`: the write is recorded first, the outcomes follow in heredoc order,
    //    and the yield ends the file.
    let heredoc = format!(
        "#{note} done: named the cause
#{no} done
#{ask} answer: yes; the 503s clear within two retries
#{doc} covers \"Retries back off exponentially, with jitter\"
unasked \"Unit tests for the backoff schedule\": fixed a typo
ask \"Log each retry at warn level\": should this be info?
@: said why the job fails and how retries back off; dropped the stacked-on list
"
    );
    let (code, stdout, stderr) = trip.respond(&heredoc);
    assert_eq!(code, 0, "step 4: {stdout}{stderr}");
    assert!(!stderr.contains("not handled"), "step 4: {stderr}");
    let new = trip.lines_after(claim);
    let ops: Vec<&Op> = new.iter().map(|l| &l.op).collect();
    assert_eq!(new.len(), 10, "step 4: external, 8 lines, yield: {ops:#?}");
    let Op::External {
        to_rev: ref recorded,
        ..
    } = new[0].op
    else {
        panic!("step 4: the agent's write is recorded first: {ops:#?}");
    };
    let external = new[0].n;
    assert_eq!(new[0].by.kind, ByKind::Birta, "step 4");
    assert_eq!(*recorded, Rev::of(edited.as_bytes()), "step 4");
    assert!(new[1..].iter().all(is_claude), "step 4: {new:#?}");
    let turn = claim;
    assert_eq!(
        ops[1..7],
        [
            &Op::Done {
                to: note,
                turn,
                late: false
            },
            &Op::Reply {
                to: note,
                verdict: None,
                body: Some("named the cause".to_string()),
                kind: Some(ReplyKind::Note),
                late: false,
            },
            &Op::Done {
                to: no,
                turn,
                late: false
            },
            &Op::Reply {
                to: ask,
                verdict: None,
                body: Some("yes; the 503s clear within two retries".to_string()),
                kind: Some(ReplyKind::Answer),
                late: false,
            },
            &Op::Covers {
                to: doc,
                quote: "Retries back off exponentially, with jitter".to_string(),
                turn,
                late: false,
            },
            &Op::Unasked {
                quote: "Unit tests for the backoff schedule".to_string(),
                body: Some("fixed a typo".to_string()),
                turn,
                late: false,
            },
        ],
        "step 4"
    );
    match ops[7] {
        Op::Comment {
            at: At::Anchor(anchor),
            verdict: Some(Verdict::Ask),
            body: Some(body),
            turn: Some(t),
            ..
        } => {
            assert!(
                anchor.quote.contains("Log each retry"),
                "step 4: {anchor:?}"
            );
            assert_eq!(
                (body.as_str(), *t),
                ("should this be info?", turn),
                "step 4"
            );
        }
        other => panic!("step 4: not an agent ask: {other:?}"),
    }
    assert_eq!(
        ops[8..],
        [
            &Op::TurnNote {
                body:
                    "said why the job fails and how retries back off; dropped the stacked-on list"
                        .to_string(),
                turn,
                late: false,
            },
            &Op::Yield {
                turn: Some(turn),
                release: false,
            },
        ],
        "step 4"
    );

    // 5. The fold: the outside save stays outside; claude's hunks are credited to its turn
    //    by position, whatever their mtime; solid for the asked, dotted for the unasked.
    let state = trip.fold();
    let (mine, others): (Vec<_>, Vec<_>) =
        state.changes.iter().partition(|c| c.turn == Some(claim));
    assert_eq!(
        others.len(),
        1,
        "step 5: only the outside save: {others:#?}"
    );
    assert_eq!(
        (others[0].attr, others[0].asked.kind),
        (Attr::Outside, AskedKind::Outside),
        "step 5"
    );
    assert!(
        mine.iter()
            .all(|c| c.attr == Attr::Agent && c.op == external),
        "step 5: {mine:#?}"
    );
    let mut asked: Vec<(String, Vec<u32>)> = mine
        .iter()
        .map(|c| (format!("{:?}", c.asked.kind), c.asked.notes.clone()))
        .collect();
    asked.sort();
    let mut want = vec![
        ("Cited".to_string(), vec![doc]),
        ("Location".to_string(), vec![note]),
        ("Location".to_string(), vec![no]),
        ("Unasked".to_string(), vec![]),
    ];
    want.sort();
    assert_eq!(asked, want, "step 5: {mine:#?}");
    let cited = mine
        .iter()
        .find(|c| c.asked.kind == AskedKind::Cited)
        .unwrap();
    let now = trip.text();
    assert_eq!(
        (
            cited.id.as_str(),
            &now[cited.range.bytes.0..cited.range.bytes.1],
        ),
        (
            format!("{external}.0").as_str(),
            "Retries back off exponentially, with jitter.\n",
        ),
        "step 5: covers binds the doc note to the hunk that wrote its quote"
    );
    for n in [doc, note, no, ask] {
        let item = state.items.iter().find(|i| i.n == n).unwrap();
        assert_eq!(item.status, ItemStatus::Handled, "step 5: #{n}: {item:#?}");
    }
    assert_eq!(state.phrase.stage, Stage::Replied, "step 5");
    let replied = state.phrase.replied.as_ref().expect("step 5: replied");
    assert_eq!(
        (
            replied.turn,
            replied.by.as_str(),
            replied.done,
            replied.answered,
            replied.didnt,
            replied.asked,
            replied.unasked,
            replied.not_handled,
        ),
        (claim, "claude", 2, 1, 0, 1, 1, 0),
        "step 5: {replied:#?}"
    );
    assert_eq!(
        (state.counts.not_handled, state.counts.open_asks),
        (0, 1),
        "step 5"
    );

    // 6. Round two: the agent leaves a note out, is warned, and settles it late, after its
    //    own yield, without opening its turn again.
    let cap = trip.note(quote("with jitter"), None, "name the cap");
    let rollout = trip.note(doc_note(), None, "add a rollout line");
    let second = trip.submit();
    let (code, view, stderr) = trip.review();
    assert_eq!(code, 0, "step 6: {view}{stderr}");
    let claims = trip.claims();
    assert_eq!(
        claims.len(),
        2,
        "step 6: one claim for round two: {claims:#?}"
    );
    let claim2 = claims[1].n;
    assert_eq!(
        claims[1].op,
        Op::Claim {
            round: second,
            target: Some("pr.md".to_string())
        },
        "step 6"
    );
    let item = |state: &ReviewState, n: u32| state.items.iter().find(|i| i.n == n).unwrap().status;
    for n in [doc, note, no, ask] {
        assert_eq!(
            item(&trip.fold(), n),
            ItemStatus::Handled,
            "step 6: #{n}, settled in round one, stays settled when round two is picked up"
        );
    }
    let listed: BTreeSet<&str> = view
        .lines()
        .skip_while(|l| !l.ends_with("<<'EOF'"))
        .take_while(|l| *l != "EOF")
        .filter(|l| l.starts_with('#'))
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from([format!("#{cap}").as_str(), format!("#{rollout}").as_str()]),
        "step 6: only round two's notes are asked about:\n{view}"
    );
    let capped = replaced(&trip.text(), "with jitter.", "with jitter, capped at 30 s.");
    trip.write_doc(&capped, now_ms());
    let (code, stdout, stderr) = trip.respond(&format!("#{cap} done\n@: capped the backoff\n"));
    assert_eq!(code, 0, "step 6: {stdout}{stderr}");
    assert!(
        stderr.contains(&format!("warning: not handled: #{rollout}")),
        "step 6: {stderr}"
    );
    let r2 = trip.lines_after(claim2);
    assert!(
        matches!(r2[0].op, Op::External { .. }),
        "step 6: the round-two write is recorded first: {r2:#?}"
    );
    assert!(matches!(r2.last().unwrap().op, Op::Yield { .. }), "step 6");
    let state = trip.fold();
    assert!(
        state
            .changes
            .iter()
            .any(|c| c.turn == Some(claim2) && c.attr == Attr::Agent && c.op == r2[0].n),
        "step 6: the round-two write is claude's: {:#?}",
        state.changes
    );
    assert_eq!(item(&state, rollout), ItemStatus::NotHandled, "step 6");
    assert_eq!(item(&state, cap), ItemStatus::Handled, "step 6");
    assert_eq!(state.counts.not_handled, 1, "step 6");
    let settled = state.items.iter().find(|i| i.n == doc).unwrap();
    assert_eq!(
        settled.turn,
        Some(claim),
        "step 6: round one keeps its note"
    );
    let replied = state.phrase.replied.as_ref().unwrap();
    assert_eq!(
        (
            replied.turn,
            replied.done,
            replied.didnt,
            replied.not_handled
        ),
        (claim2, 1, 0, 1),
        "step 6"
    );
    let yielded = trip.lines().last().unwrap().n;

    // A save after the yield is no longer the agent's: its turn is closed
    let noted = replaced(&trip.text(), "capped at 30 s.", "capped at 30 seconds.");
    trip.write_doc(&noted, now_ms());

    let (code, stdout, stderr) = trip.respond(&format!(
        "#{rollout} didnt: the rollout is in the release notes\n"
    ));
    assert_eq!(code, 0, "step 6: {stdout}{stderr}");
    assert!(stdout.contains("recorded late"), "step 6: {stdout}");
    let late = trip.lines_after(yielded);
    assert!(
        matches!(late[0].op, Op::External { .. }),
        "step 6: the save lands first: {late:#?}"
    );
    let outside_n = late[0].n;
    assert_eq!(
        late[1..].iter().map(|l| &l.op).collect::<Vec<_>>(),
        [&Op::Reply {
            to: rollout,
            verdict: None,
            body: Some("the rollout is in the release notes".to_string()),
            kind: Some(ReplyKind::Didnt),
            late: true,
        }],
        "step 6: one late line and no second yield"
    );
    let state = trip.fold();
    let saved: Vec<_> = state.changes.iter().filter(|c| c.op == outside_n).collect();
    assert!(
        !saved.is_empty()
            && saved
                .iter()
                .all(|c| (c.attr, c.turn) == (Attr::Outside, None)),
        "step 6: {saved:#?}"
    );
    assert_eq!(item(&state, rollout), ItemStatus::Handled, "step 6");
    assert_eq!(state.counts.not_handled, 0, "step 6");
    assert_eq!(state.phrase.stage, Stage::Replied, "step 6");
    let replied = state.phrase.replied.as_ref().unwrap();
    assert_eq!(
        (
            replied.turn,
            replied.done,
            replied.didnt,
            replied.not_handled
        ),
        (claim2, 1, 1, 0),
        "step 6: the late line counts and opens no turn"
    );

    // A late respond takes only its own rounds' notes
    let log = trip.read(LOG);
    let (code, stdout, stderr) = trip.respond(&format!("#{note} done\n"));
    assert_eq!(code, 2, "step 6: {stdout}{stderr}");
    assert_eq!(
        trip.read(LOG),
        log,
        "step 6: a refused respond writes nothing"
    );

    // 7. The approve: `review` says so and names the rev to `show`; the approved bytes
    //    stay readable after the doc moves on, and `review` then exits 3.
    let approved = trip.text();
    let approve = trip.approve();
    let (code, view, stderr) = trip.review();
    assert_eq!(code, 0, "step 7: {view}{stderr}");
    let short = Rev::of(approved.as_bytes()).to_string()[..8].to_string();
    assert!(
        view.lines()
            .any(|l| l == format!("APPROVED rev {short} (current ✓)")),
        "step 7: {view}"
    );
    let claims = trip.claims();
    assert_eq!(claims.len(), 3, "step 7: {claims:#?}");
    assert_eq!(
        claims[2].op,
        Op::Claim {
            round: approve,
            target: Some("pr.md".to_string())
        },
        "step 7: the approve is picked up"
    );
    let log = trip.read(LOG);
    let (code, stdout, stderr) = trip.respond(&format!("#{rollout} done\n"));
    assert_eq!(code, 4, "step 7: approved, unchanged: {stdout}{stderr}");
    assert_eq!(trip.read(LOG), log, "step 7: nothing to respond to");
    let hinted = shown_rev(&view);
    assert_eq!(hinted, short, "step 7: {view}");
    assert_eq!(
        trip.show(&format!("pr.md@{hinted}")).1,
        approved.as_bytes(),
        "step 7: show the hinted rev"
    );

    let after = format!("{approved}\nOne more line after the approve.\n");
    trip.write_doc(&after, now_ms());
    assert_eq!(
        trip.show("pr.md@approved").1,
        approved.as_bytes(),
        "step 7: @approved before review"
    );
    let (code, view, stderr) = trip.review();
    assert_eq!(code, 3, "step 7: {view}{stderr}");
    assert!(view.contains("CHANGED"), "step 7: {view}");
    assert_eq!(
        shown_rev(&view),
        short,
        "step 7: the hint keeps the approved rev"
    );
    assert_eq!(
        trip.show("pr.md@approved").1,
        approved.as_bytes(),
        "step 7: @approved after review"
    );
    assert_eq!(
        trip.show(&format!("pr.md@{hinted}")).1,
        approved.as_bytes(),
        "step 7: the hinted rev after review"
    );
    assert_eq!(
        trip.claims().len(),
        3,
        "step 7: no claim for a changed approve"
    );
    let before = trip.lines().last().unwrap().n;
    let (code, stdout, stderr) = trip.respond(&format!("#{rollout} done\n"));
    assert_eq!(code, 3, "step 7: approved, changed: {stdout}{stderr}");
    assert!(
        !trip.lines_after(before).iter().any(is_claude),
        "step 7: no agent line"
    );

    // Back to the approved bytes: current again, by rev
    trip.write_doc(&approved, now_ms());
    let (code, view, stderr) = trip.review();
    assert_eq!(code, 0, "step 7: {view}{stderr}");
    assert!(
        view.lines()
            .any(|l| l == format!("APPROVED rev {short} (current ✓)")),
        "step 7: {view}"
    );

    // A second approve is the one `@approved` names; the first rev still shows
    trip.write_doc(&after, now_ms());
    trip.approve();
    assert_eq!(
        trip.show("pr.md@approved").1,
        after.as_bytes(),
        "step 7: the latest approve"
    );
    assert_eq!(
        trip.show(&format!("pr.md@{short}")).1,
        approved.as_bytes(),
        "step 7: the first approved rev"
    );
    assert_eq!(trip.claims().len(), 3, "step 7");
}

// --- the cold-agent eval harness: checked without ever calling an agent ---

const FIXTURES: &[&str] = &["pr", "report", "questions"];

fn eval_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/agent-eval")
}

/// Whether `s` starts with a `YYYY-MM-DD` date.
fn starts_with_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 10
        && b[..10].iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// A temp folder to run `run.sh` in. The only `claude` it can reach is a fake that leaves
/// a marker, so a misbehaving script can never call the real one.
struct Eval {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    /// Written by any fake `claude` that runs.
    called: PathBuf,
    out_dir: PathBuf,
    tmpdir: PathBuf,
    path: String,
    script: PathBuf,
}

impl Eval {
    /// With a fake `claude` on `PATH`, or with none at all.
    fn new(fake_on_path: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (bin, home, tmpdir) = (base.join("bin"), base.join("home"), base.join("t"));
        for dir in [&bin, &home, &tmpdir] {
            fs::create_dir(dir).unwrap();
        }
        let eval = Self {
            _tmp: tmp,
            called: base.join("claude-was-called"),
            out_dir: base.join("eval"),
            path: format!("{}:/usr/bin:/bin", bin.display()),
            script: eval_dir().join("run.sh"),
            tmpdir,
            base,
        };
        if fake_on_path {
            eval.fake("bin/claude");
        }
        eval
    }

    /// A fake `claude` at `rel` that records its call and fails.
    fn fake(&self, rel: &str) -> PathBuf {
        let path = self.base.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$0\" >> '{}'\nexit 1\n",
                self.called.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// `program args…` in the harness env plus `env`, with `stdin` piped: exit code,
    /// stdout and stderr. `BIRTA_EVAL_OUT` is set unless `env` unsets it with "".
    fn exec(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: &str,
    ) -> (i32, String, String) {
        assert!(self.script.is_file(), "{:?} is missing", self.script);
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.base.join("home"))
            .env("TMPDIR", &self.tmpdir)
            .env("BIRTA_BIN", env!("CARGO_BIN_EXE_birta"))
            .env("BIRTA_EVAL_OUT", &self.out_dir);
        for (key, value) in env {
            if value.is_empty() && *key == "BIRTA_EVAL_OUT" {
                cmd.env_remove(key);
            } else {
                cmd.env(key, value);
            }
        }
        let out = cmd
            .current_dir(&self.base)
            .write_stdin(stdin)
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        let code = out
            .status
            .code()
            .expect("run.sh was killed by the test timeout");
        (
            code,
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
        self.run_with(args, env, "y\n")
    }

    fn run_with(&self, args: &[&str], env: &[(&str, &str)], stdin: &str) -> (i32, String, String) {
        let script = self.script.to_str().unwrap().to_string();
        let mut all = vec![script.as_str()];
        all.extend_from_slice(args);
        self.exec("bash", &all, env, stdin)
    }

    /// `run.sh args…` on a terminal of its own; what it wrote there comes back as stdout.
    fn run_on_a_terminal(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let line = format!("bash '{}' {}", self.script.display(), args.join(" "));
        let (_, out, _) = if cfg!(target_os = "macos") {
            self.exec(
                "script",
                &["-q", "/dev/null", "bash", "-c", &line],
                env,
                "run\n",
            )
        } else {
            self.exec(
                "script",
                &["-q", "-e", "-c", &line, "/dev/null"],
                env,
                "run\n",
            )
        };
        out
    }

    /// `run.sh args…` on a terminal of its own, with `typed` sent once the prompt is up
    /// and the terminal kept open until it exits. Returns what it wrote there.
    fn type_on_a_terminal(&self, args: &[&str], typed: &str) -> String {
        use std::io::{Read, Write};
        let line = format!("bash '{}' {}", self.script.display(), args.join(" "));
        let script: Vec<&str> = if cfg!(target_os = "macos") {
            vec!["-q", "/dev/null", "bash", "-c", &line]
        } else {
            vec!["-q", "-e", "-c", &line, "/dev/null"]
        };
        let mut child = std::process::Command::new("script")
            .args(&script)
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.base.join("home"))
            .env("TMPDIR", &self.tmpdir)
            .env("BIRTA_BIN", env!("CARGO_BIN_EXE_birta"))
            .env("BIRTA_EVAL_OUT", &self.out_dir)
            .current_dir(&self.base)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out = String::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                out.push_str(&String::from_utf8_lossy(&buf[..n]));
                let _ = tx.send(out.clone());
            }
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut seen = String::new();
        while !seen.contains("Type run to go on") {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(left) {
                Ok(out) => seen = out,
                Err(_) => {
                    let _ = child.kill();
                    panic!("no prompt:\n{seen}");
                }
            }
        }
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(typed.as_bytes()).unwrap();
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                panic!("run.sh was still running:\n{seen}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(stdin);
        while let Ok(out) = rx.recv_timeout(Duration::from_millis(200)) {
            seen = out;
        }
        seen
    }

    /// Nothing was called, written or left behind.
    fn untouched(&self, what: &str) {
        // A spawn left running in the background would write its marker late
        std::thread::sleep(Duration::from_millis(500));
        let called = fs::read_to_string(&self.called).unwrap_or_default();
        assert!(called.is_empty(), "{what} called claude: {called}");
        assert!(!self.out_dir.exists(), "{what} wrote results");
        assert_eq!(
            fs::read_dir(&self.tmpdir).unwrap().count(),
            0,
            "{what} left temp files"
        );
    }
}

#[test]
fn the_eval_dry_run_prints_the_plan_and_calls_nothing() {
    let eval = Eval::new(true);
    let (code, stdout, stderr) = eval.run(&["--dry-run", "--runs", "3"], &[]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    eval.untouched("a dry run");
    let fake = eval.base.join("bin/claude");
    for part in [
        "claude -p",
        "take a look at my notes on",
        "in birta",
        "BIRTA_REVIEW=1",
        "runs: 3",
        "Bash(birta:*) Read Edit",
        env!("CARGO_BIN_EXE_birta"),
        fake.to_str().unwrap(),
    ] {
        assert!(stdout.contains(part), "{part:?} in the plan:\n{stdout}");
    }
    let prefix = format!("{}/", eval.out_dir.display());
    for fixture in FIXTURES {
        assert!(
            stdout.contains(&format!("fixtures/{fixture}.md")),
            "{fixture}.md in the plan:\n{stdout}"
        );
        let named = stdout.match_indices(&prefix).any(|(at, _)| {
            let name = &stdout[at + prefix.len()..];
            starts_with_date(name) && name[10..].starts_with(&format!("-{fixture}.md"))
        });
        assert!(
            named,
            "results for {fixture} go to <out>/<date>-{fixture}.md:\n{stdout}"
        );
    }
}

#[test]
fn a_dry_run_names_the_claude_bin_it_would_call() {
    let eval = Eval::new(true);
    let custom = eval.fake("my bin/claude-custom");
    let custom = custom.to_str().unwrap();
    let (code, stdout, stderr) = eval.run(&["--dry-run"], &[("CLAUDE_BIN", custom)]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    eval.untouched("a dry run");
    assert!(stdout.contains(custom), "CLAUDE_BIN beats PATH:\n{stdout}");
    assert!(
        !stdout.contains(eval.base.join("bin/claude").to_str().unwrap()),
        "{stdout}"
    );
}

#[test]
fn a_claude_bin_that_is_relative_or_not_executable_is_refused() {
    let eval = Eval::new(true);
    let plain = eval.base.join("not-exec");
    fs::write(&plain, "#!/bin/sh\nexit 1\n").unwrap();
    for bad in ["bin/claude", plain.to_str().unwrap()] {
        for args in [&["--dry-run"][..], &["--runs", "1"][..]] {
            let (code, stdout, stderr) = eval.run(args, &[("CLAUDE_BIN", bad)]);
            assert_eq!(code, 2, "CLAUDE_BIN={bad} {args:?}: {stdout}{stderr}");
            assert!(stderr.contains("CLAUDE_BIN"), "{stderr}");
        }
    }
    eval.untouched("a refused CLAUDE_BIN");
}

#[test]
fn with_no_claude_a_dry_run_says_so_and_a_run_exits_1() {
    let eval = Eval::new(false);
    let (code, stdout, stderr) = eval.run(&["--dry-run"], &[]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let said = format!("{stdout}{stderr}");
    assert!(
        said.contains("claude") && said.contains("not found"),
        "the dry run says no claude was found:\n{said}"
    );
    eval.untouched("a dry run");

    let (code, stdout, stderr) = eval.run(&[], &[]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    eval.untouched("a run with no claude");
}

#[test]
fn a_run_with_no_terminal_never_calls_claude_even_with_a_piped_yes() {
    let eval = Eval::new(true);
    let custom = eval.fake("cb/claude");
    let started = std::time::Instant::now();
    let (code, stdout, stderr) = eval.run_with(
        &["--runs", "1"],
        &[
            ("CLAUDE_BIN", custom.to_str().unwrap()),
            ("ans", "run"),
            ("REPLY", "run"),
        ],
        "y\n3\nyes\nrun\n",
    );
    assert_eq!(code, 1, "a piped answer is not a yes:\n{stdout}{stderr}");
    assert!(stderr.contains("terminal"), "it says why: {stderr}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it never waits on /dev/tty"
    );
    eval.untouched("a run with stdin piped");
}

#[test]
fn bad_arguments_exit_2_and_never_run() {
    let eval = Eval::new(true);
    for args in [
        &["--dryrun"][..],
        &["--dry-run=1"],
        &["-n"],
        &["--runs"],
        &["--runs", "0"],
        &["--runs", "abc"],
        &["--runs", "-1"],
        &["--runs", "11"],
        &["--dry-run", "--bogus"],
        &["pr"],
    ] {
        let (code, stdout, stderr) = eval.run(args, &[]);
        assert_eq!(code, 2, "{args:?}: {stdout}{stderr}");
    }
    eval.untouched("a bad argument");
}

#[test]
fn a_run_under_ci_or_an_agent_never_calls_claude_even_on_a_terminal() {
    // `script` merges the terminal's output and doesn't pass on the exit code everywhere
    for (key, value, reason) in [
        ("CI", "", "CI"),
        ("CI", "false", "CI"),
        ("CLAUDECODE", "1", "CLAUDECODE"),
        ("AI_AGENT", "claude-code_2-1-284_agent", "AI_AGENT"),
    ] {
        let eval = Eval::new(true);
        let out = eval.run_on_a_terminal(&["--runs", "1"], &[(key, value)]);
        assert!(out.contains(reason), "{key}={value:?} refuses: {out}");
        eval.untouched(&format!("a run with {key}={value:?}"));
    }
}

/// The positive control for every refusal above: the same harness, typed `run` on a
/// terminal, does reach the fake `claude`, so "nothing was called" can fail.
#[test]
fn a_typed_run_on_a_terminal_calls_the_fake_claude_once_per_fixture() {
    let eval = Eval::new(true);
    let fake = eval.base.join("bin/claude");
    let (code, stdout, stderr) = eval.run(&["--dry-run"], &[]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains(&format!("claude: {}", fake.display())),
        "the fake is the claude it resolves:\n{stdout}"
    );

    let out = eval.type_on_a_terminal(&["--runs", "1"], "run\n");
    let called = fs::read_to_string(&eval.called).unwrap_or_default();
    let calls: Vec<&str> = called.lines().collect();
    assert_eq!(calls.len(), FIXTURES.len(), "{out}\n{called}");
    assert!(
        calls.iter().all(|c| *c == fake.to_str().unwrap()),
        "{called}"
    );
}

#[test]
fn without_a_context_folder_the_default_out_is_refused_and_never_made() {
    let eval = Eval::new(true);
    let repo = eval.base.join("repo");
    let scripts = repo.join("scripts/agent-eval");
    fs::create_dir_all(scripts.join("fixtures")).unwrap();
    fs::copy(eval_dir().join("run.sh"), scripts.join("run.sh")).unwrap();
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", eval.base.join("home"))
        .status()
        .unwrap();
    assert!(git.success());
    let eval = Eval {
        script: scripts.join("run.sh"),
        ..eval
    };

    let (code, stdout, stderr) = eval.run(&["--dry-run"], &[("BIRTA_EVAL_OUT", "")]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains(&format!("{}/.context does not exist", repo.display())),
        "{stdout}"
    );
    let (code, stdout, stderr) = eval.run(&["--runs", "1"], &[("BIRTA_EVAL_OUT", "")]);
    assert_ne!(code, 0, "{stdout}{stderr}");
    assert!(!repo.join(".context").exists(), "run.sh made .context");
    eval.untouched("a run with no .context");
}

#[test]
fn results_of_the_same_day_are_appended_as_new_sections() {
    let eval = Eval::new(true);
    let file = eval.base.join("2026-09-30-pr.md");
    let append = |body: &str| {
        eval.exec(
            "bash",
            &[
                "-c",
                ". \"$1\" && append_results \"$2\" 'Agent eval: pr' \"$3\"",
                "_",
                eval.script.to_str().unwrap(),
                file.to_str().unwrap(),
                body,
            ],
            &[],
            "",
        )
    };
    let (code, stdout, stderr) = append("| 1 | first |");
    assert_eq!(
        (code, stdout.as_str()),
        (0, ""),
        "sourcing runs nothing: {stderr}"
    );
    assert_eq!(append("| 1 | second |").0, 0);
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# Agent eval: pr\n"), "{text}");
    assert_eq!(text.matches("# Agent eval: pr").count(), 1, "{text}");
    let runs: Vec<&str> = text.lines().filter(|l| l.starts_with("## Run ")).collect();
    assert_eq!(runs.len(), 2, "{text}");
    for run in runs {
        let t = run.trim_start_matches("## Run ").as_bytes();
        assert!(
            t.len() == 8
                && t.iter().enumerate().all(|(i, c)| match i {
                    2 | 5 => *c == b':',
                    _ => c.is_ascii_digit(),
                }),
            "{run:?}"
        );
    }
    let (first, second) = (text.find("first").unwrap(), text.find("second").unwrap());
    assert!(first < second, "{text}");

    // An existing file is only ever appended to
    fs::write(&file, "keep me\n").unwrap();
    assert_eq!(append("| 1 | third |").0, 0);
    let text = fs::read_to_string(&file).unwrap();
    assert!(
        text.starts_with("keep me\n") && text.contains("third"),
        "{text}"
    );
    eval.untouched("sourcing run.sh");
}

/// Each fixture is a doc, a hand-back ready to claim (`<name>.handback.jsonl`, never a
/// live `*.birta.jsonl`), and the hand-labelled outcome of every note it sends.
#[test]
fn every_eval_fixture_is_handed_back_and_labelled() {
    let dir = eval_dir().join("fixtures");
    for name in FIXTURES {
        let doc_path = dir.join(format!("{name}.md"));
        let bytes = fs::read(&doc_path).unwrap_or_else(|e| panic!("{doc_path:?}: {e}"));
        let text = String::from_utf8(bytes).unwrap();
        let handback = dir.join(format!("{name}.handback.jsonl"));
        let log = fs::read(&handback).unwrap_or_else(|e| panic!("{handback:?}: {e}"));
        assert!(
            !dir.join(format!("{name}.md.birta.jsonl")).exists(),
            "{name}: a live review file in the fixtures would be claimed by `birta review`"
        );

        let parsed = ops::parse(&log);
        assert!(!parsed.read_only, "{name}");
        assert!(parsed.bad.is_empty(), "{name}: {:?}", parsed.bad);
        assert!(parsed.unknown.is_empty(), "{name}: {:?}", parsed.unknown);
        let chain = Chain::build(&parsed);
        assert_eq!(
            chain.head(),
            Some(&Rev::of(text.as_bytes())),
            "{name}: the hand-back was written on this exact text"
        );
        assert!(
            parsed.lines.iter().all(|l| !matches!(
                l.op,
                Op::Claim { .. } | Op::Yield { .. } | Op::Approve { .. } | Op::External { .. }
            )),
            "{name}: nothing picked up, approved or edited yet"
        );
        let round = fold::scan(&parsed, now_ms())
            .claimable_round(now_ms())
            .unwrap_or_else(|| panic!("{name}: a round is due and unclaimed"));

        let doc = Doc::new(&text);
        let state = fold::fold(&parsed, &chain, &doc).state;
        let sent: BTreeSet<String> = state
            .items
            .iter()
            .filter(|i| i.sent_in == Some(round))
            .map(|i| format!("#{}", i.n))
            .collect();
        assert!(!sent.is_empty(), "{name}: the round sends notes");
        for item in &state.items {
            assert_eq!(item.sent_in, Some(round), "{name}: #{} is a draft", item.n);
            assert!(
                item.at.status.is_none_or(|s| s == AnchorStatus::Ok),
                "{name}: #{} lost its place: {:?}",
                item.n,
                item.at
            );
        }

        let expect_path = dir.join(format!("{name}.expect.json"));
        let expect: Value = serde_json::from_slice(
            &fs::read(&expect_path).unwrap_or_else(|e| panic!("{expect_path:?}: {e}")),
        )
        .unwrap();
        for key in ["not_handled", "unasked"] {
            assert!(expect[key].is_u64(), "{name}: {key} is a count: {expect}");
        }
        let labelled: BTreeSet<String> = expect["notes"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: notes is an object: {expect}"))
            .iter()
            .map(|(id, label)| {
                assert!(
                    label.as_str().is_some_and(|l| !l.is_empty()),
                    "{name}: {id} has a label"
                );
                id.clone()
            })
            .collect();
        assert_eq!(
            labelled, sent,
            "{name}: every sent note is labelled, and only those"
        );
    }
}
