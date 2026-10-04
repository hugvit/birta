//! One writer at a time: from a hand-back to its reply, review sockets see the doc as it
//! was handed back, notes land on that text, and nothing the page sends writes the doc.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use birta::rev::Rev;
use birta::review::doc::Doc;
use birta::review::log::{AppendCtx, Log};
use birta::review::ops::{By, ByKind, Op, Via};
use serde_json::{Value, json};

mod support {
    pub mod scenario;
    pub mod served;
}
use support::scenario::{DOC, Scenario};
use support::served::{
    self, GATE_ON, Served, Site, Ws, acked, drain, eventually, log_lines, nacked, next_within,
    ops_named, until,
};

const PR: &str = "pr.md";
const LOG: &str = "pr.md.birta.jsonl";
const HOLD_0: (&str, &str) = ("BIRTA_REVIEW_HOLD_MS", "0");
const HOLD_LONG: (&str, &str) = ("BIRTA_REVIEW_HOLD_MS", "10000");
/// Long enough for a frame the server would send to have arrived.
const QUIET: Duration = Duration::from_millis(800);

/// Words only Claude's write has.
const NEW_PARA: &str = "A new opening paragraph.";

/// Claude's write: a paragraph above the intro, and `- third item` cut.
fn claudes_text() -> String {
    DOC.replacen("# Plan\n\n", &format!("# Plan\n\n{NEW_PARA}\n\n"), 1)
        .replacen("- third item\n", "", 1)
}

fn rev(text: &str) -> String {
    Rev::of(text.as_bytes()).as_str().to_string()
}

/// An `op` frame on `path` at `rev`, with the op's own fields in `fields`.
fn op_on(path: &str, cid: &str, rev: &str, name: &str, fields: Value) -> Value {
    let mut frame = json!({"type": "op", "cid": cid, "path": path, "rev": rev, "op": name});
    for (k, v) in fields.as_object().unwrap() {
        frame[k] = v.clone();
    }
    frame
}

fn op(cid: &str, rev: &str, name: &str, fields: Value) -> Value {
    op_on(PR, cid, rev, name, fields)
}

/// The `sp` of the stop starting on the line of `text` that holds `needle`.
fn stop_sp(text: &str, needle: &str) -> String {
    let line = text
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is not in the doc")) as u32
        + 1;
    Doc::new(text)
        .blocks
        .stops
        .iter()
        .find(|s| s.lines.0 == line)
        .unwrap_or_else(|| panic!("no stop starts on line {line} ({needle:?})"))
        .sp
        .clone()
}

fn sps(text: &str) -> BTreeSet<String> {
    Doc::new(text)
        .blocks
        .stops
        .iter()
        .map(|s| s.sp.clone())
        .collect()
}

/// `birta args…` as an agent shell runs it in the served folder, with `stdin`.
fn agent(site: &Site, args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut child = Command::new(assert_cmd::cargo::cargo_bin("birta"))
        .args(args)
        .current_dir(&site.root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", site.base.join("home"))
        .env("XDG_STATE_HOME", site.base.join("state"))
        .env("TZ", "UTC")
        .env(GATE_ON.0, GATE_ON.1)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), stdin.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Appends `ops` as `claude`'s lines, under the review file's lock as the CLI writes.
fn append_agent(review: &Path, ops: Vec<Value>) -> Vec<u32> {
    let ops: Vec<Op> = ops
        .into_iter()
        .map(|op| serde_json::from_value(op).expect("a test op must parse"))
        .collect();
    let ctx = AppendCtx {
        now_ms: now_ms(),
        by: By {
            kind: ByKind::Agent,
            name: Some("claude".into()),
            via: Some(Via::Cli),
        },
        cid: None,
    };
    Log::open(review)
        .append(ctx, ops, |_, _| Ok(()))
        .expect("the agent's lines land")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The last line of the review file whose op is `name`.
fn last_named(site: &Site, review: &str, name: &str) -> Value {
    ops_named(&site.path(review), name)
        .pop()
        .unwrap_or_else(|| panic!("no {name} line in {review}"))
}

/// Where the hello's own `content` is: the content right before a `review_state`. A
/// socket takes every doc's writes until its hello is handled, so a push may come first.
fn hello_at(frames: &[Value]) -> usize {
    frames
        .windows(2)
        .position(|w| w[0]["type"] == "content" && w[1]["type"] == "review_state")
        .unwrap_or_else(|| panic!("no review_state right after a content: {frames:#?}"))
}

/// The hello's own `review_state`: the frame right after its `content`.
fn hello_state(frames: &[Value]) -> &Value {
    &frames[hello_at(frames) + 1]
}

fn hello_content(frames: &[Value]) -> &Value {
    &frames[hello_at(frames)]
}

/// What the reviewer would see of a state: anything an agent line could change.
fn seen_by_reviewer(state: &Value) -> Value {
    json!({
        "rev": state["rev"],
        "items": state["items"],
        "changes": state["changes"],
        "deletions": state["deletions"],
        "news": state["news"],
        "counts": state["counts"],
        "lost": state["lost"],
        "later": state["later"],
        "stage": state["phrase"]["stage"],
        "replied": state["phrase"]["replied"],
        "rounds": state["rounds"],
    })
}

/// A page's turn handed to `claude`, who has written [`claudes_text`] over the doc.
struct MidTurn {
    site: Site,
    served: Served,
    /// The socket that said hello before the hand-back.
    ws: Ws,
    note: u64,
    submit: u64,
    /// The state the page had just after the claim, before Claude wrote anything.
    picked: Value,
}

/// The page notes and hands back, `claude` picks it up and, unless `write` is false,
/// writes its text.
async fn mid_turn(write: bool) -> MidTurn {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON, HOLD_0]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(DOC);
    let note = acked(
        &mut ws,
        op(
            "c1",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "Tighten the details."}),
        ),
    )
    .await["n"]
        .as_u64()
        .unwrap();
    let submit = acked(&mut ws, op("c2", &r, "hand_back", json!({"scope": "doc"}))).await["n"]
        .as_u64()
        .unwrap();
    let (code, out, err) = agent(&site, &["review", PR, "--as", "claude"], "");
    assert_eq!(code, 0, "setup: the agent picks it up: {out}{err}");
    let claim = last_named(&site, LOG, "claim");
    assert_eq!(claim["round"], submit, "setup: {claim}");
    let (_fresh, frames) = served.viewing(PR).await;
    let picked = hello_state(&frames)["state"].clone();
    assert_eq!(
        picked["phrase"]["stage"], "picked_up",
        "setup: {}",
        picked["phrase"]
    );
    // What the page was sent before the claim is not what this test is about
    drain(&mut ws, Duration::from_millis(300)).await;
    if write {
        site.write(PR, &claudes_text());
        let to = rev(&claudes_text());
        eventually("Claude's write is recorded", || {
            ops_named(&site.path(LOG), "external")
                .iter()
                .any(|l| l["to_rev"] == to)
        })
        .await;
    }
    MidTurn {
        site,
        served,
        ws,
        note,
        submit,
        picked,
    }
}

