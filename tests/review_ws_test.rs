//! Browser review over the real socket: ops, the hold, compose drafts, the beacon, the
//! gate and the review assets, each against a served `birta` process.

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use birta::anchor::{OwnerKind, StopKind};
use birta::rev::Rev;
use birta::review::doc::Doc;
use serde_json::{Value, json};

mod support {
    pub mod scenario;
    pub mod served;
}
use support::scenario::{DOC, Scenario, human};
use support::served::{
    GATE_ON, Served, Site, Ws, acked, body_of, drain, eventually, exchange, header, log_lines,
    nacked, next_within, ops_named, reply, send, until,
};

const PR: &str = "pr.md";
const LOG: &str = "pr.md.birta.jsonl";
const HOLD_1500: (&str, &str) = ("BIRTA_REVIEW_HOLD_MS", "1500");

/// A doc with one task, for checkbox writes.
const TASKS: &str = "# Tasks\n\n- [ ] ship it\n";

fn rev(text: &str) -> String {
    Rev::of(text.as_bytes()).as_str().to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// An `op` frame on `pr.md` at `rev`, with the op's own fields in `fields`.
fn op(cid: &str, rev: &str, name: &str, fields: Value) -> Value {
    let mut frame = json!({"type": "op", "cid": cid, "path": PR, "rev": rev, "op": name});
    for (k, v) in fields.as_object().unwrap() {
        frame[k] = v.clone();
    }
    frame
}

/// The `sp` of the stop starting on the line of `text` that holds `needle`.
fn stop_sp(text: &str, needle: &str) -> String {
    let line = line_of(text, needle);
    Doc::new(text)
        .blocks
        .stops
        .iter()
        .find(|s| s.lines.0 == line)
        .unwrap_or_else(|| panic!("no stop starts on line {line} ({needle:?})"))
        .sp
        .clone()
}

/// 1-based line of the first line holding `needle`.
fn line_of(text: &str, needle: &str) -> u32 {
    text.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is not in the doc")) as u32
        + 1
}

fn review_file(site: &Site) -> PathBuf {
    site.path(LOG)
}

/// The hello's own `review_state`: the frame right after its `content`.
fn hello_state(frames: &[Value]) -> &Value {
    let i = frames
        .iter()
        .position(|f| f["type"] == "content")
        .unwrap_or_else(|| panic!("no content: {frames:#?}"));
    match frames.get(i + 1) {
        Some(f) if f["type"] == "review_state" => f,
        _ => panic!("no review_state after the hello's content: {frames:#?}"),
    }
}

// --- the op fixtures: the frames the page sends, replayed through the real handler ---

fn ops_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("e2e/fixtures/review/ops")
}

/// One op fixture: the doc served as `pr.md`, its review file if any, frames that must
/// be acked first, and the frame under test with the reply type it must get.
fn fixture(
    doc: &str,
    log: Option<&Scenario>,
    before: Vec<Value>,
    frame: Value,
    want: &str,
) -> Value {
    json!({
        "doc": doc,
        "log": log.map(Scenario::log),
        "before": before,
        "frame": frame,
        "reply": want,
    })
}

/// A note, handed back and claimed by `claude`, whose turn made the write `new`.
/// Returns the scenario and the line of the write.
fn agent_changed(new: impl FnOnce(&Scenario) -> String) -> (Scenario, u32) {
    let mut s = Scenario::new(DOC);
    s.note("Tighten the details.");
    s.submit();
    let turn = s.claim("claude");
    let text = new(&s);
    let n = s.external(&text);
    s.yield_turn("claude", turn);
    let state = s.fold();
    let change = state
        .changes
        .iter()
        .find(|c| c.id == format!("{n}.0"))
        .expect("the write is a change the page can point at");
    assert!(change.reversible.ok, "setup: {:?}", change.reversible);
    (s, n)
}

fn op_fixtures() -> Vec<(&'static str, Value)> {
    let r = rev(DOC);
    let noted = || {
        let mut s = Scenario::new(DOC);
        let n = s.note("Tighten this.");
        (s, n)
    };
    let mut out = Vec::new();

    out.push((
        "comment-doc",
        fixture(
            DOC,
            None,
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "doc"}, "body": "Reads well overall."}),
            ),
            "ack",
        ),
    ));
    out.push((
        "comment-dir",
        fixture(
            DOC,
            None,
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "dir"}, "body": "Make these consistent."}),
            ),
            "ack",
        ),
    ));
    out.push((
        "comment-block",
        fixture(
            DOC,
            None,
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "block", "sp": stop_sp(DOC, "second item"), "scope": "block"}, "verdict": "no", "body": "Drop this one."}),
            ),
            "ack",
        ),
    ));
    out.push((
        "comment-section",
        fixture(
            DOC,
            Some(&Scenario::new(DOC)),
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "block", "sp": stop_sp(DOC, "## Details"), "scope": "section"}, "verdict": "yes"}),
            ),
            "ack",
        ),
    ));
    let para = Doc::new(DOC)
        .blocks
        .owners
        .iter()
        .find(|o| o.kind == OwnerKind::Paragraph && o.lines.0 == line_of(DOC, "Intro paragraph"))
        .unwrap()
        .sp
        .clone();
    out.push((
        "comment-quote",
        fixture(
            DOC,
            Some(&Scenario::new(DOC)),
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "quote", "sp": para, "q": "second line", "pre": "with a ", "suf": "."}, "verdict": "rewrite", "body": "Say why."}),
            ),
            "ack",
        ),
    ));
    // A verdict key's frames: no body, and a selection carries the extractor's offsets
    out.push((
        "comment-doc-verdict",
        fixture(
            DOC,
            None,
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "doc"}, "verdict": "no"}),
            ),
            "ack",
        ),
    ));
    out.push((
        "comment-quote-offsets",
        fixture(
            DOC,
            None,
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "quote", "sp": para, "q": "second line", "pre": "Intro paragraph here, with a ", "suf": ".", "off": 29, "tl": 41}, "verdict": "no"}),
            ),
            "ack",
        ),
    ));
    out.push((
        "comment-quote-without-sp",
        fixture(
            DOC,
            Some(&Scenario::new(DOC)),
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "quote", "q": "detail two", "pre": "", "suf": ""}, "body": "Which detail?"}),
            ),
            "ack",
        ),
    ));
    let (changed, n) = agent_changed(|s| s.replaced("- detail one", "- detail one, with retries"));
    out.push((
        "comment-change",
        fixture(
            changed.text(),
            Some(&changed),
            vec![],
            op(
                "c1",
                &rev(changed.text()),
                "comment",
                json!({"at": {"k": "change", "id": format!("{n}.0")}, "body": "Why retries?"}),
            ),
            "ack",
        ),
    ));
    let (s, note) = noted();
    out.push((
        "comment-aside",
        fixture(
            DOC,
            Some(&s),
            vec![],
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "doc"}, "kind": "aside", "to": note, "body": "Only if it's cheap."}),
            ),
            "ack",
        ),
    ));
    out.push((
        "amend",
        fixture(
            DOC,
            Some(&s),
            vec![],
            op(
                "c1",
                &r,
                "amend",
                json!({"to": note, "body": "Tighten this, a lot."}),
            ),
            "ack",
        ),
    ));
    out.push((
        "amend-drop-verdict",
        fixture(
            DOC,
            Some(&s),
            vec![],
            op("c1", &r, "amend", json!({"to": note, "drop_verdict": true})),
            "ack",
        ),
    ));
    out.push((
        "retract",
        fixture(
            DOC,
            Some(&s),
            vec![],
            op("c1", &r, "retract", json!({"to": note})),
            "ack",
        ),
    ));
    let mut asked = Scenario::new(DOC);
    asked.note("Is the table right?");
    asked.submit();
    let turn = asked.claim("claude");
    let blank = asked.blank("claude", asked.block("| a | 1 |"), "Should a be 1?", turn);
    asked.yield_turn("claude", turn);
    out.push((
        "reply",
        fixture(
            DOC,
            Some(&asked),
            vec![],
            op("c1", &r, "reply", json!({"to": blank, "body": "Yes, 1."})),
            "ack",
        ),
    ));
    out.push((
        "seen",
        fixture(
            changed.text(),
            Some(&changed),
            vec![],
            op(
                "c1",
                &rev(changed.text()),
                "seen",
                json!({"ids": [format!("{n}.0")]}),
            ),
            "ack",
        ),
    ));
    for (name, scope) in [("hand_back-doc", "doc"), ("hand_back-all", "all")] {
        out.push((
            name,
            fixture(
                DOC,
                Some(&s),
                vec![],
                op("c1", &r, "hand_back", json!({"scope": scope})),
                "ack",
            ),
        ));
    }
    out.push((
        "approve",
        fixture(
            DOC,
            Some(&Scenario::new(DOC)),
            vec![],
            op("c1", &r, "approve", json!({})),
            "ack",
        ),
    ));
    out.push((
        "withdraw",
        fixture(
            DOC,
            Some(&s),
            vec![op("c0", &r, "hand_back", json!({"scope": "doc"}))],
            op("c1", &r, "withdraw", json!({"to": s.next()})),
            "ack",
        ),
    ));
    let mut claimed = Scenario::new(DOC);
    claimed.note("Tighten this.");
    claimed.submit();
    let claim = claimed.claim("claude");
    out.push((
        "release",
        fixture(
            DOC,
            Some(&claimed),
            vec![],
            op("c1", &r, "release", json!({"claim": claim})),
            "ack",
        ),
    ));
    out.push((
        "owner",
        fixture(
            DOC,
            Some(&Scenario::drafted_by(DOC, "claude")),
            vec![],
            op("c1", &r, "owner", json!({"drafted_by": null})),
            "ack",
        ),
    ));
    let (one, n) = agent_changed(|s| s.replaced("- detail one", "- detail one, with retries"));
    out.push((
        "undo_change",
        fixture(
            one.text(),
            Some(&one),
            vec![],
            op(
                "c1",
                &rev(one.text()),
                "undo_change",
                json!({"change": format!("{n}.0")}),
            ),
            "ack",
        ),
    ));
    let (two, n) = agent_changed(|s| {
        s.text()
            .replace("- first item\n", "- first item, reworded\n")
            .replace("- more item\n", "- more item, reworded\n")
    });
    out.push((
        "undo_change-confirm",
        fixture(
            two.text(),
            Some(&two),
            vec![],
            op(
                "c1",
                &rev(two.text()),
                "undo_change",
                json!({"change": format!("{n}.0"), "confirm": true}),
            ),
            "ack",
        ),
    ));
    out.push((
        "checkbox",
        fixture(
            TASKS,
            Some(&Scenario::new(TASKS)),
            vec![],
            json!({"type": "checkbox", "cid": "c1", "path": PR, "line": 3, "checked": true, "rev": rev(TASKS)}),
            "ack",
        ),
    ));
    out.push((
        "resolve",
        fixture(
            DOC,
            None,
            vec![],
            json!({"type": "resolve", "cid": "c1", "path": PR, "rev": r, "text": "detail two"}),
            "resolved",
        ),
    ));
    out
}

/// Rewrite with `BIRTA_UPDATE_FIXTURES=1`.
#[test]
fn op_fixtures_match_the_committed_json() {
    let dir = ops_dir();
    let update = std::env::var_os("BIRTA_UPDATE_FIXTURES").is_some_and(|v| v == "1");
    let fixtures = op_fixtures();
    let mut stale = Vec::new();
    for (name, fixture) in &fixtures {
        let path = dir.join(format!("{name}.json"));
        let want = serde_json::to_string_pretty(fixture).unwrap() + "\n";
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &want).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(want.as_str()) {
            stale.push(name.to_string());
        }
    }
    assert!(
        stale.is_empty(),
        "out of date: {stale:?}; rerun with BIRTA_UPDATE_FIXTURES=1"
    );
    let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let want: BTreeSet<String> = fixtures.iter().map(|(n, _)| format!("{n}.json")).collect();
    assert_eq!(on_disk, want, "stray or missing fixture files");
}

