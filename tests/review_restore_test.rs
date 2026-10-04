//! Restoring your own revert: `u` on a reverted change undoes the undo, which puts
//! Claude's text back, and `u` again reverts it again. The fold reads each change's state
//! by parity, names the change the next `u` sends (`flip`), and the agent's view stops
//! listing a revert you took back.

use std::path::Path;
use std::process::{Command, Stdio};

use birta::rev::Rev;
use birta::review::doc::Doc;
use birta::review::state::ReviewState;
use serde_json::{Value, json};

mod support {
    pub mod scenario;
    pub mod served;
}
use support::scenario::{DOC, Scenario};
use support::served::{GATE_ON, Served, Site, acked, log_lines, nacked, ops_named};

const PR: &str = "pr.md";
const LOG: &str = "pr.md.birta.jsonl";
const CLAUDE: &str = "claude";
const HOLD_0: (&str, &str) = ("BIRTA_REVIEW_HOLD_MS", "0");
/// The journey's draft, and the text Claude's first write leaves.
const PR_DRAFT: &str = include_str!("../e2e/fixtures/review/journey/pr-draft.md");
const ROUND1: &str = include_str!("../e2e/fixtures/review/journey/round1.md");

fn rev(text: &str) -> String {
    Rev::of(text.as_bytes()).as_str().to_string()
}

/// The state as the page receives it, so new fields are checked by their wire names.
fn wire(state: &ReviewState) -> Value {
    serde_json::to_value(state).unwrap()
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

fn change<'v>(v: &'v Value, id: &str) -> &'v Value {
    v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no change {id} in {}", v["changes"]))
}

fn news_of<'v>(v: &'v Value, id: &str) -> Option<&'v Value> {
    v["news"].as_array().unwrap().iter().find(|n| n["id"] == id)
}

/// Ids of your own writes the fold still lists as changes.
fn yours(v: &Value) -> Vec<String> {
    v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["attr"] == "human")
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

/// A round in which Claude rewords `- detail one`, and its reply lands. The scenario, the
/// change, the text before it and Claude's text.
fn replied() -> (Scenario, String, String, String) {
    replied_on(DOC, |s| {
        s.replace("- detail one\n", "- detail one, reworded\n")
    })
}

fn replied_on(text: &str, write: impl Fn(&str) -> String) -> (Scenario, String, String, String) {
    let mut s = Scenario::drafted_by(text, CLAUDE);
    let note = s.note("Reword the details.");
    s.submit();
    let turn = s.claim(CLAUDE);
    let before = s.text().to_string();
    let theirs = write(&before);
    let n = s.external(&theirs);
    s.done(CLAUDE, note, turn);
    s.yield_turn(CLAUDE, turn);
    let id = format!("{n}.0");
    let c = s.fold().changes.into_iter().find(|c| c.id == id);
    assert!(
        c.as_ref().is_some_and(|c| c.reversible.ok),
        "setup: {id} must be revertible: {c:?}"
    );
    (s, id, before, theirs)
}

// --- the fold ---