/// `respond` for [`claudes_text`], citing the note; asserts it was recorded.
fn respond(t: &MidTurn) {
    let heredoc = format!(
        "#{n} done\n#{n} covers \"{NEW_PARA}\"\nunasked \"third item\": the list reads better without it\n",
        n = t.note
    );
    let (code, out, err) = agent(&t.site, &["respond", PR, "--as", "claude"], &heredoc);
    assert_eq!(code, 0, "setup: respond records the reply: {out}{err}");
    assert_eq!(
        ops_named(&t.site.path(LOG), "yield").len(),
        1,
        "setup: one yield"
    );
}

/// Fails when a content of `rev` reaches the socket within [`QUIET`].
async fn no_content_of(ws: &mut Ws, rev: &str, when: &str) {
    let frames = drain(ws, QUIET).await;
    assert!(
        !frames
            .iter()
            .any(|f| f["type"] == "content" && f["rev"] == rev),
        "Claude's text reached the page {when}: {frames:#?}"
    );
}

/// The first content of `rev` on the socket and the frames from it on, through the
/// first state after it.
async fn content_then_state(ws: &mut Ws, rev: &str) -> (Value, Value) {
    let (_, content) = until(ws, "the content of the reply's rev", |f| {
        f["type"] == "content" && f["rev"] == rev
    })
    .await;
    let next = next_within(ws, Duration::from_secs(5))
        .await
        .expect("a frame after the content");
    (content, next)
}

// --- the held view ---