fn committed_fixtures() -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = std::fs::read_dir(ops_dir())
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (name, serde_json::from_str(&text).unwrap())
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn op_fixtures_cover_every_frozen_frame() {
    let covered: BTreeSet<String> = committed_fixtures()
        .iter()
        .map(|(_, f)| match f["frame"]["type"].as_str().unwrap() {
            "op" => f["frame"]["op"].as_str().unwrap().to_string(),
            other => other.to_string(),
        })
        .collect();
    let frozen = [
        "comment",
        "amend",
        "retract",
        "reply",
        "seen",
        "hand_back",
        "approve",
        "withdraw",
        "release",
        "owner",
        "undo_change",
        "checkbox",
        "resolve",
    ];
    let frozen: BTreeSet<String> = frozen.iter().map(|s| s.to_string()).collect();
    assert_eq!(covered, frozen);
    let kinds: BTreeSet<String> = committed_fixtures()
        .iter()
        .filter(|(_, f)| f["frame"]["op"] == "comment")
        .map(|(_, f)| {
            let at = &f["frame"]["at"];
            match (at["k"].as_str().unwrap(), at.get("sp").is_some()) {
                ("quote", false) => "quote without sp".to_string(),
                (k, _) => k.to_string(),
            }
        })
        .collect();
    let want: BTreeSet<String> = ["doc", "dir", "block", "quote", "quote without sp", "change"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(kinds, want, "every page anchor kind");
}

#[tokio::test]
async fn every_op_fixture_is_acked_by_the_real_handler() {
    let mut failures = Vec::new();
    for (name, fixture) in committed_fixtures() {
        let site = Site::new();
        site.write(PR, fixture["doc"].as_str().unwrap());
        if let Some(log) = fixture["log"].as_str() {
            site.write(LOG, log);
        }
        // A note on the folder exists only where a folder is served
        let args: &[&str] = if name == "comment-dir" { &["."] } else { &[PR] };
        let served = site.serve(args, &[GATE_ON]);
        let (mut ws, _) = served.viewing(PR).await;
        let mut ok = true;
        for frame in fixture["before"].as_array().unwrap() {
            match reply(&mut ws, frame.clone()).await {
                Some(r) if r["type"] == "ack" => {}
                other => {
                    failures.push(format!("{name}: setup frame {frame} got {other:?}"));
                    ok = false;
                }
            }
        }
        if !ok {
            continue;
        }
        let want = fixture["reply"].as_str().unwrap();
        match reply(&mut ws, fixture["frame"].clone()).await {
            Some(r) if r["type"] == want => {
                if fixture["frame"]["type"] == "op" && !r["n"].is_u64() {
                    failures.push(format!("{name}: ack without a line number: {r}"));
                }
            }
            other => failures.push(format!("{name}: want {want}, got {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn undoing_a_multi_hunk_change_needs_confirm() {
    let fixture = committed_fixtures()
        .into_iter()
        .find(|(name, _)| name == "undo_change-confirm")
        .unwrap()
        .1;
    let doc = fixture["doc"].as_str().unwrap();
    let site = Site::new();
    site.write(PR, doc);
    site.write(LOG, fixture["log"].as_str().unwrap());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, frames) = served.viewing(PR).await;
    let mut frame = fixture["frame"].clone();
    let id = frame["change"].clone();
    let change = hello_state(&frames)["state"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .cloned()
        .unwrap();
    assert_eq!(change["op_hunks"], 2, "setup: {change}");
    frame.as_object_mut().unwrap().remove("confirm");
    let lines = log_lines(&review_file(&site)).len();
    assert_eq!(nacked(&mut ws, frame).await, "not_reversible");
    assert_eq!(site.read(PR).as_deref(), Some(doc), "the doc is untouched");
    assert_eq!(log_lines(&review_file(&site)).len(), lines);
}

/// Serves `fixture` with its doc changed on disk since the frames were made, sends its
/// setup frames and returns the reply to its frame, and the site.
async fn replay_on_a_changed_doc(name: &str) -> (Option<Value>, Site) {
    let fixture = committed_fixtures()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no fixture {name}"))
        .1;
    let site = Site::new();
    let doc = fixture["doc"].as_str().unwrap();
    site.write(
        PR,
        &format!("{doc}\nA line written since the page loaded.\n"),
    );
    if let Some(log) = fixture["log"].as_str() {
        site.write(LOG, log);
    }
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let mut last = Value::Null;
    for frame in fixture["before"].as_array().unwrap() {
        last = acked(&mut ws, frame.clone()).await["n"].clone();
    }
    let mut frame = fixture["frame"].clone();
    // The outside write is recorded first, so a round opened here lands a line later
    if frame["op"] == "withdraw" {
        frame["to"] = last;
    }
    let got = reply(&mut ws, frame).await;
    (got, site)
}

#[tokio::test]
async fn ops_that_read_the_text_are_refused_on_a_rev_the_doc_has_left() {
    let mut failures = Vec::new();
    for name in [
        "comment-block",
        "comment-quote",
        "approve",
        "undo_change",
        "checkbox",
    ] {
        let (got, site) = replay_on_a_changed_doc(name).await;
        if got.as_ref().map(|r| (&r["type"], &r["reason"]))
            != Some((&json!("nack"), &json!("changed_since")))
        {
            failures.push(format!("{name}: {got:?}"));
        }
        let fixture = &committed_fixtures()
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1;
        if fixture["log"].is_null() && review_file(&site).exists() {
            failures.push(format!("{name}: a refused first op created the file"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn ops_that_do_not_read_the_text_ignore_the_rev() {
    let mut failures = Vec::new();
    for name in [
        "seen",
        "hand_back-doc",
        "withdraw",
        "release",
        "owner",
        "amend",
        "reply",
        "retract",
        "comment-doc",
    ] {
        match replay_on_a_changed_doc(name).await.0 {
            Some(r) if r["type"] == "ack" => {}
            other => failures.push(format!("{name}: {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// --- notes on a change ---

#[tokio::test]
async fn a_note_on_a_change_anchors_to_its_block_or_to_the_doc_once_deleted() {
    let (kept, n) = agent_changed(|s| s.replaced("- detail one", "- detail one, with retries"));
    let (mut gone, gone_n) =
        agent_changed(|s| s.replaced("- detail one", "- detail one, with retries"));
    let deleted = gone.text().replace("- detail one, with retries\n", "");
    gone.external(&deleted);
    let state = gone.fold();
    assert!(
        state.changes.iter().any(|c| c.id == format!("{gone_n}.0")),
        "setup: the change is still listed"
    );
    let cases = [
        (&kept, n, json!(line_of(kept.text(), "with retries"))),
        (&gone, gone_n, Value::Null),
    ];
    for (s, n, want_line) in cases {
        let site = Site::new();
        site.write(PR, s.text());
        site.write(LOG, &s.log());
        let served = site.serve(&[PR], &[GATE_ON]);
        let (mut ws, _) = served.viewing(PR).await;
        let frame = op(
            "c1",
            &rev(s.text()),
            "comment",
            json!({"at": {"k": "change", "id": format!("{n}.0")}, "body": "Why?"}),
        );
        let ack = acked(&mut ws, frame).await;
        let line = &log_lines(&review_file(&site))[ack["n"].as_u64().unwrap() as usize - 1];
        if want_line.is_null() {
            assert_eq!(line["at"], "doc", "{line}");
        } else {
            assert_eq!(line["at"]["scope"], "block", "{line}");
            assert_eq!(line["at"]["lines"][0], want_line, "{line}");
        }
    }
}

// --- hello ---

#[tokio::test]
async fn hello_sends_content_then_review_state_then_dir_state() {
    for with_log in [false, true] {
        let site = Site::new();
        site.write(PR, DOC);
        if with_log {
            site.write(LOG, &Scenario::new(DOC).log());
        }
        let served = site.serve(&[PR], &[GATE_ON]);
        let (_ws, frames) = served.viewing(PR).await;
        let types: Vec<&str> = frames
            .iter()
            .map(|f| f["type"].as_str().unwrap_or("?"))
            .collect();
        // A push from the startup scan may land around the hello's own three frames
        let at = types.iter().position(|&t| t == "content");
        let hello = at.map(|i| &types[i..(i + 3).min(types.len())]);
        assert_eq!(
            hello,
            Some(&["content", "review_state", "dir_state"][..]),
            "log {with_log}: {frames:#?}"
        );
        let extra = types
            .iter()
            .enumerate()
            .filter(|&(j, _)| !(at.unwrap()..at.unwrap() + 3).contains(&j));
        for (_, t) in extra {
            assert!(["review_state", "dir_state"].contains(t), "{frames:#?}");
        }
        let state = hello_state(&frames);
        assert_eq!(state["path"], PR);
        assert_eq!(state["rev"], rev(DOC));
        assert_eq!(state["state"]["schema"], "review-state/1");
        assert_eq!(state["state"]["rev"], rev(DOC));
    }
}

/// Every `review_state` for `path` whose rev no earlier content frame carried.
fn states_before_their_content(path: &str, first: &str, frames: &[Value]) -> Vec<String> {
    let mut shown: BTreeSet<String> = BTreeSet::from([first.to_string()]);
    let mut out = Vec::new();
    for f in frames.iter().filter(|f| f["path"] == path) {
        let rev = f["rev"].as_str().unwrap_or("").to_string();
        let carries_doc =
            f["type"] == "content" || (f["type"] == "theme_update" && !f["html"].is_null());
        if carries_doc {
            shown.insert(rev);
        } else if f["type"] == "review_state" && !shown.contains(&rev) {
            out.push(rev);
        }
    }
    out
}

#[tokio::test]
async fn a_doc_saved_again_and_again_sends_each_state_after_the_content_of_its_rev() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    // A review file, so every save pushes a state as well as the content
    acked(
        &mut ws,
        op(
            "c1",
            &rev(DOC),
            "comment",
            json!({"at": {"k": "doc"}, "body": "first"}),
        ),
    )
    .await;
    let mut text = DOC.to_string();
    for i in 1..=6 {
        text.push_str(&format!("\nSaved paragraph {i}.\n"));
        site.write(PR, &text);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let last = rev(&text);
    let (mut frames, end) = until(&mut ws, "the last save's state", |f| {
        f["type"] == "review_state" && f["rev"] == last
    })
    .await;
    frames.push(end);
    let early = states_before_their_content(PR, &rev(DOC), &frames);
    assert!(
        early.is_empty(),
        "states before their content {early:?}: {frames:#?}"
    );
}

#[tokio::test]
async fn an_outside_write_recorded_in_the_log_never_sends_its_state_before_the_content() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    acked(
        &mut ws,
        op(
            "c1",
            &rev(DOC),
            "comment",
            json!({"at": {"k": "doc"}, "body": "first"}),
        ),
    )
    .await;
    // One save: the doc event records it in the log, and that log write is its own event
    let changed = format!("{DOC}\nAn agent's closing line.\n");
    site.write(PR, &changed);
    let (mut frames, end) = until(&mut ws, "the save's state", |f| {
        f["type"] == "review_state" && f["rev"] == rev(&changed)
    })
    .await;
    frames.push(end);
    frames.extend(drain(&mut ws, Duration::from_millis(500)).await);
    let early = states_before_their_content(PR, &rev(DOC), &frames);
    assert!(
        early.is_empty(),
        "states before their content {early:?}: {frames:#?}"
    );
    assert!(
        frames
            .iter()
            .any(|f| f["type"] == "content" && f["rev"] == rev(&changed)),
        "{frames:#?}"
    );
}

#[tokio::test]
async fn a_checkbox_write_sends_its_state_after_the_content_of_its_rev() {
    let site = Site::new();
    site.write(PR, TASKS);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    acked(
        &mut ws,
        op(
            "c1",
            &rev(TASKS),
            "comment",
            json!({"at": {"k": "doc"}, "body": "first"}),
        ),
    )
    .await;
    let ticked = TASKS.replace("[ ]", "[x]");
    send(
        &mut ws,
        json!({"type": "checkbox", "cid": "k1", "path": PR, "line": 3, "checked": true, "rev": rev(TASKS)}),
    )
    .await;
    let (mut frames, end) = until(&mut ws, "the tick's state", |f| {
        f["type"] == "review_state" && f["rev"] == rev(&ticked)
    })
    .await;
    frames.push(end);
    frames.extend(drain(&mut ws, Duration::from_millis(500)).await);
    assert_eq!(
        site.read(PR).unwrap(),
        ticked,
        "setup: the tick wrote the doc"
    );
    let early = states_before_their_content(PR, &rev(TASKS), &frames);
    assert!(
        early.is_empty(),
        "states before their content {early:?}: {frames:#?}"
    );
}

#[tokio::test]
async fn an_undo_sends_its_state_after_the_content_of_its_rev() {
    let fixture = committed_fixtures()
        .into_iter()
        .find(|(n, _)| n == "undo_change")
        .unwrap()
        .1;
    let doc = fixture["doc"].as_str().unwrap();
    let site = Site::new();
    site.write(PR, doc);
    site.write(LOG, fixture["log"].as_str().unwrap());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    send(&mut ws, fixture["frame"].clone()).await;
    let (mut frames, end) = until(&mut ws, "the undo's ack", |f| f["type"] == "ack").await;
    frames.push(end);
    let undone = site.read(PR).unwrap();
    assert_ne!(undone, doc, "setup: the undo wrote the doc");
    let (more, end) = until(&mut ws, "the undo's state", |f| {
        f["type"] == "review_state" && f["rev"] == rev(&undone)
    })
    .await;
    frames.extend(more);
    frames.push(end);
    frames.extend(drain(&mut ws, Duration::from_millis(500)).await);
    let early = states_before_their_content(PR, &rev(doc), &frames);
    assert!(
        early.is_empty(),
        "states before their content {early:?}: {frames:#?}"
    );
}

#[tokio::test]
async fn a_second_tab_on_the_doc_gets_each_state_after_its_content_too() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut a, _) = served.viewing(PR).await;
    let (mut b, _) = served.viewing(PR).await;
    acked(
        &mut a,
        op(
            "c1",
            &rev(DOC),
            "comment",
            json!({"at": {"k": "doc"}, "body": "first"}),
        ),
    )
    .await;
    let mut text = DOC.to_string();
    for i in 1..=6 {
        text.push_str(&format!("\nSaved paragraph {i}.\n"));
        site.write(PR, &text);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let last = rev(&text);
    let (mut frames, end) = until(&mut b, "the last save's state", |f| {
        f["type"] == "review_state" && f["rev"] == last
    })
    .await;
    frames.push(end);
    let early = states_before_their_content(PR, &rev(DOC), &frames);
    assert!(early.is_empty(), "second tab: {early:?}: {frames:#?}");
}

#[tokio::test]
async fn a_doc_deleted_with_a_state_waiting_still_gets_its_states_once_it_is_back() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    acked(
        &mut ws,
        op(
            "c1",
            &rev(DOC),
            "comment",
            json!({"at": {"k": "doc"}, "body": "first"}),
        ),
    )
    .await;
    // A save then a delete in one burst: its state may be waiting when the doc goes
    site.write(PR, &format!("{DOC}\nGone soon.\n"));
    std::fs::remove_file(site.path(PR)).unwrap();
    let (mut frames, end) = until(&mut ws, "gone", |f| f["type"] == "gone").await;
    frames.push(end);
    let back = format!("{DOC}\nBack again.\n");
    site.write(PR, &back);
    let (more, end) = until(&mut ws, "the state of the doc that came back", |f| {
        f["type"] == "review_state" && f["rev"] == rev(&back)
    })
    .await;
    frames.extend(more);
    frames.push(end);
    let early = states_before_their_content(PR, &rev(DOC), &frames);
    assert!(
        early.is_empty(),
        "states before their content {early:?}: {frames:#?}"
    );
}

/// Serves a doc with no review file and returns its hello state.
async fn state_without_a_file(args: &[&str], envs: &[(&str, &str)], site: &Site) -> Value {
    let served = site.serve(args, envs);
    let (_ws, frames) = served.viewing(PR).await;
    hello_state(&frames)["state"].clone()
}

#[tokio::test]
async fn with_no_review_file_the_state_is_a_fold_of_nothing_yet() {
    let site = Site::new();
    site.write(PR, DOC);
    let state = state_without_a_file(&[PR], &[GATE_ON], &site).await;
    assert_eq!(state["log"]["lines"], 0, "{state}");
    assert_eq!(state["owner"]["drafted_by"], Value::Null);
    assert_eq!(state["phrase"]["stage"], "idle");
    assert_eq!(state["writable"], true);
    assert_eq!(state["log_writable"], true);
    let drafted = state_without_a_file(&["--drafted-by", "claude", PR], &[GATE_ON], &site).await;
    assert_eq!(drafted["owner"]["drafted_by"], "claude", "{drafted}");
    assert_eq!(drafted["log"]["lines"], 0);
    {
        let _doc = chmod(&site.path(PR), 0o444);
        let _dir = chmod(&site.root, 0o555);
        let locked = state_without_a_file(&[PR], &[GATE_ON], &site).await;
        assert_eq!(locked["writable"], false, "{locked}");
        assert_eq!(locked["log_writable"], false, "{locked}");
    }
    assert!(!review_file(&site).exists(), "a state never writes a file");
}

#[tokio::test]
async fn with_the_gate_off_hello_sends_only_content() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write(LOG, &Scenario::new(DOC).log());
    let served = site.serve(&[PR], &[]);
    let (_ws, frames) = served.viewing(PR).await;
    let types: Vec<&str> = frames
        .iter()
        .map(|f| f["type"].as_str().unwrap_or("?"))
        .collect();
    assert_eq!(types, ["content"], "{frames:#?}");
}

// --- revs and anchors ---

#[tokio::test]
async fn a_rev_the_doc_has_left_is_changed_since_and_one_never_recorded_is_stale_rev() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let r0 = rev(DOC);
    let block = json!({"at": {"k": "block", "sp": stop_sp(DOC, "second item"), "scope": "block"}, "body": "x"});
    acked(&mut ws, op("c1", &r0, "comment", block.clone())).await;
    let changed = format!("{DOC}\nA new closing line.\n");
    site.write(PR, &changed);
    assert_eq!(
        nacked(&mut ws, op("c2", &r0, "comment", block.clone())).await,
        "changed_since"
    );
    let never = rev("a text no log ever saw\n");
    assert_eq!(
        nacked(&mut ws, op("c3", &never, "comment", block)).await,
        "stale_rev"
    );
    assert_eq!(ops_named(&review_file(&site), "comment").len(), 1);
}

#[tokio::test]
async fn a_forged_sp_is_bad_anchor_both_ways_round() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let doc = Doc::new(DOC);
    let row = stop_sp(DOC, "| a | 1 |");
    let cell = doc
        .blocks
        .owners
        .iter()
        .find(|o| o.kind == OwnerKind::Cell && o.lines.0 == line_of(DOC, "| a | 1 |"))
        .unwrap()
        .sp
        .clone();
    assert!(
        doc.blocks.owners.iter().all(|o| o.sp != row),
        "a row is no owner"
    );
    assert!(
        doc.blocks.stops.iter().all(|s| s.sp != cell),
        "a cell is no stop"
    );
    let r = rev(DOC);
    let cases = [
        (
            "block on a cell",
            json!({"k": "block", "sp": cell, "scope": "block"}),
        ),
        (
            "block on nothing",
            json!({"k": "block", "sp": "99:1-99:9", "scope": "block"}),
        ),
        (
            "quote on a row",
            json!({"k": "quote", "sp": row, "q": "a", "pre": "", "suf": ""}),
        ),
        (
            "quote on nothing",
            json!({"k": "quote", "sp": "99:1-99:9", "q": "a", "pre": "", "suf": ""}),
        ),
        (
            "section on a paragraph",
            json!({"k": "block", "sp": stop_sp(DOC, "Intro paragraph"), "scope": "section"}),
        ),
    ];
    for (i, (what, at)) in cases.into_iter().enumerate() {
        let frame = op(
            &format!("c{i}"),
            &r,
            "comment",
            json!({"at": at, "body": "x"}),
        );
        assert_eq!(nacked(&mut ws, frame).await, "bad_anchor", "{what}");
    }
    assert!(!review_file(&site).exists(), "a refused op creates no file");
}

/// Every owner kind, and each paragraph flavour, in one doc.
const KINDS: &str = "\
# Kinds heading

A plain paragraph here.

- tight alpha
- tight beta

Between the lists.

- loose gamma

  loose second para

- loose delta

| col | val |
|-----|-----|
| cellword | 1 |

Termword
: Detailword here

> quoted paragraph words

> [!NOTE]
> alert paragraph words

```rust
let codeword = 1;
```
";

#[tokio::test]
async fn a_quote_on_each_owner_kind_is_accepted() {
    let cases = [
        (
            "paragraph",
            "A plain paragraph",
            OwnerKind::Paragraph,
            "plain paragraph",
        ),
        ("tight item", "tight alpha", OwnerKind::Item, "tight alpha"),
        (
            "loose item paragraph",
            "loose gamma",
            OwnerKind::Paragraph,
            "loose gamma",
        ),
        (
            "heading",
            "Kinds heading",
            OwnerKind::Heading,
            "Kinds heading",
        ),
        ("table cell", "cellword", OwnerKind::Cell, "cellword"),
        ("dt", "Termword", OwnerKind::Term, "Termword"),
        ("dd", "Detailword", OwnerKind::Detail, "Detailword"),
        (
            "blockquote paragraph",
            "quoted paragraph",
            OwnerKind::Paragraph,
            "quoted paragraph",
        ),
        (
            "alert paragraph",
            "alert paragraph",
            OwnerKind::Paragraph,
            "alert paragraph",
        ),
        ("code block", "codeword", OwnerKind::Code, "codeword"),
    ];
    let doc = Doc::new(KINDS);
    let owners: Vec<(&str, String, &str)> = cases
        .iter()
        .map(|&(what, needle, kind, q)| {
            let line = line_of(KINDS, needle);
            let owner = doc
                .blocks
                .owners
                .iter()
                .filter(|o| o.kind == kind && o.lines.0 <= line && line <= o.lines.1)
                // Two cells share a line; the one holding the words
                .filter(|o| o.kind != OwnerKind::Cell || KINDS[o.bytes.clone()].contains(q))
                .min_by_key(|o| o.bytes.len())
                .unwrap_or_else(|| {
                    panic!("no {what} owner on line {line}: {:#?}", doc.blocks.owners)
                });
            (what, owner.sp.clone(), q)
        })
        .collect();
    let site = Site::new();
    site.write(PR, KINDS);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let mut failures = Vec::new();
    for (i, (what, sp, q)) in owners.into_iter().enumerate() {
        let at = json!({"k": "quote", "sp": sp, "q": q, "pre": "", "suf": ""});
        let frame = op(
            &format!("k{i}"),
            &rev(KINDS),
            "comment",
            json!({"at": at, "body": what}),
        );
        match reply(&mut ws, frame).await {
            Some(r) if r["type"] == "ack" => {
                let n = r["n"].as_u64().unwrap() as usize;
                let line = &log_lines(&review_file(&site))[n - 1];
                if line["at"]["scope"] != "quote" || line["at"]["quote"] != q {
                    failures.push(format!("{what}: stored {}", line["at"]));
                }
            }
            other => failures.push(format!("{what}: {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// --- dedupe ---

#[tokio::test]
async fn a_replayed_cid_is_acked_again_and_written_once() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let frame = op(
        "dup-1",
        &rev(DOC),
        "comment",
        json!({"at": {"k": "doc"}, "body": "Once."}),
    );
    let (mut ws, _) = served.viewing(PR).await;
    let first = acked(&mut ws, frame.clone()).await;
    drop(ws);
    let (mut ws, _) = served.viewing(PR).await;
    let again = acked(&mut ws, frame).await;
    assert_eq!(first["n"], again["n"]);
    let written: Vec<Value> = log_lines(&review_file(&site))
        .into_iter()
        .filter(|l| l["cid"] == "dup-1")
        .collect();
    assert_eq!(written.len(), 1, "{written:#?}");
}

// --- approve ---

#[tokio::test]
async fn an_approve_right_after_an_outside_write_is_changed_since_and_the_write_is_recorded() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let r0 = rev(DOC);
    acked(
        &mut ws,
        op(
            "c1",
            &r0,
            "comment",
            json!({"at": {"k": "doc"}, "body": "ok"}),
        ),
    )
    .await;
    let changed = format!("{DOC}\nAn agent's closing line.\n");
    site.write(PR, &changed);
    assert_eq!(
        nacked(&mut ws, op("c2", &r0, "approve", json!({}))).await,
        "changed_since"
    );
    let log = review_file(&site);
    let externals: Vec<Value> = ops_named(&log, "external")
        .into_iter()
        .filter(|l| l["to_rev"] == rev(&changed))
        .collect();
    assert_eq!(
        externals.len(),
        1,
        "the write is recorded once, before the refusal"
    );
    assert!(ops_named(&log, "approve").is_empty());
}

// --- compose ---

fn sha1_hex(bytes: &[u8]) -> String {
    sha1_smol::Sha1::from(bytes).digest().to_string()
}

/// Where a half-typed note for `key` on the doc at `canonical` is kept.
fn draft_path(site: &Site, canonical: &Path, key: &str) -> PathBuf {
    site.drafts_dir()
        .join(sha1_hex(canonical.as_os_str().as_encoded_bytes()))
        .join(format!("{}.json", sha1_hex(key.as_bytes())))
}

fn compose(path: &str, key: &str, text: &str) -> Value {
    json!({"type": "compose", "path": path, "key": key, "text": text})
}

#[tokio::test]
async fn compose_survives_a_restart_on_a_new_port_and_empty_text_deletes_it() {
    let site = Site::new();
    site.write(PR, DOC);
    let key = "block:6:1-6:12";
    let kept = draft_path(&site, &site.path(PR), key);
    {
        let served = site.serve(&[PR], &[GATE_ON]);
        let (mut ws, _) = served.viewing(PR).await;
        send(&mut ws, compose(PR, key, "half a thought")).await;
        eventually("the draft is on disk", || kept.exists()).await;
    }
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, frames) = served.viewing(PR).await;
    let restore: Vec<&Value> = frames
        .iter()
        .filter(|f| f["type"] == "compose_restore")
        .collect();
    assert_eq!(restore.len(), 1, "{frames:#?}");
    assert_eq!(restore[0]["path"], PR);
    assert_eq!(
        restore[0]["items"],
        json!([{"key": key, "text": "half a thought"}])
    );
    send(&mut ws, compose(PR, key, "")).await;
    eventually("empty text deletes the draft", || !kept.exists()).await;
    assert!(
        !review_file(&site).exists(),
        "drafts never touch the review file"
    );
}

#[tokio::test]
async fn compose_outside_the_root_writes_nothing() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    for path in ["../../etc/x.md", "/etc/x.md", "missing.md", "notes.txt"] {
        send(&mut ws, compose(path, "k", "escaped")).await;
    }
    send(&mut ws, compose(PR, "k", "kept")).await;
    let kept = draft_path(&site, &site.path(PR), "k");
    eventually("the valid draft is on disk", || kept.exists()).await;
    assert_eq!(site.drafts(), [kept]);
}

#[tokio::test]
async fn compose_is_ignored_with_the_gate_off_and_in_stdin_mode() {
    let site = Site::new();
    site.write(PR, DOC);
    for (args, path, envs) in [(vec![PR], PR, vec![]), (vec!["-"], "", vec![GATE_ON])] {
        let served = site.serve(&args, &envs);
        let (mut ws, _) = served.viewing(path).await;
        exchange(&mut ws, compose(path, "k", "never kept")).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(site.drafts(), Vec::<PathBuf>::new(), "{args:?}");
    }
}

// --- the hold ---

/// Reads states until one is built at or after `not_before`, checking every earlier
/// one still shows the hold of line `n`. Returns how many came earlier, and the one after.
async fn state_at_expiry(ws: &mut support::served::Ws, n: u64, not_before: u64) -> (usize, Value) {
    let mut before = 0;
    loop {
        let (_, frame) = until(ws, "a review_state", |f| f["type"] == "review_state").await;
        let state = &frame["state"];
        // States built before the op's own line don't know of its hold
        if state["log"]["lines"].as_u64() < Some(n) {
            continue;
        }
        assert_eq!(state["hold"]["n"], n, "{state}");
        if state["server_now"].as_u64().unwrap() >= not_before {
            return (before, frame);
        }
        before += 1;
    }
}

#[tokio::test]
async fn hand_back_holds_then_pushes_state_when_the_hold_passes() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON, HOLD_1500]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(DOC);
    acked(
        &mut ws,
        op(
            "c1",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "one"}),
        ),
    )
    .await;
    let ack = acked(&mut ws, op("c2", &r, "hand_back", json!({"scope": "doc"}))).await;
    let n = ack["n"].as_u64().unwrap();
    let submit = &log_lines(&review_file(&site))[n as usize - 1];
    assert_eq!(submit["op"], "submit");
    assert_eq!(submit["hold_ms"], 1500);
    let not_before = submit["not_before"].as_u64().unwrap();
    assert_eq!(not_before - submit["ts"].as_u64().unwrap(), 1500);
    let (before, expired) = state_at_expiry(&mut ws, n, not_before).await;
    assert!(
        before > 0,
        "the hand-back's own state comes inside the hold"
    );
    assert_eq!(expired["state"]["hold"]["kind"], "hand_back");
    assert!(
        now_ms() >= not_before,
        "the expiry push comes no earlier than the hold"
    );
}

#[tokio::test]
async fn a_hold_found_at_start_pushes_state_when_it_passes() {
    let site = Site::new();
    site.write(PR, DOC);
    let mut s = Scenario::new(DOC);
    s.note("one");
    let n = s.next();
    let ts = now_ms();
    let not_before = ts + 2_500;
    s.raw(json!({"n": n, "ts": ts, "by": human(), "op": "submit", "upto": n - 1, "not_before": not_before, "hold_ms": 2_500}));
    site.write(LOG, &s.log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, frames) = served.viewing(PR).await;
    let first = hello_state(&frames)["state"].clone();
    assert_eq!(first["hold"]["n"], n);
    assert!(
        first["server_now"].as_u64().unwrap() < not_before,
        "{first}"
    );
    state_at_expiry(&mut ws, u64::from(n), not_before).await;
}

#[tokio::test]
async fn withdraw_in_the_hold_turns_the_notes_back_into_drafts() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(DOC);
    for (cid, body) in [("c1", "one"), ("c2", "two")] {
        acked(
            &mut ws,
            op(
                cid,
                &r,
                "comment",
                json!({"at": {"k": "doc"}, "body": body}),
            ),
        )
        .await;
    }
    let n = acked(&mut ws, op("c3", &r, "hand_back", json!({"scope": "doc"}))).await["n"].clone();
    let ack = acked(&mut ws, op("c4", &r, "withdraw", json!({"to": n}))).await;
    let withdraws = ops_named(&review_file(&site), "withdraw");
    assert_eq!(withdraws.len(), 1);
    assert_eq!(withdraws[0]["to"], n);
    let (_, frame) = until(&mut ws, "a state with the drafts back", |f| {
        f["type"] == "review_state" && f["state"]["log"]["lines"].as_u64() >= ack["n"].as_u64()
    })
    .await;
    assert_eq!(frame["state"]["phrase"]["stage"], "drafts");
    assert_eq!(frame["state"]["counts"]["drafts"], 2);
    assert_eq!(
        frame["state"]["hold"],
        Value::Null,
        "a withdrawn round holds nothing"
    );
}

#[tokio::test]
async fn hand_back_with_nothing_new_or_inside_a_hold_is_refused() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write(LOG, &Scenario::new(DOC).log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(DOC);
    let hand_back = |cid: &str| op(cid, &r, "hand_back", json!({"scope": "doc"}));
    assert_eq!(nacked(&mut ws, hand_back("h1")).await, "nothing_to_send");
    acked(
        &mut ws,
        op(
            "c1",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "one"}),
        ),
    )
    .await;
    acked(&mut ws, hand_back("h2")).await;
    acked(
        &mut ws,
        op(
            "c2",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "two"}),
        ),
    )
    .await;
    assert_eq!(nacked(&mut ws, hand_back("h3")).await, "hold_pending");
    assert_eq!(ops_named(&review_file(&site), "submit").len(), 1);
}

// --- doc writes and the beacon ---

#[tokio::test]
async fn a_checkbox_touches_the_log_only_with_the_gate_on() {
    for gate in [false, true] {
        let site = Site::new();
        site.write(PR, TASKS);
        let log = Scenario::new(TASKS).log();
        site.write(LOG, &log);
        let envs = if gate { vec![GATE_ON] } else { vec![] };
        let served = site.serve(&[PR], &envs);
        let (mut ws, _) = served.viewing(PR).await;
        let frame = json!({"type": "checkbox", "cid": "x1", "path": PR, "line": 3, "checked": true, "rev": rev(TASKS)});
        acked(&mut ws, frame).await;
        let ticked = TASKS.replace("[ ]", "[x]");
        assert_eq!(
            site.read(PR).as_deref(),
            Some(ticked.as_str()),
            "gate {gate}"
        );
        // Give the watcher time to see the write too
        drain(&mut ws, Duration::from_millis(1_000)).await;
        let now = site.read(LOG).unwrap();
        if gate {
            assert_eq!(ops_named(&review_file(&site), "edit").len(), 1, "{now}");
        } else {
            assert_eq!(
                now, log,
                "with the gate off the review file is never touched"
            );
        }
    }
}

#[tokio::test]
async fn a_checkbox_on_a_doc_with_a_log_appends_edit_and_no_external() {
    let site = Site::new();
    site.write(PR, TASKS);
    site.write(LOG, &Scenario::new(TASKS).log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let frame = json!({"type": "checkbox", "cid": "x1", "path": PR, "line": 3, "checked": true, "rev": rev(TASKS)});
    let ack = acked(&mut ws, frame).await;
    let ticked = TASKS.replace("[ ]", "[x]");
    assert_eq!(ack["rev"], rev(&ticked));
    let log = review_file(&site);
    let edits = ops_named(&log, "edit");
    assert_eq!(edits.len(), 1, "{:#?}", log_lines(&log));
    assert_eq!(edits[0]["act"], "checkbox");
    assert_eq!(edits[0]["rev"], rev(TASKS));
    assert_eq!(edits[0]["to_rev"], rev(&ticked));
    // Give the watcher time to see the write it must not record again
    drain(&mut ws, Duration::from_millis(1_000)).await;
    assert!(
        ops_named(&log, "external").is_empty(),
        "{:#?}",
        log_lines(&log)
    );
}

/// Posts a beacon the way a closing page does.
async fn beacon(served: &Served, body: Value) -> u16 {
    let origin = format!("http://{}", served.host());
    served
        .request(
            "POST",
            "/review/flush",
            &[
                ("origin", origin.as_str()),
                ("content-type", "text/plain;charset=UTF-8"),
            ],
            Some(&body.to_string()),
        )
        .await
        .0
}

#[tokio::test]
async fn the_beacon_needs_the_token_and_never_hands_back() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let token = served.boot(PR).await["review"]["token"]
        .as_str()
        .expect("a token in BOOT.review")
        .to_string();
    let r = rev(DOC);
    let note = |cid: &str| {
        op(
            cid,
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "from a closing tab"}),
        )
    };
    let flush = |token: Option<&str>, ops: Vec<Value>| {
        let mut body = json!({"tab": "t1", "path": PR, "seen": [], "ops": ops});
        if let Some(token) = token {
            body["token"] = json!(token);
        }
        body
    };
    for (what, token) in [("no token", None), ("a wrong token", Some("not-the-token"))] {
        let status = beacon(&served, flush(token, vec![note("b0")])).await;
        assert!(!(200..300).contains(&status), "{what}: {status}");
    }
    assert!(
        !review_file(&site).exists(),
        "a refused beacon writes nothing"
    );
    let status = beacon(&served, flush(Some(&token), vec![note("b1")])).await;
    assert!((200..300).contains(&status), "{status}");
    let log = review_file(&site);
    eventually("the beacon's note is kept", || {
        log_lines(&log).iter().any(|l| l["cid"] == "b1")
    })
    .await;
    beacon(
        &served,
        flush(
            Some(&token),
            vec![
                op("b2", &r, "hand_back", json!({"scope": "doc"})),
                op("b3", &r, "approve", json!({})),
            ],
        ),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(ops_named(&log, "submit").is_empty());
    assert!(ops_named(&log, "approve").is_empty());
}