/// Revert, restore, revert again, restore again: the state reads by parity, `flip`
/// names the latest undo's own change, and an undo pair that cancels leaves `changes`.
#[test]
fn the_fold_reads_each_toggle_by_parity_and_names_the_flip() {
    let (mut s, x, before, theirs) = replied();
    let v = wire(&s.fold());
    let c = change(&v, &x);
    assert_eq!(c["check"], Value::Null, "{c}");
    assert!(c.get("flip").is_none(), "never reverted, so no flip: {c}");

    let mut flip = x.clone();
    let mut checked: Option<Value> = None;
    let toggles = [
        (&before, "reverted"),
        (&theirs, "restored"),
        (&before, "reverted"),
        (&theirs, "restored"),
    ];
    for (k, (text, want)) in toggles.into_iter().enumerate() {
        let at = format!("toggle {}", k + 1);
        let n = s.undo(&flip, text);
        flip = format!("{n}.0");
        let v = wire(&s.fold());
        let c = change(&v, &x);
        assert_eq!(c["check"], want, "{at}: {c}");
        assert_eq!(c["flip"], json!(flip), "{at}: the latest undo's own change");
        let item = news_of(&v, &x).unwrap_or_else(|| panic!("{at}: {x} left news"));
        assert_eq!(item["check"], want, "{at}: news follows the change");
        if want == "restored" {
            assert!(
                c["undone_by"].is_null(),
                "{at}: a restore has no undone_by: {c}"
            );
            assert_eq!(yours(&v), Vec::<String>::new(), "{at}: the undos cancel");
            // Claude's text is back, so the change sits on its block again for the lane and walk
            let sp = stop_sp(&theirs, "- detail one, reworded");
            assert_eq!(c["range"]["status"], "ok", "{at}: {c}");
            assert_eq!(c["range"]["sp"], sp, "{at}: {c}");
            assert_eq!(item["sp"], sp, "{at}: {item}");
        } else {
            assert!(c["undone_by"].is_u64(), "{at}: {c}");
            assert_eq!(
                yours(&v),
                vec![flip.clone()],
                "{at}: only the standing revert"
            );
        }
        // Reverted and restored are both checks: neither puts the stop back to check
        let count = v["counts"]["to_check"].clone();
        assert_eq!(checked.get_or_insert(count.clone()), &count, "{at}");
    }
}

/// A restore is a check, so a hand-back settles it like a keep, and nothing of the
/// cancelled undos is left for the lane or the next round.
#[test]
fn a_restore_handed_back_is_settled_and_leaves_no_undo_behind() {
    let (mut s, x, before, theirs) = replied();
    let u = s.undo(&x, &before);
    s.undo(&format!("{u}.0"), &theirs);
    s.note("Looks fine now.");
    s.submit();
    let v = wire(&s.fold());
    let c = change(&v, &x);
    assert_eq!(c["check"], "restored", "{c}");
    assert_eq!(c["settled"], true, "checked, then handed back: {c}");
    assert!(news_of(&v, &x).is_none(), "settled news leaves the list");
    assert_eq!(yours(&v), Vec::<String>::new());
    assert!(
        v["deletions"].as_array().unwrap().is_empty(),
        "{}",
        v["deletions"]
    );
}