#[tokio::test]
async fn mid_turn_a_hello_gets_the_doc_as_handed_back_and_the_state_names_the_hand_back() {
    let t = mid_turn(true).await;
    let (_ws, frames) = t.served.viewing(PR).await;
    let content = hello_content(&frames);
    assert_eq!(
        content["rev"],
        rev(DOC),
        "the handed-back rev, not Claude's"
    );
    for part in ["html", "source"] {
        let text = content[part].as_str().unwrap();
        assert!(
            !text.contains(NEW_PARA),
            "{part} shows Claude's write: {text}"
        );
        assert!(
            text.contains("third item"),
            "{part} lost a handed-back line"
        );
    }
    let stops: BTreeSet<String> = content["stops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["sp"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(stops, sps(DOC), "the stops are the handed-back text's");
    let state = &hello_state(&frames)["state"];
    assert_eq!(state["rev"], rev(DOC));
    assert_eq!(
        state["held"],
        json!({"rev": rev(DOC), "n": t.submit}),
        "{state}"
    );
    assert_eq!(state["changes"], json!([]), "no rows for Claude's write");
    assert_eq!(state["phrase"]["stage"], "picked_up");
    // The file's own bookkeeping is not held back
    let last = log_lines(&t.site.path(LOG)).len() as u64;
    assert_eq!(state["log"]["lines"], last, "{}", state["log"]);
}

#[tokio::test]
async fn before_a_hand_back_nothing_is_held() {
    let site = Site::new();
    site.write(PR, DOC);
    site.write(LOG, &Scenario::new(DOC).log());
    let served = site.serve(&[PR], &[GATE_ON, HOLD_0]);
    let (_ws, frames) = served.viewing(PR).await;
    assert_eq!(hello_state(&frames)["state"]["held"], Value::Null);
}

#[tokio::test]
async fn a_hand_back_on_a_doc_the_chain_can_rebuild_writes_no_snap() {
    let t = mid_turn(false).await;
    let snaps = ops_named(&t.site.path(LOG), "snap");
    assert_eq!(
        snaps.len(),
        1,
        "only the review file's baseline: {snaps:#?}"
    );
}

#[tokio::test]
async fn the_held_view_hides_claudes_write_and_every_line_of_its_turn() {
    let mut t = mid_turn(true).await;
    let claim = last_named(&t.site, LOG, "claim")["n"].as_u64().unwrap();
    let ask = json!({"op": "comment", "at": "doc", "verdict": "ask", "body": "Which details?", "turn": claim});
    append_agent(
        &t.site.path(LOG),
        vec![
            json!({"op": "done", "to": t.note, "turn": claim}),
            json!({"op": "covers", "to": t.note, "quote": NEW_PARA, "turn": claim}),
            json!({"op": "unasked", "quote": "third item", "body": "reads better", "turn": claim}),
            json!({"op": "reply", "to": t.note, "kind": "note", "body": "Tightened them."}),
            ask,
            json!({"op": "turn_note", "body": "I added an opening paragraph.", "turn": claim}),
        ],
    );
    let parsed = Log::open(&t.site.path(LOG)).read().unwrap().unwrap();
    assert!(
        parsed.bad.is_empty() && parsed.unknown.is_empty(),
        "setup: {:?} {:?}",
        parsed.bad,
        parsed.unknown
    );
    let want = seen_by_reviewer(&t.picked);
    let (_late, frames) = t.served.viewing(PR).await;
    assert_eq!(
        seen_by_reviewer(&hello_state(&frames)["state"]),
        want,
        "a hello mid-turn"
    );
    let pushed = drain(&mut t.ws, QUIET).await;
    for f in &pushed {
        match f["type"].as_str() {
            Some("content") => panic!("a content frame mid-turn: rev {}", f["rev"]),
            Some("review_state") => {
                assert_eq!(seen_by_reviewer(&f["state"]), want, "a push mid-turn")
            }
            _ => {}
        }
    }
}

// --- notes at the held rev ---

#[tokio::test]
async fn a_note_at_the_held_rev_is_accepted_and_moves_with_the_text_when_the_reply_lands() {
    let mut t = mid_turn(true).await;
    let r = rev(DOC);
    let ours = claudes_text();
    let on =
        |sp: &str| json!({"at": {"k": "block", "sp": sp, "scope": "block"}, "body": "mid-turn"});
    let kept = acked(
        &mut t.ws,
        op("m1", &r, "comment", on(&stop_sp(DOC, "detail one"))),
    )
    .await["n"]
        .as_u64()
        .unwrap();
    let cut = acked(
        &mut t.ws,
        op("m2", &r, "comment", on(&stop_sp(DOC, "third item"))),
    )
    .await["n"]
        .as_u64()
        .unwrap();
    let quoted = acked(
        &mut t.ws,
        op(
            "m3",
            &r,
            "comment",
            json!({"at": {"k": "quote", "q": "Intro paragraph here", "pre": "", "suf": ""}, "body": "quoted"}),
        ),
    )
    .await["n"]
        .as_u64()
        .unwrap();
    let lines = log_lines(&t.site.path(LOG));
    for n in [kept, cut, quoted] {
        let line = &lines[n as usize - 1];
        assert_eq!(
            line["at"]["rev"], r,
            "the anchor is at the held rev: {line}"
        );
    }
    // A block only Claude's text has is not on the page
    let only_claudes = stop_sp(&ours, NEW_PARA);
    assert!(!sps(DOC).contains(&only_claudes), "setup: {only_claudes}");
    assert_eq!(
        nacked(&mut t.ws, op("m4", &r, "comment", on(&only_claudes))).await,
        "bad_anchor"
    );
    respond(&t);
    let (_, released) = until(&mut t.ws, "the reply's state", |f| {
        f["type"] == "review_state" && f["state"]["held"].is_null() && f["rev"] == rev(&ours)
    })
    .await;
    let state = &released["state"];
    let item = |n: u64| {
        state["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["n"] == n)
            .unwrap_or_else(|| panic!("no item {n}: {state}"))
            .clone()
    };
    assert_eq!(
        item(kept)["at"]["loc"]["sp"],
        stop_sp(&ours, "detail one"),
        "the note follows its block"
    );
    assert!(
        state["lost"].as_array().unwrap().contains(&json!(cut)),
        "a note on a block Claude cut is lost: {}",
        state["lost"]
    );
    assert_eq!(
        item(quoted)["at"]["loc"]["sp"],
        stop_sp(&ours, "Intro paragraph here")
    );
}

// --- doc writes wait ---

/// A doc with a task, after a round where Claude reworded a line; then a note handed back
/// and not picked up. Returns the scenario and the change Claude made.
fn handed_back_after_a_change() -> (Scenario, String) {
    let text = "# Tasks\n\n- [ ] ship it\n\nSome words here.\n";
    let mut s = Scenario::new(text);
    s.note("Better words.");
    s.submit();
    let turn = s.claim("claude");
    let n = s.replace_line("Some words here.", "Some better words here.");
    s.yield_turn("claude", turn);
    let change = format!("{n}.0");
    let state = s.fold();
    let found = state.changes.iter().find(|c| c.id == change);
    assert!(
        found.is_some_and(|c| c.reversible.ok),
        "setup: {change} must be revertible: {found:?}"
    );
    s.note("Again.");
    (s, change)
}

fn checkbox(cid: &str, rev: &str) -> Value {
    json!({"type": "checkbox", "cid": cid, "path": PR, "line": 3, "checked": true, "rev": rev})
}

#[tokio::test]
async fn with_claude_an_undo_and_a_checkbox_get_held_and_write_nothing() {
    let (mut s, change) = handed_back_after_a_change();
    s.submit();
    let site = Site::new();
    site.write(PR, s.text());
    site.write(LOG, &s.log());
    let served = site.serve(&[PR], &[GATE_ON, HOLD_0]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(s.text());
    let before = site.read(LOG).unwrap();
    assert_eq!(
        nacked(
            &mut ws,
            op("u1", &r, "undo_change", json!({"change": change}))
        )
        .await,
        "held"
    );
    assert_eq!(nacked(&mut ws, checkbox("x1", &r)).await, "held");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(site.read(PR).unwrap(), s.text(), "the doc is untouched");
    assert_eq!(site.read(LOG).unwrap(), before, "nothing logged");
}

#[tokio::test]
async fn in_the_hand_back_hold_doc_writes_already_wait() {
    let (s, change) = handed_back_after_a_change();
    let site = Site::new();
    site.write(PR, s.text());
    site.write(LOG, &s.log());
    let served = site.serve(&[PR], &[GATE_ON, HOLD_LONG]);
    let (mut ws, _) = served.viewing(PR).await;
    let r = rev(s.text());
    let n = acked(&mut ws, op("h1", &r, "hand_back", json!({"scope": "doc"}))).await["n"].clone();
    let (_, state) = until(&mut ws, "the hand-back's state", |f| {
        f["type"] == "review_state" && f["state"]["hold"]["n"] == n
    })
    .await;
    let before = site.read(LOG).unwrap();
    assert_eq!(
        nacked(
            &mut ws,
            op("u1", &r, "undo_change", json!({"change": change}))
        )
        .await,
        "held"
    );
    assert_eq!(nacked(&mut ws, checkbox("x1", &r)).await, "held");
    let submit = &log_lines(&site.path(LOG))[n.as_u64().unwrap() as usize - 1];
    assert!(
        now_ms() < submit["not_before"].as_u64().unwrap(),
        "setup: still inside the hold"
    );
    assert_eq!(site.read(PR).unwrap(), s.text());
    assert_eq!(site.read(LOG).unwrap(), before);
    assert_eq!(state["state"]["held"], json!({"rev": r, "n": n}));
}

#[tokio::test]
async fn once_the_reply_lands_an_undo_works_again() {
    let mut t = mid_turn(true).await;
    respond(&t);
    let ours = claudes_text();
    let (_, released) = until(&mut t.ws, "the reply's state", |f| {
        f["type"] == "review_state" && f["state"]["held"].is_null() && f["rev"] == rev(&ours)
    })
    .await;
    let change = released["state"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["attr"] == "agent" && c["reversible"]["ok"] == true)
        .unwrap_or_else(|| panic!("no revertible change of Claude's: {released}"))["id"]
        .clone();
    let ack = acked(
        &mut t.ws,
        op("u1", &rev(&ours), "undo_change", json!({"change": change})),
    )
    .await;
    assert!(ack["rev"].is_string(), "{ack}");
}

// --- what ends it ---

#[tokio::test]
async fn the_reply_ends_it_with_claudes_text_rows_and_counts_together() {
    let mut t = mid_turn(true).await;
    no_content_of(&mut t.ws, &rev(&claudes_text()), "before the reply").await;
    respond(&t);
    let ours = claudes_text();
    let (content, state) = content_then_state(&mut t.ws, &rev(&ours)).await;
    assert!(content["html"].as_str().unwrap().contains(NEW_PARA));
    assert_eq!(state["type"], "review_state", "{state}");
    assert_eq!(state["state"]["held"], Value::Null);
    assert_eq!(state["state"]["rev"], rev(&ours));
    assert!(
        !state["state"]["changes"].as_array().unwrap().is_empty(),
        "Claude's rows come with its text"
    );
    assert_eq!(state["state"]["phrase"]["stage"], "replied");
}

#[tokio::test]
async fn a_release_ends_it_and_shows_the_doc_on_disk() {
    let mut t = mid_turn(true).await;
    no_content_of(&mut t.ws, &rev(&claudes_text()), "before the release").await;
    let claim = last_named(&t.site, LOG, "claim")["n"].clone();
    acked(
        &mut t.ws,
        op("r1", &rev(DOC), "release", json!({"claim": claim})),
    )
    .await;
    let (content, state) = content_then_state(&mut t.ws, &rev(&claudes_text())).await;
    assert!(content["html"].as_str().unwrap().contains(NEW_PARA));
    assert_eq!(state["type"], "review_state", "{state}");
    assert_eq!(state["state"]["held"], Value::Null);
    assert_eq!(state["state"]["rev"], rev(&claudes_text()));
}

#[tokio::test]
async fn a_withdraw_in_the_hold_ends_it_and_shows_a_write_made_meanwhile() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON, HOLD_LONG]);
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
    let n = acked(&mut ws, op("c2", &r, "hand_back", json!({"scope": "doc"}))).await["n"].clone();
    // An editor saves the doc inside the hold
    let edited = DOC.replace("detail two", "detail two, edited");
    site.write(PR, &edited);
    eventually("the editor's save is recorded", || {
        ops_named(&site.path(LOG), "external")
            .iter()
            .any(|l| l["to_rev"] == rev(&edited))
    })
    .await;
    let meanwhile = drain(&mut ws, QUIET).await;
    assert!(
        !meanwhile
            .iter()
            .any(|f| f["type"] == "content" && f["rev"] == rev(&edited)),
        "the save shows inside the hold: {meanwhile:#?}"
    );
    acked(&mut ws, op("w1", &r, "withdraw", json!({"to": n}))).await;
    let (content, state) = content_then_state(&mut ws, &rev(&edited)).await;
    assert!(content["html"].as_str().unwrap().contains("edited"));
    assert_eq!(state["type"], "review_state", "{state}");
    assert_eq!(state["state"]["held"], Value::Null);
    assert_eq!(state["state"]["rev"], rev(&edited));
}