// --- the gate ---

#[tokio::test]
async fn with_the_gate_off_ops_are_disabled_and_nothing_is_created() {
    for gate in [false, true] {
        let site = Site::new();
        site.write(PR, DOC);
        let envs = if gate { vec![GATE_ON] } else { vec![] };
        let served = site.serve(&[PR], &envs);
        let (mut ws, _) = served.viewing(PR).await;
        let r = rev(DOC);
        let note = op(
            "c1",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "hi"}),
        );
        let resolve =
            json!({"type": "resolve", "cid": "c2", "path": PR, "rev": r, "text": "detail two"});
        if gate {
            acked(&mut ws, note).await;
            assert!(
                review_file(&site).exists(),
                "the gate-on twin creates the file"
            );
        } else {
            assert_eq!(nacked(&mut ws, note).await, "disabled");
            assert_eq!(nacked(&mut ws, resolve).await, "disabled");
            assert!(!review_file(&site).exists());
        }
    }
}

/// The `src` or `href` value of the tag holding `needle`, with the tag itself.
fn tag_with<'a>(page: &'a str, needle: &str) -> Option<&'a str> {
    let at = page.find(needle)?;
    let open = page[..at].rfind('<')?;
    let close = at + page[at..].find('>')?;
    Some(&page[open..=close])
}

