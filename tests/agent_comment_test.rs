//! `birta comment`: an agent's note or question on a doc, placed by the one anchor
//! constructor on the text on disk. Every process runs with a cleared env, `HOME` in a
//! temp folder and the gate on.

use std::fs;
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use birta::rev::Rev;
use birta::review::chain::Chain;
use birta::review::doc::Doc;
use birta::review::fold;
use birta::review::log::Log;
use birta::review::ops::{self, At, ByKind, Line, Op, Scope, Verdict, Via};
use birta::review::state::ItemStatus;
use birta::review::sync::sync_external;
use serde_json::Value;

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, Scenario};

const LOG: &str = "pr.md.birta.jsonl";

/// A temp folder holding `home/`, `pr.md` and, when given, its review file.
struct Tmp {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Tmp {
    fn new(text: &str, log: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        fs::create_dir(base.join("home")).unwrap();
        fs::write(base.join("pr.md"), text).unwrap();
        if let Some(log) = log {
            fs::write(base.join(LOG), log).unwrap();
        }
        Self { _tmp: tmp, base }
    }

    /// `pr.md` with no review file.
    fn bare() -> Self {
        Self::new(DOC, None)
    }

    fn with(s: &Scenario) -> Self {
        Self::new(s.text(), Some(&s.log()))
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn log_bytes(&self) -> Option<Vec<u8>> {
        fs::read(self.path(LOG)).ok()
    }

    fn cmd(&self, envs: &[(&str, &str)]) -> Command {
        let mut cmd = Command::cargo_bin("birta").unwrap();
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path("home"))
            .env("TZ", "UTC")
            .env("BIRTA_REVIEW", "1")
            .envs(envs.iter().copied())
            .current_dir(&self.base)
            .timeout(Duration::from_secs(20));
        cmd
    }

    /// `birta comment pr.md args…` with `stdin`: exit code, stdout and stderr.
    fn comment_env(
        &self,
        args: &[&str],
        stdin: &str,
        envs: &[(&str, &str)],
    ) -> (i32, String, String) {
        let out = self
            .cmd(envs)
            .args(["comment", "pr.md"])
            .args(args)
            .write_stdin(stdin)
            .output()
            .unwrap();
        let code = out
            .status
            .code()
            .expect("birta was killed by the test timeout");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !stderr.contains("not available in this build"),
            "birta comment {args:?} is still the stub: {stderr}"
        );
        (code, stdout, stderr)
    }

    fn comment(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        self.comment_env(args, stdin, &[])
    }

    fn lines(&self) -> Vec<Line> {
        let parsed = ops::parse(self.log_bytes().expect("the review file exists"));
        assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
        assert!(!parsed.read_only, "the review file has a good header");
        parsed.lines
    }

    fn last(&self) -> Line {
        self.lines().pop().unwrap()
    }
}

fn is_claude(line: &Line) -> bool {
    line.by.kind == ByKind::Agent
        && line.by.name.as_deref() == Some("claude")
        && line.by.via == Some(Via::Cli)
}

/// The `at` of a comment line, as the kit's anchors are written.
fn at_value(line: &Line) -> Value {
    match &line.op {
        Op::Comment { at, .. } => serde_json::to_value(at).unwrap(),
        op => panic!("line {} is not a comment: {op:?}", line.n),
    }
}

/// Runs a comment that must be refused, and checks that nothing was written: the review
/// file is byte-identical, or still absent.
fn refused(tmp: &Tmp, args: &[&str], stdin: &str) -> (i32, String) {
    let before = tmp.log_bytes();
    let (code, stdout, stderr) = tmp.comment(&[args, &["--as", "claude"]].concat(), stdin);
    assert_ne!(code, 0, "{args:?} was accepted:\n{stdout}");
    assert_eq!(
        tmp.log_bytes(),
        before,
        "a refused comment wrote the review file"
    );
    assert_eq!(fs::read_to_string(tmp.path("pr.md")).unwrap(), DOC);
    (code, stderr)
}

// --- the review file ---

#[test]
fn a_comment_on_a_doc_with_no_log_creates_it_drafted_by_nobody() {
    let tmp = Tmp::bare();
    // An agent's shell, named every way: it still never becomes the drafter.
    let env = [("CLAUDECODE", "1"), ("AI_AGENT", "claude-code_2_agent")];
    let (code, stdout, stderr) =
        tmp.comment_env(&["--doc", "why so long?", "--as", "claude"], "", &env);
    assert_eq!(code, 0, "{stdout}{stderr}");

    let parsed = ops::parse(tmp.log_bytes().unwrap());
    let header = parsed.header.as_ref().expect("line 1 is the header");
    assert_eq!(header.birta, "review/1");
    assert_eq!(header.doc, "pr.md");
    let lines = &parsed.lines;
    assert_eq!(lines.len(), 3, "snap, owner, comment: {lines:#?}");
    assert_eq!(
        lines.iter().map(|l| l.n).collect::<Vec<_>>(),
        [2, 3, 4],
        "{lines:#?}"
    );
    assert_eq!(
        lines[0].op,
        Op::Snap {
            rev: Rev::of(DOC.as_bytes()),
            text: DOC.to_string(),
            checkpoint: false,
        }
    );
    assert_eq!(lines[1].op, Op::Owner { drafted_by: None });
    assert!(is_claude(&lines[2]), "{:?}", lines[2].by);
    assert!(
        matches!(
            &lines[2].op,
            Op::Comment { at: At::Doc, verdict: None, body: Some(b), to: None, .. } if b == "why so long?"
        ),
        "{:?}",
        lines[2].op
    );
}