// --- a rev-only chain ---

/// Plain paragraphs past the size the chain records by rev only.
fn big(seed: &str) -> String {
    let line = format!("{seed} {}\n\n", "word ".repeat(200));
    let mut text = String::from("# Big\n\n");
    while text.len() <= birta::review::chain::REV_ONLY_OVER + 4096 {
        text.push_str(&line);
    }
    text
}

#[tokio::test]
async fn a_hand_back_on_a_rev_only_chain_snaps_the_text_it_hands_back() {
    let site = Site::new();
    let first = big("first");
    site.write(PR, &first);
    site.write(LOG, &Scenario::new(&first).log());
    let served = site.serve(&[PR], &[GATE_ON, HOLD_0]);
    let (mut ws, _) = served.viewing(PR).await;
    // An editor's save, recorded by rev only: nothing in the log can rebuild its text
    let saved = format!("{first}Saved by an editor.\n");
    site.write(PR, &saved);
    let r = rev(&saved);
    eventually("the save is recorded", || {
        ops_named(&site.path(LOG), "external")
            .iter()
            .any(|l| l["to_rev"] == r)
    })
    .await;
    let external = last_named(&site, LOG, "external");
    assert!(external["patch"].is_null(), "setup: rev only: {external}");
    assert!(
        !ops_named(&site.path(LOG), "snap")
            .iter()
            .any(|l| l["rev"] == r),
        "setup: no snap of the save"
    );
    acked(
        &mut ws,
        op(
            "c1",
            &r,
            "comment",
            json!({"at": {"k": "doc"}, "body": "Shorter."}),
        ),
    )
    .await;
    let n = acked(&mut ws, op("c2", &r, "hand_back", json!({"scope": "doc"}))).await["n"]
        .as_u64()
        .unwrap();
    let lines = log_lines(&site.path(LOG));
    let before = &lines[n as usize - 2];
    assert_eq!(before["op"], "snap", "the line before the hand-back");
    assert_eq!(before["rev"], r);
    assert_eq!(before["text"], saved.as_str());
    // Claude's write can't hide the handed-back text now
    let (code, out, err) = agent(&site, &["review", PR, "--as", "claude"], "");
    assert_eq!(code, 0, "setup: {out}{err}");
    let ours = format!("{saved}Claude's closing line.\n");
    site.write(PR, &ours);
    eventually("Claude's write is recorded", || {
        ops_named(&site.path(LOG), "external")
            .iter()
            .any(|l| l["to_rev"] == rev(&ours))
    })
    .await;
    let (_late, frames) = served.viewing(PR).await;
    assert_eq!(hello_content(&frames)["rev"], r);
    assert_eq!(hello_state(&frames)["state"]["held"]["rev"], r);
}