fn page_nonce(response: &str) -> String {
    let csp = header(response, "content-security-policy").expect("a page CSP");
    let script = csp
        .split(';')
        .map(str::trim)
        .find(|d| d.starts_with("script-src"))
        .expect("script-src");
    assert!(script.contains("'strict-dynamic'"), "{script}");
    script
        .split_whitespace()
        .find_map(|s| s.strip_prefix("'nonce-")?.strip_suffix('\''))
        .expect("a nonce source")
        .to_string()
}

#[tokio::test]
async fn with_the_gate_off_the_page_has_no_review_tags_and_boot_says_so() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[]);
    let (status, page) = served.get(&format!("/view/{PR}")).await;
    assert_eq!(status, 200);
    assert!(!page.contains("/_birta/review.js"), "no review script");
    assert!(!page.contains("/_birta/review.css"), "no review css");
    assert_eq!(
        support::served::boot_of(&page)["review"],
        json!({"enabled": false})
    );
    for asset in ["/_birta/review.js", "/_birta/review.css"] {
        assert_eq!(served.get(asset).await.0, 404, "{asset}");
    }
}

#[tokio::test]
async fn review_js_serves_quote_js_under_the_page_csp() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (status, page) = served.get(&format!("/view/{PR}")).await;
    assert_eq!(status, 200);
    let nonce = page_nonce(&page);
    let script = tag_with(&page, "src=\"/_birta/review.js\"").expect("a review script tag");
    assert!(script.starts_with("<script"), "{script}");
    assert!(script.contains(&format!("nonce=\"{nonce}\"")), "{script}");
    assert!(script.contains(" defer"), "{script}");
    let link = tag_with(&page, "href=\"/_birta/review.css\"").expect("a review css link");
    assert!(
        link.starts_with("<link") && link.contains("stylesheet"),
        "{link}"
    );
    let boot = support::served::boot_of(body_of(&page));
    assert_eq!(boot["review"]["enabled"], true);
    let token = boot["review"]["token"].as_str().unwrap();
    assert!(token.len() >= 16, "{token}");
    let other = site.serve(&[PR], &[GATE_ON]);
    assert_ne!(
        other.boot(PR).await["review"]["token"],
        token,
        "a token per server"
    );

    let (status, js) = served.get("/_birta/review.js").await;
    assert_eq!(status, 200, "{js}");
    assert!(
        header(&js, "content-type").is_some_and(|t| t.contains("javascript")),
        "{js}"
    );
    assert_eq!(header(&js, "x-content-type-options"), Some("nosniff"));
    let quote = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/review/quote.js"),
    )
    .unwrap();
    let body = body_of(&js);
    assert!(body.starts_with(&quote), "review.js opens with quote.js");
    assert!(
        body[quote.len()..].contains("BR.shadowCss = "),
        "the shadow CSS rides along"
    );
    let (status, css) = served.get("/_birta/review.css").await;
    assert_eq!(status, 200);
    assert!(header(&css, "content-type").is_some_and(|t| t.starts_with("text/css")));
}