#[test]
fn a_comment_never_touches_the_drafter_of_an_existing_log() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.note("human note");
    let tmp = Tmp::with(&s);
    let before = tmp.lines();
    let (code, stdout, stderr) = tmp.comment(&["--doc", "a thought", "--as", "pi"], "");
    assert_eq!(code, 0, "{stdout}{stderr}");
    let after = tmp.lines();
    assert_eq!(
        &after[..before.len()],
        &before[..],
        "earlier lines are kept"
    );
    assert_eq!(
        after.len(),
        before.len() + 1,
        "one comment and nothing else"
    );
    assert_eq!(after.last().unwrap().by.name.as_deref(), Some("pi"));
}

#[test]
fn the_review_file_is_excluded_from_git_unless_the_config_says_not() {
    for (config, excluded) in [
        ("[review]\nenabled = true\n", true),
        ("[review]\nenabled = true\ngit_exclude = false\n", false),
    ] {
        let tmp = Tmp::bare();
        let init = StdCommand::new("git")
            .args(["init", "-q"])
            .current_dir(&tmp.base)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", tmp.path("home"))
            .status()
            .unwrap();
        assert!(init.success());
        let dir = tmp.path("home/.config/birta");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), config).unwrap();
        let (code, stdout, stderr) = tmp.comment(&["--doc", "hi", "--as", "claude"], "");
        assert_eq!(code, 0, "{stdout}{stderr}");
        let exclude = fs::read_to_string(tmp.path(".git/info/exclude")).unwrap_or_default();
        assert_eq!(
            exclude.contains("*.birta.jsonl"),
            excluded,
            "{config:?}: {exclude:?}"
        );
    }
}

// --- anchors ---