// --- who it applies to ---

#[tokio::test]
async fn a_socket_that_never_said_hello_still_gets_every_write() {
    let site = Site::new();
    site.write(PR, DOC);
    let served = site.serve(&[PR], &[GATE_ON, HOLD_0]);
    let mut plain = served.connect().await;
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
    acked(&mut ws, op("c2", &r, "hand_back", json!({"scope": "doc"}))).await;
    let (code, out, err) = agent(&site, &["review", PR, "--as", "claude"], "");
    assert_eq!(code, 0, "setup: {out}{err}");
    site.write(PR, &claudes_text());
    let (_, content) = until(&mut plain, "Claude's write on the plain socket", |f| {
        f["type"] == "content" && f["rev"] == rev(&claudes_text())
    })
    .await;
    assert!(content["html"].as_str().unwrap().contains(NEW_PARA));
}

#[tokio::test]
async fn a_theme_change_mid_turn_carries_the_held_text() {
    let mut t = mid_turn(true).await;
    let theme = json!({"type": "theme_change", "theme": "dracula"});
    served::send(&mut t.ws, theme).await;
    let (_, update) = until(&mut t.ws, "the theme update with the doc", |f| {
        f["type"] == "theme_update" && !f["html"].is_null()
    })
    .await;
    assert_eq!(update["rev"], rev(DOC), "the handed-back rev");
    for part in ["html", "source"] {
        let text = update[part].as_str().unwrap();
        assert!(!text.contains(NEW_PARA), "{part} shows Claude's write");
    }
    let after = drain(&mut t.ws, QUIET).await;
    assert!(
        !after
            .iter()
            .any(|f| f["rev"] == rev(&claudes_text()) && f["type"] != "review_state"),
        "{after:#?}"
    );
}