// --- the drafter ---

#[tokio::test]
async fn drafted_by_names_the_drafter_and_without_it_the_doc_is_yours() {
    type Case<'a> = (&'a [&'a str], &'a [(&'a str, &'a str)], Value);
    let cases: [Case; 2] = [
        (&["--drafted-by", "claude", PR], &[GATE_ON], json!("claude")),
        (
            &[PR],
            &[GATE_ON, ("CLAUDECODE", "1"), ("AI_AGENT", "claude")],
            Value::Null,
        ),
    ];
    for (args, envs, want) in cases {
        let site = Site::new();
        site.write(PR, DOC);
        let served = site.serve(args, envs);
        let (mut ws, _) = served.viewing(PR).await;
        acked(
            &mut ws,
            op(
                "c1",
                &rev(DOC),
                "comment",
                json!({"at": {"k": "doc"}, "body": "first"}),
            ),
        )
        .await;
        let lines = log_lines(&review_file(&site));
        assert_eq!(lines[2]["op"], "owner", "{lines:#?}");
        assert_eq!(lines[2]["drafted_by"], want, "{args:?}");
    }
}

// --- writability ---

struct Mode(PathBuf, u32);

impl Drop for Mode {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
    }
}

fn chmod(path: &Path, mode: u32) -> Mode {
    let old = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    Mode(path.to_path_buf(), old)
}

#[tokio::test]
async fn a_read_only_doc_is_notes_only() {
    let site = Site::new();
    site.write(PR, DOC);
    let _mode = chmod(&site.path(PR), 0o444);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let ack = acked(
        &mut ws,
        op(
            "c1",
            &rev(DOC),
            "comment",
            json!({"at": {"k": "doc"}, "body": "a note"}),
        ),
    )
    .await;
    let (_, frame) = until(&mut ws, "a state for the note", |f| {
        f["type"] == "review_state" && f["state"]["log"]["lines"].as_u64() >= ack["n"].as_u64()
    })
    .await;
    assert_eq!(frame["state"]["counts"]["drafts"], 1);
    assert_eq!(frame["state"]["writable"], false);
    assert_eq!(frame["state"]["log_writable"], true);
}

#[tokio::test]
async fn a_folder_birta_cannot_write_reports_log_writable_false() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write(LOG, &Scenario::new(DOC).log());
    let _mode = chmod(&site.root, 0o555);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (_ws, frames) = served.viewing(PR).await;
    let state = &hello_state(&frames)["state"];
    assert_eq!(state["log_writable"], false, "{state}");
    assert_eq!(state["writable"], true, "{state}");
}

// --- the watcher ---

#[tokio::test]
async fn an_outside_write_to_a_doc_with_a_log_is_recorded_then_pushed() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write(LOG, &Scenario::new(DOC).log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let changed = format!("{DOC}\nWritten by an editor.\n");
    site.write(PR, &changed);
    let (_, frame) = until(&mut ws, "a state on the new text", |f| {
        f["type"] == "review_state" && f["state"]["rev"] == rev(&changed)
    })
    .await;
    assert_eq!(frame["rev"], rev(&changed));
    let externals = ops_named(&review_file(&site), "external");
    assert_eq!(externals.len(), 1);
    assert_eq!(externals[0]["to_rev"], rev(&changed));
}

#[tokio::test]
async fn a_review_file_written_by_the_agent_pushes_state() {
    let site = Site::new();
    site.write(PR, DOC);
    let mut s = Scenario::new(DOC);
    let note = s.note("Tighten this.");
    s.submit();
    let turn = s.claim("claude");
    site.write(LOG, &s.log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, frames) = served.viewing(PR).await;
    let lines = hello_state(&frames)["state"]["log"]["lines"]
        .as_u64()
        .unwrap();
    s.done("claude", note, turn);
    site.write(LOG, &s.log());
    let (_, frame) = until(&mut ws, "a state with the agent's line", |f| {
        f["type"] == "review_state" && f["state"]["log"]["lines"].as_u64() > Some(lines)
    })
    .await;
    assert_eq!(frame["path"], PR);
}

// --- rev checks, the beacon, holds, nack reasons, writability ---

/// Skips a test whose refusal depends on file modes, which root ignores.
fn as_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail
    unsafe { libc::geteuid() == 0 }
}