/// A cut put back and cut again: the restored cut is a removal in its gap once more.
#[test]
fn a_cut_put_back_then_cut_again_is_a_removal_again() {
    let (mut s, x, before, theirs) = replied_on(DOC, |t| t.replace("- detail two\n", ""));
    let v = wire(&s.fold());
    assert_eq!(news_of(&v, &x).unwrap()["kind"], "removal", "setup");
    let u = s.undo(&x, &before);
    let v = wire(&s.fold());
    assert_eq!(change(&v, &x)["check"], "reverted");
    assert_eq!(change(&v, &x)["flip"], json!(format!("{u}.0")));

    let back = s.undo(&format!("{u}.0"), &theirs);

    let v = wire(&s.fold());
    let c = change(&v, &x);
    assert_eq!(c["check"], "restored", "{c}");
    assert_eq!(c["flip"], json!(format!("{back}.0")));
    assert_eq!(yours(&v), Vec::<String>::new());
    let item = news_of(&v, &x).unwrap_or_else(|| panic!("{x} left news"));
    assert_eq!(
        (&item["kind"], &item["check"]),
        (&json!("removal"), &json!("restored")),
        "{item}"
    );
    assert_eq!(
        item["after_sp"],
        stop_sp(&theirs, "- detail one"),
        "in its gap again: {item}"
    );
    let cut = v["deletions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["change"] == json!(x))
        .unwrap_or_else(|| panic!("no deletion for {x}: {}", v["deletions"]));
    assert!(cut["at_byte"].is_u64(), "{cut}");
}

/// Each revert of a write's two changes flips on its own: two undos, two flips.
#[test]
fn two_changes_on_one_block_each_flip_alone() {
    let text = "# T\n\nalpha one\nbeta two\ngamma three\n\nend\n";
    let theirs = "# T\n\nALPHA one\nbeta two\nGAMMA three\n\nend\n";
    let mut s = Scenario::drafted_by(text, CLAUDE);
    let note = s.note("Shout.");
    s.submit();
    let turn = s.claim(CLAUDE);
    let n = s.external(theirs);
    s.done(CLAUDE, note, turn);
    s.yield_turn(CLAUDE, turn);
    let (a, b) = (format!("{n}.0"), format!("{n}.1"));
    let v = wire(&s.fold());
    assert_eq!(
        change(&v, &a)["range"]["sp"],
        change(&v, &b)["range"]["sp"],
        "setup: one block"
    );

    let first = s.undo(&a, "# T\n\nalpha one\nbeta two\nGAMMA three\n\nend\n");
    let second = s.undo(&b, text);
    let v = wire(&s.fold());
    assert_eq!(change(&v, &a)["flip"], json!(format!("{first}.0")));
    assert_eq!(change(&v, &b)["flip"], json!(format!("{second}.0")));

    let back = s.undo(
        &format!("{first}.0"),
        "# T\n\nALPHA one\nbeta two\ngamma three\n\nend\n",
    );
    let v = wire(&s.fold());
    assert_eq!(change(&v, &a)["check"], "restored");
    assert_eq!(change(&v, &a)["flip"], json!(format!("{back}.0")));
    assert_eq!(change(&v, &b)["check"], "reverted");
    assert_eq!(change(&v, &b)["flip"], json!(format!("{second}.0")));
    assert_eq!(yours(&v), vec![format!("{second}.0")]);
}

// --- the agent's view ---

/// `birta args…` as an agent shell runs it in the served folder.
fn agent(site: &Site, args: &[&str]) -> (i32, String, String) {
    let child = Command::new(assert_cmd::cargo::cargo_bin("birta"))
        .args(args)
        .current_dir(&site.root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", site.base.join("home"))
        .env("XDG_STATE_HOME", site.base.join("state"))
        .env("TZ", "UTC")
        .env(GATE_ON.0, GATE_ON.1)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// What `birta review` prints Claude for the hand-back `s` ends on.
fn agent_view(s: &Scenario) -> String {
    let site = Site::new();
    site.write(PR, s.text());
    site.write(LOG, &s.log());
    let (code, out, err) = agent(&site, &["review", PR, "--as", CLAUDE]);
    assert_eq!(code, 0, "{out}{err}");
    out
}

#[test]
fn a_revert_you_restored_before_handing_back_is_not_listed() {
    let (mut s, x, before, theirs) = replied();
    let u = s.undo(&x, &before);
    s.undo(&format!("{u}.0"), &theirs);
    s.comment(s.quote("second item"), None, Some("second round"));
    s.submit();

    let out = agent_view(&s);

    assert!(!out.lines().any(|l| l.starts_with("REVERTED")), "{out}");
    // The undo and its undo cancel: neither is an edit of yours
    assert!(!out.lines().any(|l| l.starts_with("YOU")), "{out}");
}

#[test]
fn a_revert_restored_then_reverted_again_is_listed_once() {
    let (mut s, x, before, theirs) = replied();
    let u = s.undo(&x, &before);
    let v = s.undo(&format!("{u}.0"), &theirs);
    s.undo(&format!("{v}.0"), &before);
    s.comment(s.quote("second item"), None, Some("second round"));
    s.submit();

    let out = agent_view(&s);

    let reverted: Vec<&str> = out.lines().filter(|l| l.starts_with("REVERTED")).collect();
    assert_eq!(reverted.len(), 1, "{out}");
    assert!(
        reverted[0].starts_with("REVERTED  \"- detail one"),
        "{}",
        reverted[0]
    );
    assert!(
        reverted[0].ends_with("\" (## Details) — your reviewer put the old text back; leave it"),
        "{}",
        reverted[0]
    );
    assert!(!out.lines().any(|l| l.starts_with("YOU")), "{out}");
}

// --- on the served socket: the bytes ---

fn op_on(path: &str, cid: &str, rev: &str, name: &str, fields: Value) -> Value {
    let mut frame = json!({"type": "op", "cid": cid, "path": path, "rev": rev, "op": name});
    for (k, v) in fields.as_object().unwrap() {
        frame[k] = v.clone();
    }
    frame
}

/// The state a fresh hello for `path` gets.
async fn state_now(served: &Served, path: &str) -> Value {
    let (_ws, frames) = served.viewing(path).await;
    let at = frames
        .windows(2)
        .position(|w| w[0]["type"] == "content" && w[1]["type"] == "review_state")
        .unwrap_or_else(|| panic!("no review_state right after a content: {frames:#?}"));
    frames[at + 1]["state"].clone()
}

/// Serves `s` as `path` (its review file beside it) in a fresh site, with `args`.
fn serve_as(s: &Scenario, path: &str, args: &[&str]) -> (Site, Served) {
    let site = Site::new();
    let name = Path::new(path).file_name().unwrap().to_str().unwrap();
    site.write(path, s.text());
    let log = s
        .log()
        .replacen("\"doc\":\"pr.md\"", &format!("\"doc\":\"{name}\""), 1);
    site.write(&format!("{path}.birta.jsonl"), &log);
    let served = site.serve(args, &[GATE_ON, HOLD_0]);
    (site, served)
}

/// Presses `u` on change `x` of `path` `times` times, as the page does: the first names
/// `x`, each later one the `flip` the state names then. The doc is `before` after each
/// odd press and `theirs` after each even one, byte for byte, and the state agrees.
async fn toggle(
    site: &Site,
    served: &Served,
    path: &str,
    x: &str,
    (before, theirs): (&str, &str),
    times: usize,
) {
    let review = site.path(&format!("{path}.birta.jsonl"));
    let (mut ws, _) = served.viewing(path).await;
    let mut disk = theirs;
    for k in 0..times {
        let at = format!("{path}, u ×{}", k + 1);
        let c = change(&state_now(served, path).await, x).clone();
        let send = if k == 0 {
            assert!(c.get("flip").is_none(), "{at}: {c}");
            x.to_string()
        } else {
            c["flip"]
                .as_str()
                .unwrap_or_else(|| panic!("{at}: no flip: {c}"))
                .to_string()
        };
        let (want, check) = if k % 2 == 0 {
            (before, "reverted")
        } else {
            (theirs, "restored")
        };
        let frame = op_on(
            path,
            &format!("u{k}"),
            &rev(disk),
            "undo_change",
            json!({"change": send}),
        );
        let ack = acked(&mut ws, frame).await;
        assert_eq!(ack["rev"], rev(want), "{at}");
        assert_eq!(site.read(path).unwrap(), want, "{at}: the doc's bytes");
        let lines = log_lines(&review);
        let edits: Vec<(usize, &Value)> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l["op"] == "edit")
            .collect();
        assert_eq!(edits.len(), k + 1, "{at}: one edit per press");
        let (i, last) = edits[k];
        assert_eq!(
            (&last["act"], &last["of"], &last["to_rev"]),
            (&json!("undo"), &json!(send), &json!(rev(want))),
            "{at}"
        );
        assert_eq!(ack["n"], json!(i + 1), "{at}");
        let c = change(&state_now(served, path).await, x).clone();
        assert_eq!(c["check"], check, "{at}: the state agrees with the file");
        assert_eq!(c["flip"], json!(format!("{}.0", i + 1)), "{at}");
        disk = want;
    }
}

/// J-10's `u u u`, and two more: each write is exact.
#[tokio::test]
async fn five_toggles_leave_the_bytes_exact_each_time() {
    let (s, x, before, theirs) = replied();
    let (site, served) = serve_as(&s, PR, &[PR]);
    toggle(&site, &served, PR, &x, (&before, &theirs), 5).await;
}

/// The journey's J-10: Claude's merge of two bullets into one is one change of a write
/// of seven, and its revert writes two blocks back. Each toggle is exact.
#[tokio::test]
async fn the_journeys_merged_bullets_toggle_exactly() {
    let mut s = Scenario::drafted_by(PR_DRAFT, CLAUDE);
    let note = s.note("Way too verbose.");
    s.submit();
    let turn = s.claim(CLAUDE);
    let n = s.external(ROUND1);
    s.done(CLAUDE, note, turn);
    s.yield_turn(CLAUDE, turn);
    let merged = s
        .fold()
        .changes
        .into_iter()
        .find(|c| c.op == n && c.what.old_lines == 2 && c.what.new_lines == 1)
        .expect("setup: the merged bullets are one change");
    assert!(merged.reversible.ok, "setup: {merged:?}");
    let before = ROUND1.replacen(&merged.new_text, &merged.old_text, 1);
    let (site, served) = serve_as(&s, PR, &[PR]);
    toggle(&site, &served, PR, &merged.id, (&before, ROUND1), 5).await;
}

#[tokio::test]
async fn crlf_without_a_final_eol_toggles_exactly() {
    let text =
        "\u{feff}# Plan\r\n\r\nIntro words here.\r\n\r\n- old item\r\n\r\nEnd without an eol";
    let (s, x, before, theirs) = replied_on(text, |t| t.replace("- old item", "- new item"));
    let (site, served) = serve_as(&s, PR, &[PR]);
    toggle(&site, &served, PR, &x, (&before, &theirs), 4).await;
}

#[tokio::test]
async fn in_a_folder_a_doc_toggles_exactly() {
    let (s, x, before, theirs) = replied();
    let (site, served) = serve_as(&s, "docs/a.md", &["."]);
    site.write("b.md", "# B\n\nOther doc.\n");
    toggle(&site, &served, "docs/a.md", &x, (&before, &theirs), 3).await;
}

/// Two changes on one block: reverting both, then restoring one, writes only its line.
#[tokio::test]
async fn two_changes_on_one_block_restore_one_at_a_time() {
    let text = "# T\n\nalpha one\nbeta two\ngamma three\n\nend\n";
    let theirs = "# T\n\nALPHA one\nbeta two\nGAMMA three\n\nend\n";
    let (s, a, _, _) = replied_on(text, |_| theirs.to_string());
    let b = a.replace(".0", ".1");
    let (site, served) = serve_as(&s, PR, &[PR]);
    let (mut ws, _) = served.viewing(PR).await;
    let steps = [
        (
            a.clone(),
            "# T\n\nalpha one\nbeta two\nGAMMA three\n\nend\n",
        ),
        (b.clone(), text),
    ];
    let mut disk = theirs;
    for (k, (id, want)) in steps.iter().enumerate() {
        let frame = op_on(
            PR,
            &format!("r{k}"),
            &rev(disk),
            "undo_change",
            json!({"change": id}),
        );
        acked(&mut ws, frame).await;
        assert_eq!(site.read(PR).unwrap(), *want, "revert {id}");
        disk = want;
    }
    let state = state_now(&served, PR).await;
    let (fa, fb) = (
        change(&state, &a)["flip"].clone(),
        change(&state, &b)["flip"].clone(),
    );
    assert!(fa.is_string() && fb.is_string() && fa != fb, "{fa} {fb}");

    let restores = [
        (fa, "# T\n\nALPHA one\nbeta two\ngamma three\n\nend\n"),
        (fb, theirs),
    ];
    for (k, (flip, want)) in restores.iter().enumerate() {
        let frame = op_on(
            PR,
            &format!("s{k}"),
            &rev(disk),
            "undo_change",
            json!({"change": flip}),
        );
        acked(&mut ws, frame).await;
        assert_eq!(site.read(PR).unwrap(), *want, "restore {flip}");
        disk = want;
    }
    let state = state_now(&served, PR).await;
    for id in [&a, &b] {
        assert_eq!(change(&state, id)["check"], "restored", "{id}");
    }
}

/// A write from your editor above the reverted line: the restore follows it.
#[tokio::test]
async fn a_restore_follows_an_outside_write_above_it() {
    let (mut s, x, before, theirs) = replied();
    let u = s.undo(&x, &before);
    let prefaced = before.replacen("# Plan\n\n", "# Plan\n\nA preface.\n\n", 1);
    s.external(&prefaced);
    let (site, served) = serve_as(&s, PR, &[PR]);
    let flip = change(&state_now(&served, PR).await, &x)["flip"].clone();
    assert_eq!(flip, json!(format!("{u}.0")), "setup");
    let (mut ws, _) = served.viewing(PR).await;

    acked(
        &mut ws,
        op_on(
            PR,
            "u1",
            &rev(&prefaced),
            "undo_change",
            json!({"change": flip}),
        ),
    )
    .await;

    let want = theirs.replacen("# Plan\n\n", "# Plan\n\nA preface.\n\n", 1);
    assert_eq!(site.read(PR).unwrap(), want);
    assert_eq!(
        change(&state_now(&served, PR).await, &x)["check"],
        "restored"
    );
}

/// A later write over the reverted line: the restore is refused and writes nothing.
#[tokio::test]
async fn an_overlapped_restore_is_not_reversible() {
    let (mut s, x, before, _) = replied();
    let u = s.undo(&x, &before);
    let over = before.replace("- detail one\n", "- detail one, from an editor\n");
    s.external(&over);
    let (site, served) = serve_as(&s, PR, &[PR]);
    let (doc, log) = (site.read(PR).unwrap(), site.read(LOG).unwrap());
    let (mut ws, _) = served.viewing(PR).await;

    let reason = nacked(
        &mut ws,
        op_on(
            PR,
            "u1",
            &rev(&over),
            "undo_change",
            json!({"change": format!("{u}.0")}),
        ),
    )
    .await;

    assert_eq!(reason, "not_reversible");
    assert_eq!(site.read(PR).unwrap(), doc);
    assert_eq!(site.read(LOG).unwrap(), log);
    assert_eq!(ops_named(&site.path(LOG), "edit").len(), 1);
}

/// A hand-back or an approve settles your revert: its flip is refused and writes nothing.
#[tokio::test]
async fn a_settled_revert_is_not_restored() {
    for round in ["hand back", "approve"] {
        let (mut s, x, before, _) = replied();
        let u = s.undo(&x, &before);
        if round == "approve" {
            s.approve();
        } else {
            s.note("Keep them apart.");
            s.submit();
            // Picked up and replied, so nothing holds the doc
            let turn = s.claim(CLAUDE);
            s.yield_turn(CLAUDE, turn);
        }
        let (site, served) = serve_as(&s, PR, &[PR]);
        let flip = change(&state_now(&served, PR).await, &x)["flip"].clone();
        assert_eq!(flip, json!(format!("{u}.0")), "setup: {round}");
        let (doc, log) = (site.read(PR).unwrap(), site.read(LOG).unwrap());
        let (mut ws, _) = served.viewing(PR).await;

        let reason = nacked(
            &mut ws,
            op_on(
                PR,
                "u1",
                &rev(&before),
                "undo_change",
                json!({"change": flip}),
            ),
        )
        .await;

        assert_eq!(reason, "not_reversible", "{round}");
        assert_eq!(site.read(PR).unwrap(), doc, "{round}");
        assert_eq!(site.read(LOG).unwrap(), log, "{round}");
    }
}