#[tokio::test]
async fn two_hand_backs_hold_the_second_rounds_text_and_keep_the_first_rounds_rows() {
    let mut s = Scenario::new(DOC);
    let first = s.note("Tighten the intro.");
    s.submit();
    let turn = s.claim("claude");
    let one = s.replace_line("Intro paragraph here,", "Intro, tightened,");
    s.done("claude", first, turn);
    s.turn_note("claude", "I tightened the intro.", turn);
    s.yield_turn("claude", turn);
    let round_one = s.text().to_string();
    s.note("Now the details.");
    let second = s.submit();
    // Claude's second turn is still open: a write and no yield
    s.claim("claude");
    let two = s.replace_line("- detail two", "- detail two, rewritten");
    let site = Site::new();
    site.write(PR, s.text());
    site.write(LOG, &s.log());
    let served = site.serve(&[PR], &[GATE_ON, HOLD_0]);
    let (_ws, frames) = served.viewing(PR).await;
    let content = hello_content(&frames);
    assert_eq!(content["rev"], rev(&round_one), "round one's text");
    let state = &hello_state(&frames)["state"];
    assert_eq!(state["held"], json!({"rev": rev(&round_one), "n": second}));
    let ids: Vec<&str> = state["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&format!("{one}.0").as_str()),
        "round one's row stays: {ids:?}"
    );
    assert!(
        !ids.contains(&format!("{two}.0").as_str()),
        "round two's write is held back: {ids:?}"
    );
    let note = state["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["n"] == first)
        .unwrap();
    assert_eq!(
        note["outcome"]["done"], true,
        "round one's done stays: {note}"
    );
}

#[tokio::test]
async fn the_agents_own_review_and_respond_see_the_doc_on_disk_mid_turn() {
    let t = mid_turn(true).await;
    let (code, out, err) = agent(&t.site, &["review", PR, "--peek", "--json"], "");
    assert_eq!(code, 0, "{out}{err}");
    let view: Value = serde_json::from_str(out.trim()).unwrap_or_else(|_| panic!("{out}"));
    assert_eq!(view["items"][0]["status"], "picked_up", "{view:#}");
    // respond finds the write it made, so it asks for that change to be cited
    let before = t.site.read(LOG).unwrap();
    let (code, out, err) = agent(
        &t.site,
        &["respond", PR, "--as", "claude"],
        &format!("#{} done\n", t.note),
    );
    assert_eq!(code, 65, "{out}{err}");
    assert!(err.contains(NEW_PARA), "{err}");
    assert_eq!(t.site.read(LOG).unwrap(), before);
}

// --- a folder ---

const A_MD: &str = "# A\n\nSome text in a.\n";
const B_MD: &str = "# B\n\nSome text in b.\n";

/// A folder whose two docs hold a draft each, handed back as one batch and claimed.
async fn folder_handed_back() -> (Site, Served, Ws) {
    let site = Site::new();
    site.write("a.md", A_MD);
    site.write("b.md", B_MD);
    let mut sa = Scenario::new(A_MD);
    sa.note("Shorter a.");
    site.write("a.md.birta.jsonl", &sa.log());
    let mut sb = Scenario::new(B_MD);
    sb.note("Shorter b.");
    site.write("b.md.birta.jsonl", &sb.log());
    let served = site.serve(&["."], &[GATE_ON, HOLD_0]);
    let (mut ws, _) = served.viewing("a.md").await;
    acked(
        &mut ws,
        op_on(
            "a.md",
            "h1",
            &rev(A_MD),
            "hand_back",
            json!({"scope": "all"}),
        ),
    )
    .await;
    (site, served, ws)
}

#[tokio::test]
async fn in_a_folder_each_doc_of_the_batch_is_held_until_its_own_reply() {
    let (site, served, _ws) = folder_handed_back().await;
    let (code, out, err) = agent(&site, &["review", ".", "--as", "claude"], "");
    assert_eq!(code, 0, "setup: {out}{err}");
    let (a2, b2) = (
        A_MD.replace("Some text in a.", "Less in a."),
        B_MD.replace("Some text in b.", "Less in b."),
    );
    site.write("a.md", &a2);
    site.write("b.md", &b2);
    for (doc, text) in [("a.md", &a2), ("b.md", &b2)] {
        let review = format!("{doc}.birta.jsonl");
        eventually("Claude's write is recorded", || {
            ops_named(&site.path(&review), "external")
                .iter()
                .any(|l| l["to_rev"] == rev(text))
        })
        .await;
    }
    for (doc, held) in [("a.md", A_MD), ("b.md", B_MD)] {
        let (_ws, frames) = served.viewing(doc).await;
        assert_eq!(hello_content(&frames)["rev"], rev(held), "{doc}");
        assert_eq!(hello_state(&frames)["state"]["held"]["rev"], rev(held));
    }
    let (mut b_ws, _) = served.viewing("b.md").await;
    let (mut a_ws, _) = served.viewing("a.md").await;
    let note = log_lines(&site.path("a.md.birta.jsonl"))
        .iter()
        .position(|l| l["op"] == "comment")
        .expect("a.md's note")
        + 1;
    let (code, out, err) = agent(
        &site,
        &["respond", "a.md", "--as", "claude"],
        &format!("#{note} done\n#{note} covers \"Less in a.\"\n"),
    );
    assert_eq!(code, 0, "setup: {out}{err}");
    let (content, state) = content_then_state(&mut a_ws, &rev(&a2)).await;
    assert_eq!(content["path"], "a.md");
    assert_eq!(state["state"]["held"], Value::Null, "{state}");
    let b_frames = drain(&mut b_ws, QUIET).await;
    assert!(
        !b_frames
            .iter()
            .any(|f| f["type"] == "content" && f["rev"] == rev(&b2)),
        "b.md is still with Claude: {b_frames:#?}"
    );
    let (_ws, frames) = served.viewing("b.md").await;
    assert_eq!(hello_state(&frames)["state"]["held"]["rev"], rev(B_MD));
}

#[tokio::test]
async fn a_folder_hand_back_snaps_a_rev_only_doc_of_the_batch() {
    let first = big("b");
    let site = Site::new();
    site.write("a.md", A_MD);
    site.write("b.md", &first);
    let mut sa = Scenario::new(A_MD);
    sa.note("Shorter a.");
    site.write("a.md.birta.jsonl", &sa.log());
    site.write("b.md.birta.jsonl", &Scenario::new(&first).log());
    let served = site.serve(&["."], &[GATE_ON, HOLD_0]);
    let (mut ws, _) = served.viewing("a.md").await;
    let saved = format!("{first}Saved by an editor.\n");
    site.write("b.md", &saved);
    eventually("the save is recorded", || {
        ops_named(&site.path("b.md.birta.jsonl"), "external")
            .iter()
            .any(|l| l["to_rev"] == rev(&saved))
    })
    .await;
    acked(
        &mut ws,
        op_on(
            "b.md",
            "c1",
            &rev(&saved),
            "comment",
            json!({"at": {"k": "doc"}, "body": "Shorter b."}),
        ),
    )
    .await;
    acked(
        &mut ws,
        op_on(
            "a.md",
            "h1",
            &rev(A_MD),
            "hand_back",
            json!({"scope": "all"}),
        ),
    )
    .await;
    let lines = log_lines(&site.path("b.md.birta.jsonl"));
    let submit = lines
        .iter()
        .position(|l| l["op"] == "submit")
        .expect("b.md is in the batch");
    let before = &lines[submit - 1];
    assert_eq!(before["op"], "snap", "the line before b.md's hand-back");
    assert_eq!(before["rev"], rev(&saved));
    assert_eq!(before["text"], saved.as_str());
}

#[tokio::test]
async fn a_second_tab_opened_mid_turn_gets_claudes_text_with_the_reply() {
    let t = mid_turn(true).await;
    let (mut second, frames) = t.served.viewing(PR).await;
    assert_eq!(hello_content(&frames)["rev"], rev(DOC));
    respond(&t);
    let (content, state) = content_then_state(&mut second, &rev(&claudes_text())).await;
    assert!(content["html"].as_str().unwrap().contains(NEW_PARA));
    assert_eq!(state["type"], "review_state", "{state}");
    assert_eq!(state["state"]["held"], Value::Null);
}

// --- repairs ---

/// The listing's entry for `path` in a `dir_state`.
fn entry<'a>(dir: &'a Value, path: &str) -> &'a Value {
    dir["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["paths"].as_array().unwrap().iter().any(|p| p == path))
        .unwrap_or_else(|| panic!("{path} is not listed: {dir}"))
}