#[tokio::test]
async fn ops_outside_the_checked_four_ignore_the_rev() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let r0 = rev(DOC);
    let doc_note = json!({"at": {"k": "doc"}, "body": "one"});
    let note = acked(&mut ws, op("c1", &r0, "comment", doc_note)).await["n"].clone();
    site.write(PR, &format!("{DOC}\nMoved on.\n"));
    let never = rev("a text no log ever saw\n");
    let block = json!({"at": {"k": "block", "sp": stop_sp(DOC, "second item"), "scope": "block"}, "body": "x"});
    // The control: an anchored note on the old rev is refused
    assert_eq!(
        nacked(&mut ws, op("c2", &r0, "comment", block)).await,
        "changed_since"
    );
    // A note on the whole doc reads no text
    acked(
        &mut ws,
        op(
            "c2b",
            &never,
            "comment",
            json!({"at": {"k": "doc"}, "body": "two"}),
        ),
    )
    .await;
    acked(
        &mut ws,
        op(
            "c3",
            &r0,
            "amend",
            json!({"to": note, "body": "one, amended"}),
        ),
    )
    .await;
    acked(
        &mut ws,
        op("c4", &never, "owner", json!({"drafted_by": null})),
    )
    .await;
    let submit =
        acked(&mut ws, op("c5", &r0, "hand_back", json!({"scope": "doc"}))).await["n"].clone();
    acked(&mut ws, op("c6", &never, "withdraw", json!({"to": submit}))).await;
    acked(&mut ws, op("c8", &r0, "retract", json!({"to": note}))).await;
}

#[tokio::test]
async fn with_no_log_hello_sends_a_virtual_state_and_creates_nothing() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&["--drafted-by", "claude", PR], &[GATE_ON]);
    let (_ws, frames) = served.viewing(PR).await;
    let state = &hello_state(&frames)["state"];
    assert_eq!(state["schema"], "review-state/1", "{state}");
    assert_eq!(state["rev"], rev(DOC));
    assert_eq!(state["log"]["lines"], 0);
    assert_eq!(state["owner"]["drafted_by"], "claude");
    assert_eq!(state["writable"], true);
    assert_eq!(state["log_writable"], true);
    assert_eq!(state["hold"], Value::Null);
    assert_eq!(
        state["counts"]["items"],
        Scenario::new(DOC).fold().counts.items
    );
    assert_eq!(
        state["hand_off"],
        format!(
            "Take a look at my notes: `birta review {}`",
            site.path(PR).display()
        )
    );
    let now = state["server_now"].as_u64().unwrap();
    assert!(now.abs_diff(now_ms()) < 60_000, "{now}");
    assert!(
        !review_file(&site).exists(),
        "a hello never creates the file"
    );
}

#[tokio::test]
async fn a_change_anchor_follows_an_outside_delete_to_the_doc() {
    let (s, n) = agent_changed(|s| s.replaced("- detail one", "- detail one, with retries"));
    let id = format!("{n}.0");
    let site = Site::new();
    site.write(PR, s.text());
    site.write(LOG, &s.log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let ack = acked(
        &mut ws,
        op(
            "c1",
            &rev(s.text()),
            "comment",
            json!({"at": {"k": "change", "id": id}, "body": "why"}),
        ),
    )
    .await;
    let at = log_lines(&review_file(&site))[ack["n"].as_u64().unwrap() as usize - 1]["at"].clone();
    let line = u64::from(line_of(s.text(), "with retries"));
    assert_eq!(at["scope"], "block", "{at}");
    assert!(
        at["lines"][0].as_u64() <= Some(line) && Some(line) <= at["lines"][1].as_u64(),
        "{at}"
    );
    // An outside write deletes the changed line; the page still holds the id
    let gone = s.text().replace("- detail one, with retries\n", "");
    site.write(PR, &gone);
    let ack = acked(
        &mut ws,
        op(
            "c2",
            &rev(&gone),
            "comment",
            json!({"at": {"k": "change", "id": id}, "body": "and now?"}),
        ),
    )
    .await;
    let at = &log_lines(&review_file(&site))[ack["n"].as_u64().unwrap() as usize - 1]["at"];
    assert_eq!(*at, json!("doc"), "no nearest-block fallback: {at}");
}

#[tokio::test]
async fn an_op_right_after_an_outside_write_lands_after_the_write() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write(LOG, &Scenario::new(DOC).log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let log = review_file(&site);
    let external_to = |text: &str| {
        let at: Vec<u64> = log_lines(&log)
            .iter()
            .enumerate()
            .filter(|(_, l)| l["op"] == "external" && l["to_rev"] == rev(text))
            .map(|(i, _)| i as u64 + 1)
            .collect();
        assert_eq!(at.len(), 1, "{:#?}", log_lines(&log));
        at[0]
    };
    // Sent well inside the watcher's 200 ms debounce, so only the handler's sync can land it first
    let one = format!("{DOC}\nOne.\n");
    site.write(PR, &one);
    let note = acked(
        &mut ws,
        op(
            "c1",
            &rev(&one),
            "comment",
            json!({"at": {"k": "doc"}, "body": "x"}),
        ),
    )
    .await;
    let two = format!("{one}Two.\n");
    site.write(PR, &two);
    let amend = acked(
        &mut ws,
        op(
            "c2",
            &rev(DOC),
            "amend",
            json!({"to": note["n"], "body": "y"}),
        ),
    )
    .await;
    drain(&mut ws, Duration::from_millis(600)).await;
    assert!(external_to(&one) < note["n"].as_u64().unwrap());
    assert!(external_to(&two) < amend["n"].as_u64().unwrap());
}

#[tokio::test]
async fn a_replayed_cid_is_acked_again_after_a_restart() {
    let site = Site::new();
    site.write(PR, DOC);
    let frame = op(
        "dup-1",
        &rev(DOC),
        "comment",
        json!({"at": {"k": "doc"}, "body": "Once."}),
    );
    let first = {
        let served = site.serve(&[PR], &[GATE_ON]);
        let (mut ws, _) = served.viewing(PR).await;
        let first = acked(&mut ws, frame.clone()).await;
        acked(
            &mut ws,
            op(
                "other",
                &rev(DOC),
                "comment",
                json!({"at": {"k": "doc"}, "body": "Later."}),
            ),
        )
        .await;
        first
    };
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    assert_eq!(
        acked(&mut ws, frame).await["n"],
        first["n"],
        "the original line, not the last"
    );
    let written = log_lines(&review_file(&site))
        .into_iter()
        .filter(|l| l["cid"] == "dup-1")
        .count();
    assert_eq!(written, 1);
}

#[tokio::test]
async fn a_replayed_cid_is_acked_even_after_the_doc_moved() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let block = json!({"at": {"k": "block", "sp": stop_sp(DOC, "second item"), "scope": "block"}, "body": "Kept."});
    let frame = op("dup-2", &rev(DOC), "comment", block);
    let (mut ws, _) = served.viewing(PR).await;
    let first = acked(&mut ws, frame.clone()).await;
    site.write(PR, &format!("{DOC}\nMoved.\n"));
    // The note was kept; a changed_since here would show the outbox a failure
    assert_eq!(acked(&mut ws, frame).await["n"], first["n"]);
}