#[test]
fn an_ask_on_a_quote_is_a_blank_the_human_sees_at_once() {
    let mut s = Scenario::new(DOC);
    s.note("human draft");
    let tmp = Tmp::with(&s);
    let (code, stdout, stderr) = tmp.comment(
        &[
            "--quote",
            "detail two",
            "--verdict",
            "ask",
            "should this stay?",
            "--as",
            "claude",
        ],
        "",
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    let line = tmp.last();
    assert!(is_claude(&line), "{:?}", line.by);
    assert_eq!(
        at_value(&line),
        s.quote("detail two"),
        "the one constructor"
    );
    assert!(matches!(
        &line.op,
        Op::Comment { verdict: Some(Verdict::Ask), body: Some(b), turn: None, .. } if b == "should this stay?"
    ));

    // Not a submit, and not held back like the human's drafts: the ask is open now
    let parsed = ops::parse(tmp.log_bytes().unwrap());
    let doc = Doc::new(DOC);
    let state = fold::fold(&parsed, &Chain::build(&parsed), &doc).state;
    let item = state.items.iter().find(|i| i.n == line.n).expect("listed");
    assert_eq!(item.status, ItemStatus::Open);
    assert_eq!(item.by.kind, ByKind::Agent);
}

#[test]
fn a_quote_is_found_by_its_rendered_text_and_stored_as_source() {
    let tmp = Tmp::bare();
    // A soft break renders as a space; the anchor keeps the source's newline
    let (code, stdout, stderr) = tmp.comment(
        &[
            "--quote",
            "here, with a second",
            "why two lines?",
            "--as",
            "claude",
        ],
        "",
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    let line = tmp.last();
    let Op::Comment {
        at: At::Anchor(anchor),
        ..
    } = &line.op
    else {
        panic!("a quote anchor: {:?}", line.op);
    };
    assert_eq!(anchor.quote, "here,\nwith a second");
    assert_eq!(anchor.lines, (3, 4));
    assert_eq!(anchor.scope, Scope::Quote);
    assert_eq!(anchor.rev, Rev::of(DOC.as_bytes()));
    assert_eq!(
        at_value(&line),
        Scenario::new(DOC).quote("here, with a second")
    );
}

#[test]
fn a_quote_not_in_the_doc_exits_65_and_writes_nothing() {
    for tmp in [Tmp::bare(), Tmp::with(&Scenario::new(DOC))] {
        let (code, _) = refused(&tmp, &["--quote", "nowhere in this doc", "hi"], "");
        assert_eq!(code, 65);
    }
}

#[test]
fn an_ambiguous_quote_exits_65_numbering_the_places() {
    for tmp in [Tmp::bare(), Tmp::with(&Scenario::new(DOC))] {
        // "detail" is on lines 13 and 14
        let (code, stderr) = refused(&tmp, &["--quote", "detail", "hi"], "");
        assert_eq!(code, 65, "{stderr}");
        for part in ["1)", "2)", "line 13", "line 14", "--line"] {
            assert!(stderr.contains(part), "{part:?} in {stderr:?}");
        }
    }
}

#[test]
fn an_ambiguous_quote_inside_one_block_still_exits_65() {
    // Both hits share a paragraph, where the constructor falls back to a block anchor
    let tmp = Tmp::new("retry once, then retry again.\n", None);
    let (code, stdout, stderr) = tmp.comment(&["--quote", "retry", "hi", "--as", "claude"], "");
    assert_eq!(code, 65, "{stdout}{stderr}");
    for part in ["1)", "2)", "line 1", "--line"] {
        assert!(stderr.contains(part), "{part:?} in {stderr:?}");
    }
    assert!(
        tmp.log_bytes().is_none(),
        "a refused comment made the review file"
    );
}

#[test]
fn more_than_five_places_lists_five_and_counts_the_rest() {
    let text = "retry\n\n".repeat(7);
    let tmp = Tmp::new(&text, None);
    let (code, stdout, stderr) = tmp.comment(&["--quote", "retry", "hi", "--as", "claude"], "");
    assert_eq!(code, 65, "{stdout}{stderr}");
    assert!(stderr.contains("5)"), "{stderr}");
    assert!(!stderr.contains("6)"), "only 5 listed: {stderr}");
    assert!(stderr.contains("and 2 more"), "{stderr}");
}

#[test]
fn a_quote_across_two_blocks_exits_65() {
    let (code, stderr) = refused(
        &Tmp::bare(),
        &["--quote", "detail one detail two", "hi"],
        "",
    );
    assert_eq!(code, 65, "{stderr}");
}

#[test]
fn a_quote_of_rendered_markup_or_punctuation_is_stored_as_its_source() {
    let text = "Use the `retry` flag with **backoff**.\n\nIt's \"quoted\" at R&amp;D.\n";
    for (q, source) in [
        ("Use the retry flag with", "Use the `retry` flag with"),
        // As the page shows it, then as the source has it
        ("It\u{2019}s \u{201C}quoted\u{201D}", "It's \"quoted\""),
        ("It's \"quoted\"", "It's \"quoted\""),
        ("at R&D", "at R&amp;D"),
    ] {
        let tmp = Tmp::new(text, None);
        let (code, stdout, stderr) = tmp.comment(&["--quote", q, "hi", "--as", "claude"], "");
        assert_eq!(code, 0, "{q:?}: {stdout}{stderr}");
        let line = tmp.last();
        let Op::Comment {
            at: At::Anchor(anchor),
            ..
        } = &line.op
        else {
            panic!("{q:?}: a quote anchor: {:?}", line.op);
        };
        assert_eq!(anchor.quote, source, "{q:?}");
        assert_eq!(anchor.scope, Scope::Quote, "{q:?}");
    }
}

#[test]
fn line_n_points_at_the_innermost_block_on_that_line() {
    let s = Scenario::new(DOC);
    // Line 4 is the paragraph's second line, 8 the nested item, 11 a heading
    for (line, stop) in [
        ("3", "Intro"),
        ("4", "Intro"),
        ("8", "nested item"),
        ("11", "## Details"),
    ] {
        let tmp = Tmp::bare();
        let (code, stdout, stderr) = tmp.comment(&["--line", line, "hi", "--as", "claude"], "");
        assert_eq!(code, 0, "--line {line}: {stdout}{stderr}");
        assert_eq!(at_value(&tmp.last()), s.block(stop), "--line {line}");
    }
}

#[test]
fn line_n_outside_the_doc_is_a_usage_error() {
    for line in ["0", "28", "999"] {
        let (code, stderr) = refused(&Tmp::bare(), &["--line", line, "hi"], "");
        assert_eq!(code, 2, "--line {line}: {stderr}");
    }
}

#[test]
fn line_n_on_a_line_with_no_block_exits_65_naming_the_nearest() {
    // 2 and 10 are blank; 16 and 17 are a table's header row and rule
    for (line, near) in [
        ("2", ["line 1", "line 3"]),
        ("10", ["line 9", "line 11"]),
        ("16", ["line 14", "line 18"]),
        ("17", ["line 14", "line 18"]),
    ] {
        for tmp in [Tmp::bare(), Tmp::with(&Scenario::new(DOC))] {
            let (code, stderr) = refused(&tmp, &["--line", line, "hi"], "");
            assert_eq!(code, 65, "--line {line}: {stderr}");
            assert!(
                stderr.contains(&format!("no block at line {line}")),
                "{stderr}"
            );
            for part in near {
                assert!(stderr.contains(part), "{part:?} in {stderr:?}");
            }
        }
    }
}

#[test]
fn line_n_with_a_quote_picks_the_occurrence_nearest_it() {
    // "detail" is on lines 13 and 14
    for (line, on) in [("13", 13), ("14", 14), ("1", 13), ("27", 14)] {
        let tmp = Tmp::bare();
        let (code, stdout, stderr) = tmp.comment(
            &["--quote", "detail", "--line", line, "hi", "--as", "claude"],
            "",
        );
        assert_eq!(code, 0, "--line {line}: {stdout}{stderr}");
        let last = tmp.last();
        let Op::Comment {
            at: At::Anchor(anchor),
            ..
        } = &last.op
        else {
            panic!("a quote anchor: {:?}", last.op);
        };
        assert_eq!(anchor.scope, Scope::Quote, "--line {line}");
        assert_eq!(anchor.quote, "detail", "--line {line}");
        assert_eq!(anchor.lines, (on, on), "--line {line}");
        assert_eq!(anchor.rev, Rev::of(DOC.as_bytes()));
    }
    let (code, stderr) = refused(
        &Tmp::bare(),
        &["--quote", "nowhere in this doc", "--line", "13", "hi"],
        "",
    );
    assert_eq!(code, 65, "{stderr}");
}

#[test]
fn a_line_hint_only_breaks_ties() {
    // A hint on a quote's second line names it; a hint far from a unique quote is ignored
    for (q, hint, want) in [
        ("here, with a second", "4", (3, 4)),
        ("detail two", "3", (14, 14)),
        ("detail two", "27", (14, 14)),
    ] {
        let tmp = Tmp::bare();
        let (code, stdout, stderr) =
            tmp.comment(&["--quote", q, "--line", hint, "hi", "--as", "claude"], "");
        assert_eq!(code, 0, "{q:?} --line {hint}: {stdout}{stderr}");
        let last = tmp.last();
        let Op::Comment {
            at: At::Anchor(anchor),
            ..
        } = &last.op
        else {
            panic!("a quote anchor: {:?}", last.op);
        };
        assert_eq!(anchor.lines, want, "{q:?} --line {hint}");
        assert_eq!(anchor.scope, Scope::Quote);
    }
}

#[test]
fn line_n_that_is_not_a_line_number_is_a_usage_error() {
    for line in ["-1", "abc", ""] {
        let (code, stderr) = refused(&Tmp::bare(), &["--line", line, "hi"], "");
        assert_eq!(code, 2, "--line {line:?}: {stderr}");
    }
}

#[test]
fn doc_conflicts_with_quote_and_line() {
    for args in [
        vec!["--doc", "--quote", "detail two", "hi"],
        vec!["--doc", "--line", "3", "hi"],
    ] {
        for tmp in [Tmp::bare(), Tmp::with(&Scenario::new(DOC))] {
            let (code, stderr) = refused(&tmp, &args, "");
            assert_eq!(code, 2, "{args:?}: {stderr}");
        }
    }
}

#[test]
fn an_anchor_flag_is_required() {
    for tmp in [Tmp::bare(), Tmp::with(&Scenario::new(DOC))] {
        let (code, stderr) = refused(&tmp, &["hi"], "");
        assert_eq!(code, 2, "{stderr}");
        assert!(
            stderr.contains("choose --quote, --line or --doc"),
            "{stderr}"
        );
    }
}

// --- replies ---

#[test]
fn to_names_the_comment_this_note_answers() {
    let mut s = Scenario::new(DOC);
    let note = s.note("human note");
    for to in [note.to_string(), format!("#{note}")] {
        let tmp = Tmp::with(&s);
        let (code, stdout, stderr) =
            tmp.comment(&["--doc", "--to", &to, "an answer", "--as", "claude"], "");
        assert_eq!(code, 0, "--to {to}: {stdout}{stderr}");
        assert!(
            matches!(&tmp.last().op, Op::Comment { to: Some(n), .. } if *n == note),
            "--to {to}: {:?}",
            tmp.last().op
        );
    }
}

#[test]
fn to_may_name_the_agents_own_earlier_comment() {
    let mut s = Scenario::new(DOC);
    let own = s.push(
        support::scenario::agent("claude"),
        serde_json::json!({"op": "comment", "at": "doc", "verdict": "ask", "body": "why?"}),
    );
    let tmp = Tmp::with(&s);
    let (code, stdout, stderr) = tmp.comment(
        &[
            "--doc",
            "--to",
            &own.to_string(),
            "and also",
            "--as",
            "claude",
        ],
        "",
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(matches!(&tmp.last().op, Op::Comment { to: Some(n), .. } if *n == own));
}

#[test]
fn to_must_name_a_live_comment_in_the_log() {
    let mut s = Scenario::new(DOC);
    let note = s.note("human note");
    let reply = s.human_reply(note, None, Some("more"));
    let gone = s.note("taken back");
    s.retract(gone);
    let submit = s.submit();
    // The header, the snap, the owner, a reply, a retracted note, the submit, past the end
    let lines = [1, 2, 3, reply, gone, submit, 99].map(|n| n.to_string());
    let words = ["0", "#", "claude", "-1", "abc"].map(str::to_string);
    for to in lines.iter().chain(&words) {
        let (code, stderr) = refused(&Tmp::with(&s), &["--doc", "--to", to, "hi"], "");
        assert_eq!(code, 2, "--to {to}: {stderr}");
    }
    let (code, stderr) = refused(&Tmp::bare(), &["--doc", "--to", "1", "hi"], "");
    assert_eq!(code, 2, "no log: {stderr}");
    let (code, stderr) = refused(&Tmp::bare(), &["--doc", "--to", "4", "hi"], "");
    assert_eq!(code, 2, "no log, so no comment 4: {stderr}");
}

// --- turns ---

/// The `turn` of the comment `name` leaves on a copy of `s`.
fn turn_of(s: &Scenario, name: &str) -> Option<u32> {
    let tmp = Tmp::with(s);
    let (code, stdout, stderr) = tmp.comment(&["--doc", "hi", "--as", name], "");
    assert_eq!(code, 0, "{stdout}{stderr}");
    match tmp.last().op {
        Op::Comment { turn, .. } => turn,
        op => panic!("not a comment: {op:?}"),
    }
}

#[test]
fn a_comment_inside_its_own_open_claim_carries_the_turn() {
    let mut s = Scenario::new(DOC);
    s.note("human note");
    s.submit();
    let claim = s.claim("claude");
    assert_eq!(turn_of(&s, "claude"), Some(claim));
    assert_eq!(turn_of(&s, "pi"), None, "someone else's claim");
    s.yield_turn("claude", claim);
    assert_eq!(turn_of(&s, "claude"), None, "after its yield");

    // A release closes the claim as a yield does
    let mut s = Scenario::new(DOC);
    s.note("human note");
    s.submit();
    let claim = s.claim("claude");
    s.release(claim);
    assert_eq!(turn_of(&s, "claude"), None, "after a release");

    // A claim on an approve opens no turn
    let mut s = Scenario::new(DOC);
    s.approve();
    s.claim("claude");
    assert_eq!(turn_of(&s, "claude"), None, "an approve's claim");
}

// --- verdicts and bodies ---

#[test]
fn an_agent_verdict_other_than_ask_is_refused() {
    for verdict in [
        "no",
        "yes",
        "later",
        "your_call",
        "rewrite",
        "stet",
        "maybe",
    ] {
        for tmp in [Tmp::bare(), Tmp::with(&Scenario::new(DOC))] {
            let (code, stderr) = refused(&tmp, &["--doc", "--verdict", verdict, "hi"], "");
            assert_eq!(code, 2, "--verdict {verdict}: {stderr}");
            assert!(stderr.contains("ask"), "names what is allowed: {stderr}");
        }
    }
}

#[test]
fn the_body_comes_from_stdin_when_no_argument_is_given() {
    let tmp = Tmp::bare();
    let (code, stdout, stderr) = tmp.comment(&["--doc", "--as", "claude"], "line one\nline two\n");
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        matches!(&tmp.last().op, Op::Comment { body: Some(b), .. } if b == "line one\nline two"),
        "{:?}",
        tmp.last().op
    );
}

#[test]
fn an_empty_note_is_refused() {
    for stdin in ["", "  \n\n"] {
        let (code, stderr) = refused(&Tmp::bare(), &["--doc"], stdin);
        assert_eq!(code, 2, "{stdin:?}: {stderr}");
    }
    let (code, stderr) = refused(&Tmp::bare(), &["--doc", ""], "");
    assert_eq!(code, 2, "{stderr}");
}

#[test]
fn a_body_argument_never_reads_stdin() {
    let tmp = Tmp::bare();
    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("birta"))
        .args(["comment", "pr.md", "--doc", "hello", "--as", "claude"])
        .current_dir(&tmp.base)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.path("home"))
        .env("BIRTA_REVIEW", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Held open, so a read of stdin blocks instead of seeing EOF.
    let _stdin = child.stdin.take();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("comment with a body argument read stdin");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    assert!(matches!(&tmp.last().op, Op::Comment { body: Some(b), .. } if b == "hello"));
}

#[test]
fn a_verdict_is_matched_exactly() {
    for verdict in ["ASK", "Ask", " ask", "ask "] {
        let (code, stderr) = refused(&Tmp::bare(), &["--doc", "--verdict", verdict, "hi"], "");
        assert_eq!(code, 2, "--verdict {verdict:?}: {stderr}");
    }
}

/// The body of the comment a stdin of `stdin` leaves.
fn body_from(stdin: &[u8]) -> String {
    let tmp = Tmp::bare();
    let out = tmp
        .cmd(&[])
        .args(["comment", "pr.md", "--doc", "--as", "claude"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("not available in this build"), "{stderr}");
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    match tmp.last().op {
        Op::Comment { body: Some(b), .. } => b,
        op => panic!("a comment with a body: {op:?}"),
    }
}

#[test]
fn a_crlf_body_is_read_as_lf() {
    assert_eq!(body_from(b"line one\r\nline two\r\n"), "line one\nline two");
}

#[test]
fn control_characters_other_than_newline_and_tab_are_dropped() {
    assert_eq!(
        body_from(b"a\x1b[31mb\x00c\td\x07\ne\x7f\n"),
        "a[31mbc\td\ne\x7f"
    );
}

#[test]
fn a_body_over_64_kib_is_refused() {
    const CAP: usize = 64 * 1024;
    let at_cap = "x".repeat(CAP);
    assert_eq!(body_from(at_cap.as_bytes()).len(), CAP);
    // Dropped control characters don't count
    let padded = format!("{}{}", "\x01".repeat(100), at_cap);
    assert_eq!(body_from(padded.as_bytes()).len(), CAP);

    let over = "x".repeat(CAP + 1);
    let (code, stderr) = refused(&Tmp::bare(), &["--doc"], &over);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("body too long"), "{stderr}");
}

// --- the doc as it is on disk ---

#[test]
fn an_unrecorded_doc_edit_lands_before_the_comment() {
    let mut s = Scenario::new(DOC);
    s.note("human draft");
    let tmp = Tmp::with(&s);
    let before = tmp.lines().len();
    let edited = format!("{DOC}\nRetries now use jitter.\n");
    fs::write(tmp.path("pr.md"), &edited).unwrap();
    // No --as: the agent is named by its shell's env
    let (code, stdout, stderr) = tmp.comment_env(
        &[
            "--quote",
            "Retries now use jitter",
            "--verdict",
            "ask",
            "why?",
        ],
        "",
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    let new = &tmp.lines()[before..];
    assert_eq!(new.len(), 2, "external, then comment: {new:#?}");
    assert_eq!(new[0].by.kind, ByKind::Birta);
    let new_rev = Rev::of(edited.as_bytes());
    assert!(
        matches!(&new[0].op, Op::External { rev, to_rev, .. }
            if *rev == Rev::of(DOC.as_bytes()) && *to_rev == new_rev),
        "{:?}",
        new[0].op
    );
    assert!(is_claude(&new[1]), "{:?}", new[1].by);
    let Op::Comment {
        at: At::Anchor(anchor),
        ..
    } = &new[1].op
    else {
        panic!("a quote anchor: {:?}", new[1].op);
    };
    assert_eq!(anchor.rev, new_rev, "anchored on the text on disk");
    assert_eq!(anchor.lines, (29, 29));
}

#[test]
fn a_missing_doc_exits_1_and_creates_nothing() {
    let tmp = Tmp::bare();
    let out = tmp
        .cmd(&[])
        .args(["comment", "gone.md", "--doc", "hi", "--as", "claude"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("not available in this build"), "{stderr}");
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(!tmp.path("gone.md.birta.jsonl").exists());
    assert!(!tmp.path("gone.md").exists());
}

#[test]
fn a_doc_that_is_not_utf8_gets_no_review_file() {
    let tmp = Tmp::bare();
    fs::write(tmp.path("pr.md"), b"# caf\xe9\n").unwrap();
    let (code, stdout, stderr) = tmp.comment(&["--doc", "hi", "--as", "claude"], "");
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(tmp.log_bytes().is_none(), "a lossy snap was written");
}

#[test]
fn a_directory_or_stdin_is_not_a_file_to_comment_on() {
    let tmp = Tmp::bare();
    fs::create_dir(tmp.path("sub")).unwrap();
    for target in ["sub", "-"] {
        let out = tmp
            .cmd(&[])
            .args(["comment", target, "--doc", "hi", "--as", "claude"])
            .write_stdin("")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("not available in this build"), "{stderr}");
        assert_eq!(out.status.code(), Some(2), "{target}: {stderr}");
        assert!(stderr.contains("comment needs a file"), "{stderr}");
    }
    assert!(!tmp.path("sub.birta.jsonl").exists());
    assert!(!tmp.path("sub/.birta.jsonl").exists());
    assert!(!tmp.path("-.birta.jsonl").exists());
}

// --- the review file, under other writers ---

#[test]
fn concurrent_comments_on_a_new_doc_create_one_log_and_all_land() {
    let tmp = Tmp::bare();
    let children: Vec<_> = (0..8)
        .map(|i| {
            StdCommand::new(assert_cmd::cargo::cargo_bin("birta"))
                .args([
                    "comment",
                    "pr.md",
                    "--doc",
                    &format!("c{i}"),
                    "--as",
                    "claude",
                ])
                .current_dir(&tmp.base)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", tmp.path("home"))
                .env("BIRTA_REVIEW", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let lines = tmp.lines();
    assert_eq!(
        lines.iter().map(|l| l.n).collect::<Vec<_>>(),
        (2..=11).collect::<Vec<_>>()
    );
    let count = |f: fn(&Op) -> bool| lines.iter().filter(|l| f(&l.op)).count();
    assert_eq!(count(|op| matches!(op, Op::Snap { .. })), 1);
    assert_eq!(count(|op| matches!(op, Op::Owner { .. })), 1);
    let mut bodies: Vec<_> = lines
        .iter()
        .filter_map(|l| match &l.op {
            Op::Comment { body: Some(b), .. } => Some(b.clone()),
            _ => None,
        })
        .collect();
    bodies.sort();
    assert_eq!(bodies, (0..8).map(|i| format!("c{i}")).collect::<Vec<_>>());
}

#[test]
fn an_empty_review_file_is_replaced_and_a_newer_one_is_left_alone() {
    let tmp = Tmp::new(DOC, Some(""));
    let (code, stdout, stderr) = tmp.comment(&["--doc", "hi", "--as", "claude"], "");
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        tmp.lines().iter().map(|l| l.n).collect::<Vec<_>>(),
        [2, 3, 4]
    );

    // A whole header: a lone line missing `about` is garbled, and garbled is replaced
    let newer = "{\"birta\":\"review/2\",\"doc\":\"pr.md\",\"about\":\"later\"}\n";
    let (code, _) = refused(&Tmp::new(DOC, Some(newer)), &["--doc", "hi"], "");
    assert_ne!(code, 0);
}

#[cfg(unix)]
#[test]
fn a_symlinked_doc_keeps_its_review_file_beside_the_target() {
    let tmp = Tmp::bare();
    fs::create_dir(tmp.path("real")).unwrap();
    fs::rename(tmp.path("pr.md"), tmp.path("real/pr.md")).unwrap();
    std::os::unix::fs::symlink("real/pr.md", tmp.path("link.md")).unwrap();
    let out = tmp
        .cmd(&[])
        .args(["comment", "link.md", "--doc", "hi", "--as", "claude"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("not available in this build"), "{stderr}");
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(tmp.path("real/pr.md.birta.jsonl").exists());
    assert!(!tmp.path("link.md.birta.jsonl").exists());
}

#[test]
fn a_comment_with_no_name_is_still_an_agents() {
    let tmp = Tmp::bare();
    let (code, stdout, stderr) = tmp.comment(&["--doc", "hi"], "");
    assert_eq!(code, 0, "{stdout}{stderr}");
    let by = tmp.last().by;
    assert_eq!(by.kind, ByKind::Agent);
    assert_eq!(by.via, Some(Via::Cli));
}

#[test]
fn a_line_hint_places_rendered_markup_in_the_paragraph_it_picks() {
    let para = "It's \"quoted\" at R&amp;D.\n";
    let text = format!("{para}\n{para}");
    for q in ["It\u{2019}s \u{201C}quoted\u{201D}", "at R&D"] {
        let tmp = Tmp::new(&text, None);
        let (code, stdout, stderr) =
            tmp.comment(&["--quote", q, "--line", "3", "hi", "--as", "claude"], "");
        assert_eq!(code, 0, "{q:?}: {stdout}{stderr}");
        let line = tmp.last();
        let Op::Comment {
            at: At::Anchor(anchor),
            ..
        } = &line.op
        else {
            panic!("{q:?}: a quote anchor: {:?}", line.op);
        };
        assert_eq!(anchor.scope, Scope::Quote, "{q:?}");
        assert_eq!(anchor.lines, (3, 3), "{q:?}");
        assert!(
            ["It's \"quoted\"", "at R&amp;D"].contains(&anchor.quote.as_str()),
            "{q:?}: {:?}",
            anchor.quote
        );
    }
}

// --- races, hostile input and broken pipes ---

/// `birta comment pr.md args…` started in the background, its output kept.
fn spawn_comment(tmp: &Tmp, args: &[&str]) -> std::process::Child {
    StdCommand::new(assert_cmd::cargo::cargo_bin("birta"))
        .args(["comment", "pr.md"])
        .args(args)
        .current_dir(&tmp.base)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.path("home"))
        .env("BIRTA_REVIEW", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Waits for `child`, killing it past `limit`: exit code and stderr, `None` if killed.
fn finish_within(mut child: std::process::Child, limit: Duration) -> (Option<i32>, String) {
    let deadline = Instant::now() + limit;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    let killed = Instant::now() >= deadline;
    (
        (!killed).then(|| out.status.code()).flatten(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_save_while_the_comment_waits_for_the_lock_is_never_reverted() {
    let mut s = Scenario::new(DOC);
    s.note("human draft");
    let tmp = Tmp::with(&s);
    let saved = format!("{DOC}\nSaved while the agent waited.\n");
    let log = Log::open(&tmp.path(LOG));
    let mut held = log.hold().unwrap();
    let child = spawn_comment(&tmp, &["--quote", "second item", "x", "--as", "claude"]);
    // Long enough for the comment to read the doc and block on the lock
    std::thread::sleep(Duration::from_millis(500));
    fs::write(tmp.path("pr.md"), &saved).unwrap();
    let recorded = sync_external(&mut held, &tmp.path("pr.md"), 1).expect("the save is recorded");
    drop(held);
    let (code, stderr) = finish_within(child, Duration::from_secs(10));
    assert_eq!(code, Some(0), "{stderr}");

    let lines = tmp.lines();
    let after: Vec<&Line> = lines.iter().filter(|l| l.n > recorded).collect();
    let reverts = after
        .iter()
        .filter(
            |l| matches!(&l.op, Op::External { to_rev, .. } if *to_rev == Rev::of(DOC.as_bytes())),
        )
        .count();
    assert_eq!(reverts, 0, "a revert that never happened: {after:#?}");
    let parsed = ops::parse(tmp.log_bytes().unwrap());
    assert_eq!(
        Chain::build(&parsed).head(),
        Some(&Rev::of(saved.as_bytes()))
    );
    let Op::Comment {
        at: At::Anchor(anchor),
        ..
    } = &tmp.last().op
    else {
        panic!("a quote anchor: {:?}", tmp.last().op);
    };
    assert_eq!(
        anchor.rev,
        Rev::of(saved.as_bytes()),
        "anchored on the saved text"
    );
}

/// One 200 KB paragraph holding `word` 20 000 times.
fn crowded() -> String {
    format!("{}\n", "word abcd ".repeat(20_000).trim_end())
}

#[test]
fn a_quote_found_20k_times_in_one_paragraph_is_counted_in_under_1_s() {
    let text = crowded();
    let tmp = Tmp::new(&text, Some(&Scenario::new(&text).log()));
    for args in [
        &["--quote", "word"][..],
        &["--quote", "word", "--line", "1"],
    ] {
        let started = Instant::now();
        let out = tmp
            .cmd(&[])
            .args(["comment", "pr.md"])
            .args(args)
            .args(["x", "--as", "claude"])
            .timeout(Duration::from_secs(10))
            .output()
            .unwrap();
        let took = started.elapsed();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(65), "{args:?}: {stderr}");
        assert!(
            stderr.contains("matches 20000 places"),
            "{args:?}: {stderr}"
        );
        assert!(took < Duration::from_secs(1), "{args:?} took {took:?}");
    }
}

#[test]
fn a_name_with_control_characters_is_a_usage_error() {
    for name in ["cl\x1baude", "claude\nnext", "cl\u{202e}aude"] {
        let tmp = Tmp::bare();
        let (code, stderr) = refused_as(&tmp, name);
        assert_eq!(code, 2, "{name:?}: {stderr}");
    }
}

/// A comment run with `--as name`, which must write nothing.
fn refused_as(tmp: &Tmp, name: &str) -> (i32, String) {
    let (code, stdout, stderr) = tmp.comment(&["--doc", "hi", "--as", name], "");
    assert_ne!(code, 0, "{name:?} was accepted:\n{stdout}");
    assert!(tmp.log_bytes().is_none(), "{name:?} made the review file");
    (code, stderr)
}

#[test]
fn a_path_with_control_characters_is_refused() {
    let tmp = Tmp::bare();
    for name in ["odd\nnext.md", "odd\x1b[31m.md", "odd\u{202e}dm.md"] {
        fs::write(tmp.path(name), DOC).unwrap();
        let out = tmp
            .cmd(&[])
            .args(["comment", name, "--doc", "hi", "--as", "claude"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{name:?}: {stderr}");
        assert!(stderr.contains("path has control characters"), "{stderr}");
        assert!(!tmp.path(&format!("{name}.birta.jsonl")).exists());
    }
}

#[test]
fn an_ambiguous_quote_lists_its_lines_without_control_characters() {
    let text = "twin \x1b[31mred\u{202e} one\n\ntwin \x1b[31mred\u{202e} two\n";
    let tmp = Tmp::new(text, None);
    let (code, stdout, stderr) = tmp.comment(&["--quote", "twin", "x", "--as", "claude"], "");
    assert_eq!(code, 65, "{stdout}{stderr}");
    assert!(stderr.contains("matches 2 places"), "{stderr}");
    assert!(
        !stderr.contains('\x1b') && !stderr.contains('\u{202e}'),
        "{stderr:?}"
    );
}

#[test]
fn a_fifo_in_place_of_the_doc_is_refused_without_hanging() {
    let mut s = Scenario::new(DOC);
    s.note("human draft");
    let tmp = Tmp::with(&s);
    let before = tmp.log_bytes();
    fs::remove_file(tmp.path("pr.md")).unwrap();
    let path = std::ffi::CString::new(tmp.path("pr.md").to_str().unwrap()).unwrap();
    // SAFETY: a valid C string for the call.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o644) }, 0);
    let child = spawn_comment(&tmp, &["--doc", "hi", "--as", "claude"]);
    let (code, stderr) = finish_within(child, Duration::from_secs(5));
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(tmp.log_bytes(), before);
}

#[test]
fn a_reader_that_went_away_leaves_one_landed_note_and_exit_0() {
    let tmp = Tmp::bare();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = StdCommand::new(assert_cmd::cargo::cargo_bin("birta"))
        .args(["comment", "pr.md", "--doc", "hi", "--as", "claude"])
        .current_dir(&tmp.base)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.path("home"))
        .env("BIRTA_REVIEW", "1")
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    let notes = tmp
        .lines()
        .iter()
        .filter(|l| matches!(l.op, Op::Comment { .. }))
        .count();
    assert_eq!(notes, 1);
}