/// The listing a page opening now would get.
async fn listing(served: &Served) -> Value {
    let (_ws, frames) = served.viewing("a.md").await;
    frames
        .into_iter()
        .rfind(|f| f["type"] == "dir_state")
        .expect("a listing on hello")
}

#[tokio::test]
async fn a_folder_hand_back_never_sends_again_a_doc_claude_still_has() {
    let site = Site::new();
    site.write("a.md", A_MD);
    site.write("b.md", B_MD);
    let mut sa = Scenario::new(A_MD);
    sa.note("Shorter a.");
    site.write("a.md.birta.jsonl", &sa.log());
    site.write("b.md.birta.jsonl", &Scenario::new(B_MD).log());
    let served = site.serve(&["."], &[GATE_ON, HOLD_0]);
    let (mut ws, _) = served.viewing("a.md").await;
    let all = |cid: &str| {
        op_on(
            "a.md",
            cid,
            &rev(A_MD),
            "hand_back",
            json!({"scope": "all"}),
        )
    };
    acked(&mut ws, all("h1")).await;
    let (code, out, err) = agent(&site, &["review", "a.md", "--as", "claude"], "");
    assert_eq!(code, 0, "setup: {out}{err}");
    let a2 = A_MD.replace("Some text in a.", "Less in a.");
    site.write("a.md", &a2);
    eventually("Claude's write is recorded", || {
        ops_named(&site.path("a.md.birta.jsonl"), "external")
            .iter()
            .any(|l| l["to_rev"] == rev(&a2))
    })
    .await;
    // A note for the next hand-back on a.md, and one on b.md, then ⌘⏎ for the folder
    let note = |path: &str, cid: &str, text: &str| {
        op_on(
            path,
            cid,
            &rev(text),
            "comment",
            json!({"at": {"k": "doc"}, "body": "more"}),
        )
    };
    acked(&mut ws, note("a.md", "n1", A_MD)).await;
    acked(&mut ws, note("b.md", "n2", B_MD)).await;
    acked(&mut ws, all("h2")).await;
    let submits = |doc: &str| ops_named(&site.path(&format!("{doc}.birta.jsonl")), "submit");
    assert_eq!(submits("a.md").len(), 1, "a.md is still with Claude");
    assert_eq!(submits("b.md").len(), 1, "b.md goes");
    assert_eq!(
        nacked(
            &mut ws,
            op_on(
                "a.md",
                "h3",
                &rev(A_MD),
                "hand_back",
                json!({"scope": "doc"})
            )
        )
        .await,
        "held"
    );
    let (_late, frames) = served.viewing("a.md").await;
    assert_eq!(
        hello_content(&frames)["rev"],
        rev(A_MD),
        "still the handed-back text"
    );
    let first = ops_named(&site.path("a.md.birta.jsonl"), "comment").len();
    assert_eq!(first, 2, "setup: the first note and the one for later");
    let n = log_lines(&site.path("a.md.birta.jsonl"))
        .iter()
        .position(|l| l["op"] == "comment")
        .unwrap()
        + 1;
    let (code, out, err) = agent(
        &site,
        &["respond", "a.md", "--as", "claude"],
        &format!("#{n} done\n#{n} covers \"Less in a.\"\n"),
    );
    assert_eq!(code, 0, "setup: {out}{err}");
    let (_, released) = until(&mut ws, "a.md's reply", |f| {
        f["type"] == "review_state" && f["path"] == "a.md" && f["state"]["held"].is_null()
    })
    .await;
    assert_eq!(released["rev"], rev(&a2), "the hold ends with the reply");
}