#[tokio::test]
async fn the_beacon_takes_only_seen_and_draft_ops() {
    let (s, n) = agent_changed(|s| s.replaced("- detail one", "- detail one, with retries"));
    let site = Site::new();
    site.write(PR, s.text());
    site.write(LOG, &s.log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let token = served.boot(PR).await["review"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    let r = rev(s.text());
    let (mut ws, _) = served.viewing(PR).await;
    let note = op(
        "w1",
        &r,
        "comment",
        json!({"at": {"k": "doc"}, "body": "acked over the socket"}),
    );
    acked(&mut ws, note.clone()).await;
    let before = log_lines(&review_file(&site)).len();
    let body = json!({"token": token, "tab": "t1", "path": PR, "seen": [], "ops": [
        op("b1", &r, "undo_change", json!({"change": format!("{n}.0")})),
        op("b2", &r, "owner", json!({"drafted_by": "claude"})),
        op("b3", &r, "release", json!({"claim": 6})),
        op("b4", &r, "withdraw", json!({"to": 5})),
    ]});
    beacon(&served, body).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        site.read(PR).as_deref(),
        Some(s.text()),
        "a beacon never writes the doc"
    );
    assert_eq!(log_lines(&review_file(&site)).len(), before);
    let body =
        json!({"token": token, "tab": "t1", "path": PR, "seen": [format!("{n}.0")], "ops": [note]});
    assert!((200..300).contains(&beacon(&served, body).await));
    let log = review_file(&site);
    eventually("the beacon's seen is kept", || {
        !ops_named(&log, "seen").is_empty()
    })
    .await;
    assert_eq!(ops_named(&log, "seen")[0]["ids"], json!([format!("{n}.0")]));
    assert_eq!(
        log_lines(&log).iter().filter(|l| l["cid"] == "w1").count(),
        1,
        "one line per cid across socket and beacon"
    );
}

#[tokio::test]
async fn the_beacon_never_ticks_a_checkbox() {
    let site = Site::new();
    site.write(PR, TASKS);
    site.write(LOG, &Scenario::new(TASKS).log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let token = served.boot(PR).await["review"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    let tick = json!({"type": "checkbox", "cid": "x1", "path": PR, "line": 3, "checked": true, "rev": rev(TASKS)});
    let body = json!({"token": token, "tab": "t1", "path": PR, "seen": [], "ops": [tick]});
    beacon(&served, body).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(site.read(PR).as_deref(), Some(TASKS));
    assert!(ops_named(&review_file(&site), "edit").is_empty());
}

#[tokio::test]
async fn the_beacon_is_guarded_and_gone_with_the_gate_off() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let token = served.boot(PR).await["review"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    let body = json!({"token": token, "tab": "t1", "path": PR, "seen": [],
        "ops": [op("b1", &rev(DOC), "comment", json!({"at": {"k": "doc"}, "body": "x"}))]})
    .to_string();
    for headers in [
        vec![("origin", "http://evil.example")],
        vec![
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-mode", "no-cors"),
        ],
    ] {
        let (status, _) = served
            .request("POST", "/review/flush", &headers, Some(&body))
            .await;
        assert!(!(200..300).contains(&status), "{headers:?}: {status}");
    }
    let off = site.serve(&[PR], &[]);
    let (status, _) = off.request("POST", "/review/flush", &[], Some(&body)).await;
    assert!(!(200..300).contains(&status), "gate off: {status}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!review_file(&site).exists());
}

#[tokio::test]
async fn the_beacon_refuses_another_servers_token() {
    let site = Site::new();
    site.write(PR, DOC);
    let a = site.serve(&[PR], &[GATE_ON]);
    let b = site.serve(&[PR], &[GATE_ON]);
    let foreign = b.boot(PR).await["review"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    let body = json!({"token": foreign, "tab": "t1", "path": PR, "seen": [],
        "ops": [op("b1", &rev(DOC), "comment", json!({"at": {"k": "doc"}, "body": "x"}))]});
    let status = beacon(&a, body).await;
    assert!(!(200..300).contains(&status), "{status}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!review_file(&site).exists());
}

#[tokio::test]
async fn a_note_is_the_humans_even_when_an_agent_started_the_server() {
    let site = Site::new();
    site.write(PR, DOC);
    let envs = [
        GATE_ON,
        ("CLAUDECODE", "1"),
        ("AI_AGENT", "claude"),
        ("BIRTA_AS", "claude"),
    ];
    let served = site.serve(&[PR], &envs);
    let (mut ws, _) = served.viewing(PR).await;
    let ack = acked(
        &mut ws,
        op(
            "c1",
            &rev(DOC),
            "comment",
            json!({"at": {"k": "doc"}, "body": "mine"}),
        ),
    )
    .await;
    let line = &log_lines(&review_file(&site))[ack["n"].as_u64().unwrap() as usize - 1];
    assert_eq!(line["by"]["type"], "human", "{line}");
}

#[tokio::test]
async fn stdin_mode_is_review_off_even_with_the_gate_on() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&["-"], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("").await;
    let types: Vec<&str> = frames
        .iter()
        .map(|f| f["type"].as_str().unwrap_or("?"))
        .collect();
    assert_eq!(types, ["content"], "{frames:#?}");
    let mut frame = op(
        "c1",
        &rev(DOC),
        "comment",
        json!({"at": {"k": "doc"}, "body": "x"}),
    );
    for path in ["", PR] {
        frame["path"] = json!(path);
        assert_eq!(nacked(&mut ws, frame.clone()).await, "disabled", "{path:?}");
        // The cwd is the served folder here, so a real doc name must not resolve either
        exchange(&mut ws, compose(path, "k", "never kept")).await;
    }
    let (status, page) = served.get("/").await;
    assert_eq!(status, 200);
    assert!(!page.contains("/_birta/review.js"));
    assert_eq!(
        support::served::boot_of(&page)["review"],
        json!({"enabled": false})
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(site.drafts(), Vec::<PathBuf>::new());
    assert!(!review_file(&site).exists());
}

#[tokio::test]
async fn a_refused_hello_gets_gone_then_dir_state_and_no_review_frames() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let mut ws = served.connect().await;
    let frames = support::served::hello(&mut ws, "missing.md").await;
    let types: Vec<&str> = frames
        .iter()
        .map(|f| f["type"].as_str().unwrap_or("?"))
        .collect();
    let at = types.iter().position(|&t| t == "gone");
    assert_eq!(
        at.map(|i| &types[i..(i + 2).min(types.len())]),
        Some(&["gone", "dir_state"][..]),
        "{frames:#?}"
    );
    for t in ["content", "review_state", "compose_restore"] {
        assert!(!types.contains(&t), "{frames:#?}");
    }
}

#[test]
fn static_output_has_no_review_tags_even_with_the_gate_on() {
    let site = Site::new();
    site.write(PR, DOC);
    let out = site.base.join("out");
    assert_cmd::Command::cargo_bin("birta")
        .unwrap()
        .current_dir(&site.root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", site.base.join("home"))
        .env("BIRTA_REVIEW", "1")
        .args(["--static", "--no-open", "--out"])
        .arg(&out)
        .arg(PR)
        .timeout(Duration::from_secs(20))
        .assert()
        .success();
    fn html(path: &Path, out: &mut Vec<PathBuf>) {
        if path.is_dir() {
            for e in std::fs::read_dir(path).unwrap().flatten() {
                html(&e.path(), out);
            }
        } else if path.extension().is_some_and(|e| e == "html") {
            out.push(path.to_path_buf());
        }
    }
    let mut pages = Vec::new();
    html(&out, &mut pages);
    assert!(!pages.is_empty());
    for page in pages {
        let text = std::fs::read_to_string(&page).unwrap();
        assert!(!text.contains("/_birta/review"), "{}", page.display());
        assert_eq!(
            support::served::boot_of(&text)["review"],
            json!({"enabled": false})
        );
    }
    assert!(!review_file(&site).exists());
}

#[tokio::test]
async fn an_op_on_a_path_outside_the_root_creates_no_log_anywhere() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write("notes.txt", "plain\n");
    std::fs::write(site.base.join("outside.md"), DOC).unwrap();
    std::os::unix::fs::symlink("../outside.md", site.path("evil.md")).unwrap();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let abs = site.base.join("outside.md").display().to_string();
    for path in ["../outside.md", abs.as_str(), "evil.md", LOG, "notes.txt"] {
        let mut frame = op(
            "c1",
            &rev(DOC),
            "comment",
            json!({"at": {"k": "doc"}, "body": "x"}),
        );
        frame["path"] = json!(path);
        frame["cid"] = json!(format!("p-{path}"));
        nacked(&mut ws, frame).await;
    }
    for stray in [
        site.base.join("outside.md.birta.jsonl"),
        site.path("evil.md.birta.jsonl"),
        site.path("pr.md.birta.jsonl.birta.jsonl"),
        site.path("notes.txt.birta.jsonl"),
        review_file(&site),
    ] {
        assert!(!stray.exists(), "{}", stray.display());
    }
}

#[tokio::test]
async fn nothing_but_a_kept_note_creates_the_log() {
    let site = Site::new();
    site.write(PR, TASKS);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let old = rev("# Tasks\n");
    let block = json!({"at": {"k": "block", "sp": stop_sp(TASKS, "ship it"), "scope": "block"}, "body": "x"});
    let reason = nacked(&mut ws, op("c1", &old, "comment", block)).await;
    assert_eq!(reason, "changed_since");
    let tick = json!({"type": "checkbox", "cid": "x1", "path": PR, "line": 3, "checked": true, "rev": rev(TASKS)});
    acked(&mut ws, tick).await;
    drain(&mut ws, Duration::from_millis(600)).await;
    assert!(
        !review_file(&site).exists(),
        "neither a refused op nor a checkbox creates the log"
    );
}

#[tokio::test]
async fn a_doc_without_a_log_gets_its_state_on_the_current_text() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, frames) = served.viewing(PR).await;
    let items = hello_state(&frames)["state"]["counts"]["items"]
        .as_u64()
        .unwrap();
    let changed = format!("{DOC}- one more item\n");
    site.write(PR, &changed);
    until(&mut ws, "content on the new text", |f| {
        f["type"] == "content" && f["rev"] == rev(&changed)
    })
    .await;
    let frames = support::served::hello(&mut ws, PR).await;
    let state = &hello_state(&frames)["state"];
    assert_eq!(state["rev"], rev(&changed), "{state}");
    assert_eq!(state["counts"]["items"], items + 1);
}

#[tokio::test]
async fn writable_is_read_again_for_each_state() {
    if as_root() {
        return;
    }
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, frames) = served.viewing(PR).await;
    assert_eq!(hello_state(&frames)["state"]["writable"], true);
    let _mode = chmod(&site.path(PR), 0o444);
    let ack = acked(
        &mut ws,
        op(
            "c1",
            &rev(DOC),
            "comment",
            json!({"at": {"k": "doc"}, "body": "x"}),
        ),
    )
    .await;
    let (_, frame) = until(&mut ws, "the note's state", |f| {
        f["type"] == "review_state" && f["state"]["log"]["lines"].as_u64() >= ack["n"].as_u64()
    })
    .await;
    assert_eq!(frame["state"]["writable"], false);
}

/// The first state built at or after `at`, allowing `wait` of silence before it.
async fn state_built_after(ws: &mut Ws, at: u64, wait: Duration) -> Value {
    tokio::time::timeout(wait, async {
        loop {
            let f = next_within(ws, wait)
                .await
                .expect("no state before the timeout");
            if f["type"] == "review_state" && f["state"]["server_now"].as_u64() >= Some(at) {
                return f;
            }
        }
    })
    .await
    .expect("no state at the hold's end")
}

#[tokio::test]
async fn a_hold_pushes_to_every_viewer_through_a_second_hello() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON, HOLD_1500]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(DOC);
    acked(
        &mut ws,
        op(
            "c1",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "one"}),
        ),
    )
    .await;
    let n = acked(&mut ws, op("c2", &r, "hand_back", json!({"scope": "doc"}))).await["n"]
        .as_u64()
        .unwrap();
    let not_before = log_lines(&review_file(&site))[n as usize - 1]["not_before"]
        .as_u64()
        .unwrap();
    let (mut late, _) = served.viewing(PR).await;
    support::served::hello(&mut ws, PR).await;
    for socket in [&mut ws, &mut late] {
        let (_, f) = state_at_expiry(socket, n, not_before).await;
        assert_eq!(f["state"]["hold"]["kind"], "hand_back");
    }
}

#[tokio::test]
async fn an_approve_holds_then_pushes_state_when_the_hold_passes() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON, HOLD_1500]);
    let (mut ws, _) = served.viewing(PR).await;
    let n = acked(&mut ws, op("a1", &rev(DOC), "approve", json!({}))).await["n"]
        .as_u64()
        .unwrap();
    let line = &log_lines(&review_file(&site))[n as usize - 1];
    assert_eq!(line["op"], "approve");
    assert_eq!(line["rev"], rev(DOC));
    let not_before = line["not_before"].as_u64().unwrap();
    assert_eq!(not_before - line["ts"].as_u64().unwrap(), 1500);
    let (_, f) = state_at_expiry(&mut ws, n, not_before).await;
    assert_eq!(f["state"]["hold"]["kind"], "approve");
}

#[tokio::test]
async fn a_hold_over_a_minute_ends_at_the_clamped_time() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON, ("BIRTA_REVIEW_HOLD_MS", "120000")]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(DOC);
    acked(
        &mut ws,
        op(
            "c1",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "one"}),
        ),
    )
    .await;
    let n = acked(&mut ws, op("c2", &r, "hand_back", json!({"scope": "doc"}))).await["n"]
        .as_u64()
        .unwrap();
    let ts = log_lines(&review_file(&site))[n as usize - 1]["ts"]
        .as_u64()
        .unwrap();
    let f = state_built_after(&mut ws, ts + 5_000, Duration::from_secs(9)).await;
    assert_eq!(f["state"]["hold"]["not_before"], ts + 5_000, "{f}");
}

#[tokio::test]
async fn the_hold_length_falls_back_to_5000_and_0_holds_nothing() {
    for (value, want) in [("abc", 5_000), ("-1", 5_000), ("0", 0)] {
        let site = Site::new();
        site.write(PR, DOC);
        let served = site.serve(&[PR], &[GATE_ON, ("BIRTA_REVIEW_HOLD_MS", value)]);
        let (mut ws, _) = served.viewing(PR).await;
        let r = rev(DOC);
        acked(
            &mut ws,
            op(
                "c1",
                &r,
                "comment",
                json!({"at": {"k": "doc"}, "body": "one"}),
            ),
        )
        .await;
        let ack = acked(&mut ws, op("c2", &r, "hand_back", json!({"scope": "doc"}))).await;
        let line = &log_lines(&review_file(&site))[ack["n"].as_u64().unwrap() as usize - 1];
        assert_eq!(line["hold_ms"], want, "{value}: {line}");
        if want == 0 {
            let (_, f) = until(&mut ws, "the hand-back's state", |f| {
                f["type"] == "review_state"
                    && f["state"]["log"]["lines"].as_u64() >= ack["n"].as_u64()
            })
            .await;
            assert_eq!(f["state"]["hold"], Value::Null, "{f}");
        }
    }
}

#[tokio::test]
async fn compose_keys_on_the_canonical_doc_and_refuses_links_out() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write("c.md", "# C\n");
    std::os::unix::fs::symlink(PR, site.path("alias.md")).unwrap();
    std::fs::write(site.base.join("outside.md"), DOC).unwrap();
    std::os::unix::fs::symlink("../outside.md", site.path("evil.md")).unwrap();
    let kept = draft_path(&site, &site.path(PR), "k");
    {
        let served = site.serve(&["."], &[GATE_ON]);
        let (mut ws, _) = served.viewing(PR).await;
        send(&mut ws, compose("evil.md", "k", "escaped")).await;
        send(&mut ws, compose("alias.md", "k", "via the alias")).await;
        eventually("the alias draft is on disk", || kept.exists()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(site.drafts(), std::slice::from_ref(&kept));
    }
    let served = site.serve(&["."], &[GATE_ON]);
    let (_ws, frames) = served.viewing(PR).await;
    let restore: Vec<&Value> = frames
        .iter()
        .filter(|f| f["type"] == "compose_restore")
        .collect();
    assert_eq!(restore.len(), 1, "{frames:#?}");
    assert_eq!(
        restore[0]["items"],
        json!([{"key": "k", "text": "via the alias"}])
    );
    let (_c, frames) = served.viewing("c.md").await;
    assert!(
        frames.iter().all(|f| f["type"] != "compose_restore"),
        "another doc's drafts: {frames:#?}"
    );
}