#[tokio::test]
async fn the_folder_listing_counts_a_held_doc_as_its_page_shows_it() {
    let (site, served, _ws) = folder_handed_back().await;
    let (code, out, err) = agent(&site, &["review", "a.md", "--as", "claude"], "");
    assert_eq!(code, 0, "setup: {out}{err}");
    let a2 = A_MD.replace("Some text in a.", "Less in a.");
    site.write("a.md", &a2);
    eventually("Claude's write is recorded", || {
        ops_named(&site.path("a.md.birta.jsonl"), "external")
            .iter()
            .any(|l| l["to_rev"] == rev(&a2))
    })
    .await;
    tokio::time::sleep(QUIET).await;
    let dir = listing(&served).await;
    let a = entry(&dir, "a.md");
    assert_eq!(a["summary"]["stage"], "picked_up", "{a}");
    assert_eq!(
        a["summary"]["counts"]["to_check"], 0,
        "Claude's write is not news yet: {a}"
    );
    assert_eq!(a["news"], false, "{a}");
    let n = log_lines(&site.path("a.md.birta.jsonl"))
        .iter()
        .position(|l| l["op"] == "comment")
        .unwrap()
        + 1;
    let (code, out, err) = agent(
        &site,
        &["respond", "a.md", "--as", "claude"],
        &format!("#{n} done\n#{n} covers \"Less in a.\"\n"),
    );
    assert_eq!(code, 0, "setup: {out}{err}");
    tokio::time::sleep(QUIET).await;
    let dir = listing(&served).await;
    assert_eq!(
        entry(&dir, "a.md")["news"],
        true,
        "the reply brings it: {dir}"
    );
}

#[tokio::test]
async fn claudes_saves_while_held_send_no_state_the_page_already_has() {
    let mut t = mid_turn(false).await;
    t.site.write(PR, &claudes_text());
    eventually("Claude's write is recorded", || {
        ops_named(&t.site.path(LOG), "external")
            .iter()
            .any(|l| l["to_rev"] == rev(&claudes_text()))
    })
    .await;
    let frames = drain(&mut t.ws, QUIET).await;
    let states: Vec<&Value> = frames
        .iter()
        .filter(|f| f["type"] == "review_state")
        .collect();
    assert!(
        states.is_empty(),
        "the held view didn't change: {states:#?}"
    );
}