#[tokio::test]
async fn the_review_assets_are_the_embedded_files() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/review");
    let (_, js) = served.get("/_birta/review.js").await;
    let body = body_of(&js);
    let tail = &body[body.rfind("BR.shadowCss = ").expect("shadowCss") + "BR.shadowCss = ".len()..];
    let tail = tail.trim_end().trim_end_matches(';');
    let css: String = serde_json::from_str(tail).expect("the shadow CSS is a JSON string");
    assert_eq!(
        css,
        std::fs::read_to_string(assets.join("shadow.css")).unwrap()
    );
    let (_, page) = served.get("/_birta/review.css").await;
    assert_eq!(
        body_of(&page),
        std::fs::read_to_string(assets.join("page.css")).unwrap()
    );
}

#[tokio::test]
async fn a_block_on_a_stop_that_owns_no_text_is_accepted() {
    let site = Site::new();
    site.write(PR, KINDS);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let doc = Doc::new(KINDS);
    // A code block's stop is its own owner, so it can't tell stops from owners
    for kind in [StopKind::TableRow, StopKind::ListItem] {
        let stop = doc
            .blocks
            .stops
            .iter()
            .filter(|s| s.kind == kind)
            .find(|s| doc.blocks.owners.iter().all(|o| o.sp != s.sp))
            .unwrap_or_else(|| panic!("setup: no {kind:?} stop that is not an owner"));
        let at = json!({"k": "block", "sp": stop.sp, "scope": "block"});
        acked(
            &mut ws,
            op(
                &format!("{kind:?}"),
                &rev(KINDS),
                "comment",
                json!({"at": at, "body": "x"}),
            ),
        )
        .await;
    }
}

#[tokio::test]
async fn section_scope_is_only_for_headings() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    for needle in ["- second item", "| a | 1 |"] {
        let at = json!({"k": "block", "sp": stop_sp(DOC, needle), "scope": "section"});
        let reason = nacked(
            &mut ws,
            op(
                &format!("s-{needle}"),
                &rev(DOC),
                "comment",
                json!({"at": at, "body": "x"}),
            ),
        )
        .await;
        assert_eq!(reason, "bad_anchor", "{needle}");
    }
    let at = json!({"k": "block", "sp": stop_sp(DOC, "## Details"), "scope": "section"});
    acked(
        &mut ws,
        op(
            "s-heading",
            &rev(DOC),
            "comment",
            json!({"at": at, "body": "x"}),
        ),
    )
    .await;
}

#[tokio::test]
async fn a_quote_without_sp_is_placed_by_search_or_falls_back() {
    let text = "# Top\n\nsame word, same word\n\n## Other word\n\nlast para\n";
    let site = Site::new();
    site.write(PR, text);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let stored = |ack: &Value| {
        log_lines(&review_file(&site))[ack["n"].as_u64().unwrap() as usize - 1]["at"].clone()
    };
    let note = |cid: &str, q: &str| {
        op(
            cid,
            &rev(text),
            "comment",
            json!({"at": {"k": "quote", "q": q, "pre": "", "suf": ""}, "body": "x"}),
        )
    };
    // Twice under one stop: that stop, with the words kept
    let at = stored(&acked(&mut ws, note("q1", "same word")).await);
    assert_eq!(at["scope"], "block", "{at}");
    assert_eq!(at["lines"][0], 3, "{at}");
    assert_eq!(at["shown"], "same word", "{at}");
    // Under different stops, or nowhere: the doc
    assert_eq!(
        stored(&acked(&mut ws, note("q2", "word")).await),
        json!("doc")
    );
    assert_eq!(
        stored(&acked(&mut ws, note("q3", "not in the doc")).await),
        json!("doc")
    );
}

/// Human notes handed back three times: the first round claimed and yielded, the last
/// claimed and live. Returns the scenario and its lines.
fn claimed_rounds() -> (Scenario, [u32; 7]) {
    let mut s = Scenario::new(DOC);
    let note1 = s.note("a");
    let sub1 = s.submit();
    let claim1 = s.claim("claude");
    let blank = s.blank("claude", s.block("## Details"), "Which one?", claim1);
    s.yield_turn("claude", claim1);
    s.note("b");
    let sub2 = s.submit();
    s.note("c");
    let sub3 = s.submit();
    let claim3 = s.claim_round("claude", sub3);
    (s, [note1, sub1, claim1, blank, sub2, sub3, claim3])
}

#[tokio::test]
async fn every_reachable_nack_reason_is_sent() {
    let (s, [note1, sub1, claim1, blank, sub2, sub3, _]) = claimed_rounds();
    let site = Site::new();
    site.write(PR, DOC);
    site.write(LOG, &s.log());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(DOC);
    let cases = [
        (
            "amend a missing line",
            op("n1", &r, "amend", json!({"to": 99, "body": "x"})),
            "no_such_note",
        ),
        (
            "retract a hand-back",
            op("n2", &r, "retract", json!({"to": sub1})),
            "no_such_note",
        ),
        (
            "reply to a hand-back",
            op("n3", &r, "reply", json!({"to": sub1, "body": "x"})),
            "no_such_note",
        ),
        (
            "amend the agent's blank",
            op("n4", &r, "amend", json!({"to": blank, "body": "x"})),
            "not_editable",
        ),
        (
            "amend a picked-up note",
            op("n5", &r, "amend", json!({"to": note1, "body": "x"})),
            "not_editable",
        ),
        (
            "withdraw a claimed round",
            op("n6", &r, "withdraw", json!({"to": sub3})),
            "claimed",
        ),
        (
            "withdraw a round a later claim took",
            op("n7", &r, "withdraw", json!({"to": sub2})),
            "already_picked_up",
        ),
        (
            "release a yielded claim",
            op("n8", &r, "release", json!({"claim": claim1})),
            "claim_moved",
        ),
    ];
    let lines = log_lines(&review_file(&site)).len();
    let mut failures = Vec::new();
    for (what, frame, want) in cases {
        match reply(&mut ws, frame).await {
            Some(r) if r["type"] == "nack" && r["reason"] == want => {}
            other => failures.push(format!("{what}: want {want}, got {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(
        log_lines(&review_file(&site)).len(),
        lines,
        "nothing written"
    );
}

#[tokio::test]
async fn a_log_from_a_later_schema_is_read_only() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write(
        LOG,
        &Scenario::new(DOC).log().replacen("review/1", "review/2", 1),
    );
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let before = site.read(LOG);
    let note = op(
        "c1",
        &rev(DOC),
        "comment",
        json!({"at": {"k": "doc"}, "body": "x"}),
    );
    assert_eq!(nacked(&mut ws, note).await, "read_only");
    assert_eq!(site.read(LOG), before);
}

#[tokio::test]
async fn a_frame_that_does_not_parse_is_dropped_and_the_socket_lives_on() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(DOC);
    for bad in [
        json!({"type": "op", "cid": "b1", "path": PR, "rev": r, "op": "comment"}),
        json!({"type": "op", "cid": "b2", "path": PR, "rev": r, "op": "fly"}),
        json!({"type": "op", "cid": "b3", "path": PR, "rev": r, "op": "amend", "to": "four"}),
    ] {
        assert_eq!(reply(&mut ws, bad.clone()).await, None, "{bad}");
    }
    acked(
        &mut ws,
        op(
            "ok",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "still here"}),
        ),
    )
    .await;
}

#[tokio::test]
async fn doc_writes_on_a_read_only_doc_are_refused_and_leave_it_alone() {
    if as_root() {
        return;
    }
    let fixture = committed_fixtures()
        .into_iter()
        .find(|(n, _)| n == "undo_change")
        .unwrap()
        .1;
    let doc = fixture["doc"].as_str().unwrap();
    let site = Site::new();
    site.write(PR, doc);
    site.write(LOG, fixture["log"].as_str().unwrap());
    let _mode = chmod(&site.path(PR), 0o444);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    assert_eq!(nacked(&mut ws, fixture["frame"].clone()).await, "read_only");
    assert_eq!(site.read(PR).as_deref(), Some(doc));
    let tasks = Site::new();
    tasks.write(PR, TASKS);
    tasks.write(LOG, &Scenario::new(TASKS).log());
    let _mode = chmod(&tasks.path(PR), 0o444);
    let served = tasks.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let tick = json!({"type": "checkbox", "cid": "x1", "path": PR, "line": 3, "checked": true, "rev": rev(TASKS)});
    assert_eq!(nacked(&mut ws, tick).await, "read_only");
    assert_eq!(tasks.read(PR).as_deref(), Some(TASKS));
}

#[tokio::test]
async fn a_note_in_a_folder_birta_cannot_write_is_read_only_and_leaves_nothing() {
    if as_root() {
        return;
    }
    let site = Site::new();
    site.write(PR, DOC);
    let _mode = chmod(&site.root, 0o555);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let note = op(
        "c1",
        &rev(DOC),
        "comment",
        json!({"at": {"k": "doc"}, "body": "x"}),
    );
    assert_eq!(nacked(&mut ws, note).await, "read_only");
    let names: Vec<String> = std::fs::read_dir(&site.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [PR], "no review or temp file");
}

#[tokio::test]
async fn an_undo_restores_the_text_and_is_recorded_as_birtas_edit() {
    let fixture = committed_fixtures()
        .into_iter()
        .find(|(n, _)| n == "undo_change")
        .unwrap()
        .1;
    let site = Site::new();
    site.write(PR, fixture["doc"].as_str().unwrap());
    site.write(LOG, fixture["log"].as_str().unwrap());
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    acked(&mut ws, fixture["frame"].clone()).await;
    assert_eq!(site.read(PR).as_deref(), Some(DOC), "the change is undone");
    drain(&mut ws, Duration::from_millis(600)).await;
    let log = review_file(&site);
    let edits = ops_named(&log, "edit");
    assert_eq!(edits.len(), 1, "{:#?}", log_lines(&log));
    assert_eq!(edits[0]["act"], "undo");
    assert_eq!(edits[0]["of"], fixture["frame"]["change"]);
    assert_eq!(
        ops_named(&log, "external").len(),
        1,
        "only the agent's write, not the undo"
    );
}

#[tokio::test]
async fn an_amend_drops_a_verdict_but_never_sets_and_drops_one_at_once() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON]);
    let (mut ws, _) = served.viewing(PR).await;
    let r0 = rev(DOC);
    let block = json!({"k": "block", "sp": stop_sp(DOC, "first item"), "scope": "block"});
    let note = acked(
        &mut ws,
        op(
            "c1",
            &r0,
            "comment",
            json!({"at": block, "verdict": "no", "body": "drop it"}),
        ),
    )
    .await["n"]
        .clone();
    let before = log_lines(&review_file(&site)).len();
    assert_eq!(
        nacked(
            &mut ws,
            op(
                "c2",
                &r0,
                "amend",
                json!({"to": note, "verdict": "yes", "drop_verdict": true})
            )
        )
        .await,
        "bad_op"
    );
    assert_eq!(
        log_lines(&review_file(&site)).len(),
        before,
        "a refused op writes nothing"
    );
    acked(
        &mut ws,
        op(
            "c3",
            &r0,
            "amend",
            json!({"to": note, "drop_verdict": true}),
        ),
    )
    .await;
    let amends = ops_named(&review_file(&site), "amend");
    assert_eq!(amends.len(), 1);
    assert_eq!(amends[0]["drop_verdict"], json!(true));
    assert_eq!(amends[0].get("verdict"), None);
}
