//! The change model: hunks cut at block boundaries when a write is recorded, changes that
//! render the same, the copy each change carries, where notes on rewritten text sit, and
//! replays of real review files.

use std::path::Path;

use birta::rev::Rev;
use birta::review::chain::Chain;
use birta::review::doc::Doc;
use birta::review::fold;
use birta::review::hunk;
use birta::review::ops::{self, Hunk, Op};
use birta::review::state::{AskedKind, ReviewState};
use serde_json::{Value, json};

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, Scenario};

const CLAUDE: &str = "claude";

fn id(op: u32, idx: u32) -> String {
    format!("{op}.{idx}")
}

/// The state as the page receives it, so new fields are checked by their wire names.
fn wire(state: &ReviewState) -> Value {
    serde_json::to_value(state).unwrap()
}

fn change(state: &Value, id: &str) -> Value {
    state["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no change {id} in {}", ids(state).join(", ")))
}

fn ids(state: &Value) -> Vec<String> {
    state["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

/// Changes of the write on line `op`.
fn of_write(state: &Value, op: u32) -> Vec<Value> {
    state["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["op"] == op)
        .cloned()
        .collect()
}

fn item(state: &Value, n: u32) -> Value {
    state["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["n"] == n)
        .cloned()
        .unwrap_or_else(|| panic!("no item {n}"))
}

fn label(state: &Value, id: &str) -> Value {
    change(state, id)["label"].clone()
}

fn deletion(state: &Value, change: &str) -> Value {
    state["deletions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["change"] == change && d["kind"] == "block")
        .cloned()
        .unwrap_or_else(|| panic!("no block deletion for {change}: {}", state["deletions"]))
}

fn lost(state: &Value) -> Vec<u64> {
    state["lost"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap())
        .collect()
}

/// A turn open on a hand-back of nothing in particular.
fn claimed(s: &mut Scenario) -> u32 {
    s.note("look it over");
    s.submit();
    s.claim(CLAUDE)
}

/// The agent's write of `new` in a turn of its own; the write's line.
fn agent_writes(base: &str, new: &str) -> (Scenario, u32) {
    let mut s = Scenario::new(base);
    let turn = claimed(&mut s);
    let ext = s.external(new);
    s.yield_turn(CLAUDE, turn);
    (s, ext)
}

/// The patch recorded on line `n` of the scenario's log.
fn patch_on(s: &Scenario, n: u32) -> Vec<Hunk> {
    let parsed = s.parsed();
    let line = parsed
        .lines
        .iter()
        .find(|l| l.n == n)
        .expect("no such line");
    match &line.op {
        Op::External { patch, .. } | Op::Edit { patch, .. } => patch.clone().expect("a patch"),
        other => panic!("line {n} is not a write: {other:?}"),
    }
}

// --- real review files, copied from a try of the review layer ---

fn real(name: &str) -> (String, String) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/real_logs");
    let doc = std::fs::read_to_string(dir.join(name)).unwrap();
    let log = std::fs::read_to_string(dir.join(format!("{name}.birta.jsonl"))).unwrap();
    (doc, log)
}

fn fold_real(name: &str) -> ReviewState {
    let (doc, log) = real(name);
    let parsed = ops::parse(&log);
    assert!(parsed.bad.is_empty(), "{name}: {:?}", parsed.bad);
    let chain = Chain::build(&parsed);
    let doc = Doc::new(&doc);
    fold::fold(&parsed, &chain, &doc).state
}

/// The `pr-draft.md` text before the agent's first write, and the text that write left.
fn real_first_write() -> (String, String) {
    let (_, log) = real("pr-draft.md");
    let parsed = ops::parse(&log);
    let snap = parsed
        .lines
        .iter()
        .find_map(|l| match &l.op {
            Op::Snap { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    let patch = parsed
        .lines
        .iter()
        .find_map(|l| match &l.op {
            Op::External { patch, .. } if l.n == 10 => patch.clone(),
            _ => None,
        })
        .unwrap();
    let after = hunk::apply(&snap, &patch).unwrap();
    (snap, after)
}

/// Passes on the tree before the change model: the copies fold, every id the reviewer
/// marked seen still names a change, and the notes made on changed text still land.
#[test]
fn the_real_logs_fold_with_their_seen_ids_and_notes_in_place() {
    let (doc, _) = real("pr-draft.md");
    let state = fold_real("pr-draft.md");
    assert_eq!(state.rev, Rev::of(doc.as_bytes()).as_str());
    let v = wire(&state);
    for seen in ["10.0", "10.1", "10.2", "10.3", "33.0", "40.0"] {
        assert_eq!(change(&v, seen)["seen"], true, "{seen}");
    }
    let replies = item(&v, 7)["replies"].clone();
    assert_eq!(replies[0]["id"], "#13");
    assert_eq!(replies[0]["seen"], true);
    assert_eq!(state.counts.unseen, 0);
    for n in [24, 49, 50] {
        let at = &item(&v, n)["at"];
        assert!(
            at["status"] == "ok" || at["status"] == "moved",
            "note {n} on a changed block: {at}"
        );
        assert!(at["loc"]["sp"].is_string(), "note {n} unplaced: {at}");
    }

    let (doc, _) = real("checklist.md");
    let state = fold_real("checklist.md");
    assert_eq!(state.rev, Rev::of(doc.as_bytes()).as_str());
    assert!(state.changes.is_empty());
    let v = wire(&state);
    for n in [4, 56] {
        let at = &item(&v, n)["at"];
        assert_eq!(at["status"], "ok", "note {n}: {at}");
        assert!(at["loc"]["sp"].is_string(), "note {n} unplaced: {at}");
    }
}

#[test]
fn the_real_log_reads_in_the_change_model() {
    let v = wire(&fold_real("pr-draft.md"));
    // Hunks stored before the split are taken as they are
    assert_eq!(ids(&v), ["10.0", "10.1", "10.2", "10.3", "33.0", "40.0"]);
    assert_eq!(label(&v, "10.0"), "34 words → 18");
    assert_eq!(label(&v, "10.1"), "3 blocks → 1");
    assert_eq!(label(&v, "10.2"), "2 list items → 1");
    assert_eq!(label(&v, "10.3"), "list item removed");
    assert_eq!(label(&v, "33.0"), "list item removed");
    assert_eq!(label(&v, "40.0"), "section removed · 2 blocks");
    for removal in ["10.3", "33.0", "40.0"] {
        assert!(change(&v, removal)["range"]["sp"].is_null(), "{removal}");
    }
    assert_eq!(
        deletion(&v, "33.0")["first_line"],
        "- [ ] Ran the integration suite"
    );
    assert_eq!(deletion(&v, "40.0")["first_line"], "## Testing");
    // The doc note's covers bound the Summary rewrite
    assert_eq!(
        item(&v, 4)["outcome"]["changes"],
        serde_json::json!(["10.0"])
    );
    // The bare `no` on Stacked on sits at the change that took the section
    assert_eq!(item(&v, 6)["at"]["status"], "orphaned");
    assert_eq!(item(&v, 6)["at"]["change"], "10.1");
    assert!(!lost(&v).contains(&6));
    // The question on a line a later round deleted for another note lost its place
    assert_eq!(lost(&v), [7]);
    assert!(item(&v, 7)["at"].get("change").is_none());
}

// --- #6: hunks cut at block boundaries when a write is recorded ---

/// The reviewer's three notes on the real draft, handled by the agent's real first write
/// recorded afresh.
#[test]
fn the_real_rewrite_recorded_now_splits_the_why_paragraph_from_the_removed_section() {
    let (before, after) = real_first_write();
    let mut s = Scenario::drafted_by(&before, CLAUDE);
    let doc_note = s.note("way too verbose");
    let at = s.quote("we just fail");
    let why = s.comment(at, None, Some("say why"));
    let at = s.section("## Stacked on");
    let stacked = s.comment(at, Some("no"), None);
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(&after);
    s.done(CLAUDE, why, turn);
    s.done(CLAUDE, stacked, turn);
    s.covers(
        CLAUDE,
        doc_note,
        "Retries transient upload failures (network resets, 429s, 5xx) with exponential backoff and jitter, up to a configurable limit.",
        turn,
    );
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());

    let mine = of_write(&v, ext);
    let rewrite = mine
        .iter()
        .find(|c| c["old"] == serde_json::json!([11, 14]))
        .unwrap_or_else(|| panic!("no change over just the Why paragraph: {mine:#?}"));
    assert_eq!(rewrite["asked"]["kind"], "location");
    assert_eq!(rewrite["asked"]["notes"], serde_json::json!([why]));
    let removal = mine
        .iter()
        .find(|c| {
            c["what"]["new_lines"] == 0 && c["removed"].as_str().unwrap().contains("## Stacked on")
        })
        .unwrap_or_else(|| panic!("no removal of Stacked on: {mine:#?}"));
    let removed = removal["removed"].as_str().unwrap();
    assert!(removed.contains("- #412"), "{removed:?}");
    assert!(!removed.contains("we just fail"), "{removed:?}");
    assert_eq!(removal["asked"]["notes"], serde_json::json!([stacked]));
    assert_eq!(removal["label"], "section removed · 2 blocks");
    assert!(removal["range"]["sp"].is_null());

    let credited = |n: u32| item(&v, n)["outcome"]["changes"].clone();
    assert_eq!(credited(why), serde_json::json!([rewrite["id"]]));
    assert_eq!(credited(stacked), serde_json::json!([removal["id"]]));
    assert_eq!(item(&v, stacked)["at"]["change"], removal["id"]);
    assert!(lost(&v).is_empty(), "{}", v["lost"]);
}

#[test]
fn a_heading_and_the_paragraph_under_it_edited_together_are_two_changes() {
    let base = "# T\n\n## Old heading\nBody text here.\n\nEnd.\n";
    let (s, ext) = agent_writes(base, "# T\n\n## New heading\nBody text changed.\n\nEnd.\n");
    let v = wire(&s.fold());
    let olds: Vec<Value> = of_write(&v, ext).iter().map(|c| c["old"].clone()).collect();
    assert_eq!(olds, [serde_json::json!([3, 3]), serde_json::json!([4, 4])]);
}

#[test]
fn adjacent_list_items_edited_together_are_two_changes() {
    let base = "# T\n\n- alpha one\n- beta two\n- gamma\n";
    let (s, ext) = agent_writes(base, "# T\n\n- alpha ONE\n- beta TWO\n- gamma\n");
    let v = wire(&s.fold());
    let olds: Vec<Value> = of_write(&v, ext).iter().map(|c| c["old"].clone()).collect();
    assert_eq!(olds, [serde_json::json!([3, 3]), serde_json::json!([4, 4])]);
}

#[test]
fn edits_inside_one_block_stay_one_change() {
    let cases = [
        (
            "a paragraph rewrapped from three lines to two",
            "# T\n\nOne two three four\nfive six seven eight\nnine ten.\n\nEnd.\n",
            "# T\n\nOne two three four five six\nseven eight nine ten, more.\n\nEnd.\n",
        ),
        (
            "a whole section removed",
            DOC,
            &DOC.replace("## More\n\n- more item\n\n", ""),
        ),
        (
            "two paragraphs inserted together",
            "# T\n\nEnd.\n",
            "# T\n\nFirst new.\n\nSecond new.\n\nEnd.\n",
        ),
        (
            "a code block's lines",
            "# T\n\n```\nx1\nx2\nx3\n```\n",
            "# T\n\n```\ny1\ny2\n```\n",
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new) in cases {
        let (s, ext) = agent_writes(base, new);
        let mine = of_write(&wire(&s.fold()), ext);
        if mine.len() != 1 {
            bad.push(format!("{name}: {mine:#?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Passes on the tree before the change model; the split must keep it so.
#[test]
fn a_recorded_patch_always_rebuilds_the_new_text() {
    let cases = [
        ("lf", "# T\n\nold line\n\nend\n", "# T\n\nnew line\n\nend\n"),
        (
            "crlf, heading and paragraph",
            "# T\r\n## H\r\nbody\r\n\r\nend\r\n",
            "# T\r\n## H2\r\nbody two\r\n\r\nend\r\n",
        ),
        (
            "mixed",
            "# T\r\n\nold line\n\r\nend\r\n",
            "# T\r\n\nnew\nline\n\r\nend\r\n",
        ),
        (
            "lone cr",
            "# T\r\rold\r- a\r- b\r",
            "# T\r\rnew\r- A\r- B\r",
        ),
        (
            "bom, line 1",
            "\u{feff}# Title\n## Sub\n\nbody\n",
            "\u{feff}# Title two\n## Sub two\n\nbody\n",
        ),
        ("no final eol", "# T\n\n- a\n- b", "# T\n\n- A\n- B"),
        ("final eol added", "# T\n- a\n- b", "# T\n- a\n- b\n"),
        ("final eol removed", "# T\n- a\n- b\n", "# T\n- A\n- B"),
        ("from empty", "", "# T\n\nbody\n"),
        ("to empty", "# T\n\nbody\n", ""),
        (
            "the real first write",
            &real_first_write().0,
            &real_first_write().1,
        ),
        (
            "blocks merged",
            "# T\n\npara one\n\npara two\n\n- x\n",
            "# T\n\npara one and two\n- x\n",
        ),
    ];
    let mut bad = Vec::new();
    for (name, old, new) in cases {
        let s = Scenario::new(old);
        let Some(rec) = Chain::build(&s.parsed()).record(new) else {
            bad.push(format!("{name}: nothing recorded"));
            continue;
        };
        let patch = rec.patch.expect("a patch");
        if patch.iter().any(|h| h.d.is_empty() && h.i.is_empty()) {
            bad.push(format!("{name}: an empty hunk {patch:?}"));
        }
        match hunk::apply(old, &patch) {
            Ok(text) if text == new => {}
            other => bad.push(format!("{name}: {other:?}")),
        }
        // A cut only divides the plain diff's hunks, never moves or joins them
        let whole = spans(&hunk::diff(old, new));
        for piece in spans(&patch) {
            let inside = |w: &Spans| {
                w.0.0 <= piece.0.0 && piece.0.1 <= w.0.1 && w.1.0 <= piece.1.0 && piece.1.1 <= w.1.1
            };
            if !whole.iter().any(inside) {
                bad.push(format!("{name}: {piece:?} is in no hunk of {whole:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// A hunk's old and new lines, as 0-based half-open ranges.
type Spans = ((u32, u32), (u32, u32));

fn spans(hunks: &[Hunk]) -> Vec<Spans> {
    let mut shift = 0i64;
    hunks
        .iter()
        .map(|h| {
            let new_a = (i64::from(h.a) + shift) as u32;
            shift += h.i.len() as i64 - h.d.len() as i64;
            (
                (h.a, h.a + h.d.len() as u32),
                (new_a, new_a + h.i.len() as u32),
            )
        })
        .collect()
}

/// Passes on the tree before the change model: a hunk already in the log keeps its id,
/// so seen state recorded against it stands.
#[test]
fn a_hunk_stored_whole_keeps_its_id() {
    let (before, after) = real_first_write();
    let mut s = Scenario::drafted_by(&before, CLAUDE);
    let turn = claimed(&mut s);
    let ext = s.external_diffed(&after);
    s.yield_turn(CLAUDE, turn);
    let merged = hunk::diff(&before, &after);
    assert_eq!(patch_on(&s, ext), merged, "setup");
    s.seen(&[&id(ext, 1)]);
    let v = wire(&s.fold());
    assert_eq!(of_write(&v, ext).len(), merged.len());
    let c = change(&v, &id(ext, 1));
    assert_eq!(c["seen"], true);
    assert_eq!(c["old"], serde_json::json!([11, 18]));
}

/// Passes on the tree before the change model: hunks the fold derives for a snap are
/// not cut, so ids in logs written before the split never move.
#[test]
fn a_snap_standing_in_for_a_write_keeps_its_derived_ids() {
    let (before, after) = real_first_write();
    let mut s = Scenario::drafted_by(&before, CLAUDE);
    let snap = s.snap_to(&after);
    let v = wire(&s.fold());
    let olds: Vec<Value> = of_write(&v, snap)
        .iter()
        .map(|c| c["old"].clone())
        .collect();
    let want: Vec<Value> = hunk::diff(&before, &after)
        .iter()
        .map(|h| serde_json::json!([h.a + 1, h.a + h.d.len() as u32]))
        .collect();
    assert_eq!(olds, want);
}

// --- #12: a hunk that renders the same is no change ---

#[test]
fn a_hunk_that_renders_the_same_makes_no_change_and_stays_in_the_log() {
    let base = "# T\n\n## Rollout\n\nShip it. \n\nEnd.\n";
    let cases = [
        (
            "blank line under a heading",
            "# T\n\n## Rollout\nShip it. \n\nEnd.\n",
        ),
        (
            "one trailing space",
            "# T\n\n## Rollout\n\nShip it.\n\nEnd.\n",
        ),
        (
            "a blank line added at the end",
            "# T\n\n## Rollout\n\nShip it. \n\nEnd.\n\n",
        ),
    ];
    let mut bad = Vec::new();
    for (name, new) in cases {
        let (s, ext) = agent_writes(base, new);
        let state = s.fold();
        let v = wire(&state);
        let counts = &state.counts;
        if !of_write(&v, ext).is_empty()
            || !state.deletions.is_empty()
            || (counts.unseen, counts.unasked) != (0, 0)
            || counts.attention != counts.not_handled
            || patch_on(&s, ext).is_empty()
        {
            bad.push(format!(
                "{name}: {} {:?} {counts:?}",
                v["changes"], state.deletions
            ));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn whitespace_that_changes_the_page_is_a_change() {
    let cases = [
        (
            "a blank line that kept two paragraphs apart",
            "# T\n\nPara one.\n\nPara two.\n",
            "# T\n\nPara one.\nPara two.\n",
        ),
        (
            "a trailing double space that broke the line",
            "# T\n\nline one  \nline two\n",
            "# T\n\nline one\nline two\n",
        ),
        (
            "indentation that makes code",
            "# T\n\nPara one.\n\nplain\n",
            "# T\n\nPara one.\n\n    plain\n",
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new) in cases {
        let (s, ext) = agent_writes(base, new);
        let state = s.fold();
        if of_write(&wire(&state), ext).len() != 1 || state.counts.unseen != 1 {
            bad.push(format!("{name}: {:?}", state.changes));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn a_hunk_that_renders_the_same_hides_beside_a_real_one_without_renumbering() {
    let base = "# T\n\n## Rollout\n\nShip it.\n\nKeep going.\n\nThe end is near.\n";
    let new = "# T\n\n## Rollout\nShip it.\n\nKeep going.\n\nThe end is here.\n";
    let (s, ext) = agent_writes(base, new);
    let patch = patch_on(&s, ext);
    assert_eq!(patch.len(), 2, "setup: {patch:?}");
    let state = s.fold();
    let v = wire(&state);
    assert_eq!(ids(&v), [id(ext, 1)]);
    assert_eq!(change(&v, &id(ext, 1))["old"], serde_json::json!([9, 9]));
    assert_eq!(state.counts.unseen, 1);
}

// --- #5: copy by kind and size ---

#[test]
fn each_change_says_what_kind_of_change_it_is() {
    let para = "The quick brown fox jumps over the lazy dog today.\n";
    let cases: [(&str, String, String, &str); 9] = [
        (
            "an in-place prose edit",
            format!("# T\n\n{para}"),
            "# T\n\nThe quick red fox leaps over the lazy dog today.\n".to_string(),
            "2 words changed",
        ),
        (
            "a prose edit that shrinks it by over a quarter",
            "# T\n\none two three four five six seven eight\n".to_string(),
            "# T\n\none two three four\n".to_string(),
            "8 words → 4",
        ),
        (
            "a paragraph removed",
            format!("# T\n\n{para}\nEnd.\n"),
            "# T\n\nEnd.\n".to_string(),
            "paragraph removed",
        ),
        (
            "a list item removed",
            DOC.to_string(),
            DOC.replace("- third item\n", ""),
            "list item removed",
        ),
        (
            "a section removed",
            DOC.to_string(),
            DOC.replace("## More\n\n- more item\n\n", ""),
            "section removed · 2 blocks",
        ),
        (
            "a paragraph added",
            "# T\n\nEnd.\n".to_string(),
            format!("# T\n\n{para}\nEnd.\n"),
            "paragraph added",
        ),
        (
            "two paragraphs added",
            "# T\n\nEnd.\n".to_string(),
            "# T\n\nFirst new.\n\nSecond new.\n\nEnd.\n".to_string(),
            "2 paragraphs added",
        ),
        (
            "code lines",
            "# T\n\n```\nx1\nx2\nx3\n```\n".to_string(),
            "# T\n\n```\ny1\ny2\n```\n".to_string(),
            "3 lines → 2",
        ),
        (
            "one code line",
            "# T\n\n```\nlet a = 1;\n```\n".to_string(),
            "# T\n\n```\nlet a = 2;\n```\n".to_string(),
            "1 line changed",
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new, want) in cases {
        let (s, ext) = agent_writes(&base, &new);
        let mine = of_write(&wire(&s.fold()), ext);
        let got: Vec<&Value> = mine.iter().map(|c| &c["label"]).collect();
        if got != [want] {
            bad.push(format!("{name}: {got:?}, want {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn two_paragraphs_stored_as_one_hunk_read_as_two_to_one() {
    let mut s = Scenario::new("# T\n\nPara one.\n\nPara two.\n");
    let turn = claimed(&mut s);
    let ext = s.external_diffed("# T\n\nPara one and two.\n");
    s.yield_turn(CLAUDE, turn);
    assert_eq!(patch_on(&s, ext).len(), 1, "setup");
    assert_eq!(label(&wire(&s.fold()), &id(ext, 0)), "2 paragraphs → 1");
}

// --- N10: a removal is a tick, with its first line ---

#[test]
fn a_removal_marks_no_block_and_its_tombstone_shows_the_first_removed_line() {
    let (s, ext) = agent_writes(DOC, &DOC.replace("## More\n\n- more item\n\n", ""));
    let v = wire(&s.fold());
    let c = change(&v, &id(ext, 0));
    assert!(c["range"]["sp"].is_null(), "{c}");
    let d = deletion(&v, &id(ext, 0));
    assert_eq!(d["first_line"], "## More");
    // The tick still hangs under the table above the gap
    assert!(d["after_sp"].is_string(), "{d}");

    let text = "# T\n\nKeep this.\n\n\n   \nGone line one\ngone line two\n\nEnd.\n";
    let (s, ext) = agent_writes(text, "# T\n\nKeep this.\n\nEnd.\n");
    let v = wire(&s.fold());
    assert_eq!(deletion(&v, &id(ext, 0))["first_line"], "Gone line one");
}

#[test]
fn a_rewrite_keeps_its_block() {
    let (s, ext) = agent_writes(
        DOC,
        &DOC.replace("- third item\n", "- third item, reworded\n"),
    );
    let v = wire(&s.fold());
    assert!(change(&v, &id(ext, 0))["range"]["sp"].is_string());
}

// --- N8 / N9: a handled note on rewritten or removed text sits at its change ---

#[test]
fn a_handled_note_whose_text_the_agent_rewrote_sits_at_its_change() {
    let base = "# T\n\nCosts have doubled since the CDN migration last spring.\n\nEnd.\n";
    let mut s = Scenario::new(base);
    let at = s.quote("have doubled since");
    let x = s.comment(at, None, Some("too casual, say the cost"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext =
        s.external("# T\n\nEgress spend rose 112% after we moved to a new provider.\n\nEnd.\n");
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let at = &item(&v, x)["at"];
    assert_eq!(at["status"], "orphaned", "setup: {at}");
    assert_eq!(at["change"], id(ext, 0));
    assert!(lost(&v).is_empty());
}

#[test]
fn a_bare_verdict_on_a_section_its_turn_removed_sits_at_the_removal() {
    let mut s = Scenario::new(DOC);
    let at = s.section("## More");
    let x = s.comment(at, Some("no"), None);
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(&DOC.replace("## More\n\n- more item\n\n", ""));
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(
        item(&v, x)["outcome"]["changes"],
        serde_json::json!([id(ext, 0)])
    );
    assert_eq!(item(&v, x)["at"]["change"], id(ext, 0));
    assert!(lost(&v).is_empty());
}

#[test]
fn a_note_whose_text_another_turn_removed_stays_lost_without_a_change() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- third item", "yes");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let y = s.verdict("- more item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.external(
        &DOC.replace("- third item\n", "")
            .replace("- more item\n", ""),
    );
    s.done(CLAUDE, y, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(item(&v, x)["at"]["status"], "orphaned", "setup");
    assert_eq!(lost(&v), [u64::from(x)]);
    assert!(item(&v, x)["at"].get("change").is_none());
}

#[test]
fn a_note_still_on_its_text_names_no_change() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- third item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.external(&DOC.replace("- third item\n", "- third item, reworded\n"));
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_ne!(item(&v, x)["at"]["status"], "orphaned", "setup");
    assert!(item(&v, x)["at"].get("change").is_none());
}

// --- doc notes: `covers` credits the changes it bound ---

/// Two paragraphs the agent rewrites, apart, so each is its own change.
const TWO: &str = "# T\n\nFirst paragraph, old.\n\nMiddle.\n\nSecond paragraph, old.\n";
const TWO_NEW: &str = "# T\n\nFirst paragraph, new.\n\nMiddle.\n\nSecond paragraph, new.\n";

#[test]
fn a_doc_notes_covers_credit_the_changes_they_bound() {
    let mut s = Scenario::new(TWO);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(TWO_NEW);
    s.done(CLAUDE, x, turn);
    s.covers(CLAUDE, x, "First paragraph, new.", turn);
    s.covers(CLAUDE, x, "Second paragraph, new.", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let v = wire(&state);
    let outcome = &item(&v, x)["outcome"];
    assert_eq!(outcome["done"], true);
    assert_eq!(
        outcome["changes"],
        serde_json::json!([id(ext, 0), id(ext, 1)])
    );
    for c in of_write(&v, ext) {
        assert_eq!(c["asked"]["kind"], "cited", "setup: {c}");
    }
}

#[test]
fn covers_alone_credit_a_doc_note() {
    let mut s = Scenario::new(TWO);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(TWO_NEW);
    s.covers(CLAUDE, x, "Second paragraph, new.", turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(
        item(&v, x)["outcome"]["changes"],
        serde_json::json!([id(ext, 1)])
    );
}

#[test]
fn covers_that_bind_nothing_credit_no_change_to_a_doc_note() {
    // The quote names text the turn didn't write
    let mut s = Scenario::new(TWO);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.external(TWO_NEW);
    s.done(CLAUDE, x, turn);
    s.covers(CLAUDE, x, "Middle.", turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let changes = &item(&v, x)["outcome"]["changes"];
    assert_eq!(changes, &serde_json::json!([]), "{}", item(&v, x));

    // A turn that never held the note
    let mut s = Scenario::new(TWO);
    s.note("first round");
    s.submit();
    let turn = s.claim(CLAUDE);
    let x = s.note("written during the turn");
    s.external(TWO_NEW);
    s.covers(CLAUDE, x, "First paragraph, new.", turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert!(item(&v, x)["outcome"].is_null(), "{}", item(&v, x));
}

/// Passes on the tree before the change model: only doc notes take credit from covers.
#[test]
fn an_anchored_note_is_not_credited_with_changes_elsewhere_by_covers() {
    let mut s = Scenario::new(TWO);
    let at = s.block("First paragraph");
    let x = s.comment(at, None, Some("reword"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(TWO_NEW);
    s.done(CLAUDE, x, turn);
    s.covers(CLAUDE, x, "Second paragraph, new.", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let v = wire(&state);
    assert_eq!(
        item(&v, x)["outcome"]["changes"],
        serde_json::json!([id(ext, 0)])
    );
    let second = state.changes.iter().find(|c| c.id == id(ext, 1)).unwrap();
    assert_eq!(second.asked.kind, AskedKind::Cited, "setup");
}

// --- killer tests from the adversarial review ---

/// A cut gives each piece its own new lines; neither piece is a removal.
#[test]
fn each_piece_of_a_split_keeps_its_own_new_lines() {
    let cases = [
        (
            "# T\n\n## Old heading\nBody text here.\n\nEnd.\n",
            "# T\n\n## New heading\nBody text changed.\n\nEnd.\n",
        ),
        (
            "# T\n\n- alpha one\n- beta two\n- gamma\n",
            "# T\n\n- alpha ONE\n- beta TWO\n- gamma\n",
        ),
        (
            "# T\r\n\r\n## Old heading\r\nBody text here.\r\n\r\nEnd.\r\n",
            "# T\r\n\r\n## New heading\r\nBody text changed.\r\n\r\nEnd.\r\n",
        ),
        (
            "# T\r\r## Old heading\rBody text here.\r\rEnd.\r",
            "# T\r\r## New heading\rBody text changed.\r\rEnd.\r",
        ),
    ];
    let mut bad = Vec::new();
    for (base, new) in cases {
        let (s, ext) = agent_writes(base, new);
        let state = s.fold();
        let v = wire(&state);
        let got: Vec<(Value, Value, Value)> = of_write(&v, ext)
            .iter()
            .map(|c| (c["old"].clone(), c["new"].clone(), c["label"].clone()))
            .collect();
        let want = [
            (json!([3, 3]), json!([3, 3]), json!("1 word changed")),
            (json!([4, 4]), json!([4, 4]), json!("1 word changed")),
        ];
        if got != want || !state.deletions.is_empty() {
            bad.push(format!("{base:?}: {got:?} {:?}", state.deletions));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn a_block_rewritten_line_for_line_stays_one_change() {
    let cases = [
        (
            "a two-line paragraph",
            "# T\n\nAlpha beta gamma\ndelta epsilon zeta.\n\nEnd.\n",
            "# T\n\nAlpha BETA gamma\ndelta EPSILON zeta.\n\nEnd.\n",
            "2 words changed",
        ),
        (
            "a three-line fence, every line",
            "# T\n\n```\nx1\nx2\nx3\n```\n",
            "# T\n\n```\ny1\ny2\ny3\n```\n",
            "3 lines changed",
        ),
        (
            "a list item and its continuation line",
            "# T\n\n- item one alpha\n  goes on beta\n- two\n",
            "# T\n\n- item ONE alpha\n  goes on BETA\n- two\n",
            "2 words changed",
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new, want) in cases {
        let (s, ext) = agent_writes(base, new);
        let mine = of_write(&wire(&s.fold()), ext);
        let got: Vec<&Value> = mine.iter().map(|c| &c["label"]).collect();
        if got != [want] {
            bad.push(format!("{name}: {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Two paragraphs merged, or one split, inside one plain hunk is one change.
#[test]
fn paragraphs_merged_or_split_in_one_write_are_one_change() {
    let one = "# T\n\nAlpha beta gamma delta, epsilon zeta eta theta.\n\nEnd.\n";
    let two = "# T\n\nAlpha beta gamma delta.\n\nEpsilon zeta eta theta.\n\nEnd.\n";
    let mut bad = Vec::new();
    for (name, base, new, want) in [
        ("merged", two, one, "2 paragraphs → 1"),
        ("split", one, two, "1 paragraph → 2"),
    ] {
        let (s, ext) = agent_writes(base, new);
        let mine = of_write(&wire(&s.fold()), ext);
        let got: Vec<&Value> = mine.iter().map(|c| &c["label"]).collect();
        if got != [want] {
            bad.push(format!("{name}: {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// The reviewer asked to fold the second paragraph into the first; the agent wrote one
/// sentence keeping few of either's words.
const BACKGROUND: &str = "## Background\n\nReports are rendered by the worker pool and stored in Redis under a key\nbuilt from the report id and the requester's locale.\n\nThe timer was chosen in 2023 because writes were rare. They are not rare\nany more: the billing team writes several times a minute.\n\n## Plan\n\nShip it.\n";
const MERGED: &str = "## Background\n\nReports are cached in Redis per report and locale; the one-hour timer\ndates from when writes were rare, but billing now writes several times a minute.\n\n## Plan\n\nShip it.\n";

#[test]
fn two_paragraphs_rewritten_into_one_are_one_change_the_merge_note_asked_for() {
    let mut s = Scenario::new(BACKGROUND);
    let at = s.block("The timer was chosen");
    let merge = s.comment(at, None, Some("fold this into the paragraph above"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(MERGED);
    s.done(CLAUDE, merge, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let mine = of_write(&v, ext);
    let got: Vec<(&Value, &Value, &Value)> = mine
        .iter()
        .map(|c| (&c["label"], &c["asked"]["kind"], &c["asked"]["notes"]))
        .collect();
    assert_eq!(
        got,
        [(
            &json!("2 paragraphs → 1"),
            &json!("location"),
            &json!([merge])
        )]
    );
    assert_eq!(v["counts"]["unasked"], 0);
}

/// A paragraph cut and its neighbour edited share no words, so they stay two changes.
#[test]
fn a_paragraph_removed_beside_an_edited_one_stays_two_changes() {
    let (s, ext) = agent_writes(
        "# T\n\ngone para here\n\nold para text\n",
        "# T\n\nnew para text\n",
    );
    let v = wire(&s.fold());
    let labels: Vec<Value> = of_write(&v, ext)
        .iter()
        .map(|c| c["label"].clone())
        .collect();
    assert_eq!(
        labels,
        [json!("paragraph removed"), json!("1 word changed")]
    );
}

/// The ux-a write: `## Stacked on` and its item cut, and a blank line added under
/// `## Testing`.
const STACKED: &str = "# Add retries\n\n## How\n\n- `UploadClient::send` wraps each attempt\n- Retry-After headers win over the computed delay\n\n## Stacked on\n\n- #412 (error classification in the HTTP layer)\n\n## Testing\n\n- [x] Unit tests for the delay schedule\n- [ ] Ran the integration suite\n";
const UNSTACKED: &str = "# Add retries\n\n## How\n\n- `UploadClient::send` wraps each attempt\n- Retry-After headers win over the computed delay\n\n## Testing\n\n\n- [x] Unit tests for the delay schedule\n- [ ] Ran the integration suite\n";

/// What the agent's write shows: `(label, first_line of its tombstone)` per change.
fn shown(v: &Value, ext: u32) -> Vec<(Value, Value)> {
    of_write(v, ext)
        .iter()
        .map(|c| {
            let first = v["deletions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["change"] == c["id"] && d["kind"] == "block")
                .map_or(Value::Null, |d| d["first_line"].clone());
            (c["label"].clone(), first)
        })
        .collect()
}

#[test]
fn a_blank_line_added_under_a_heading_is_no_heading_removal() {
    let (s, ext) = agent_writes(STACKED, UNSTACKED);
    let v = wire(&s.fold());
    assert_eq!(
        shown(&v, ext),
        [(json!("section removed · 2 blocks"), json!("## Stacked on"))]
    );
}

#[test]
fn a_bare_x_on_a_section_the_agent_cut_is_one_removal_it_asked_for() {
    let mut s = Scenario::new(STACKED);
    let x = s.comment(s.section("## Stacked on"), Some("no"), None);
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(UNSTACKED);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let mine = of_write(&v, ext);
    assert_eq!(mine.len(), 1, "{mine:#?}");
    assert_eq!(mine[0]["label"], "section removed · 2 blocks");
    assert_eq!(mine[0]["asked"], json!({"kind": "location", "notes": [x]}));
    assert_eq!(
        deletion(&v, mine[0]["id"].as_str().unwrap())["label"],
        "section · 2 blocks"
    );
    assert_eq!(item(&v, x)["outcome"]["changes"], json!([mine[0]["id"]]));
}

/// Patched and rev-only writes of the same text differ only in the cut; a rev-only
/// write's derived hunks stay whole.
#[test]
fn a_rev_only_write_keeps_its_derived_ids() {
    let (before, after) = real_first_write();
    let mut s = Scenario::drafted_by(&before, CLAUDE);
    let turn = claimed(&mut s);
    let ext = s.external_rev_only(&after);
    s.yield_turn(CLAUDE, turn);
    let olds: Vec<Value> = of_write(&wire(&s.fold()), ext)
        .iter()
        .map(|c| c["old"].clone())
        .collect();
    let want: Vec<Value> = hunk::diff(&before, &after)
        .iter()
        .map(|h| json!([h.a + 1, h.a + h.d.len() as u32]))
        .collect();
    assert_eq!(olds, want);
}

/// Your edit across a heading and its paragraph, put back by the agent: both of its
/// pieces revert a piece of yours.
#[test]
fn an_agent_reverting_your_two_block_edit_reverts_both_pieces() {
    let base = "# T\n\n## Old heading\nBody text here.\n\nEnd.\n";
    let mut s = Scenario::new(base);
    let mine = s.edit("# T\n\n## New heading\nBody text changed.\n\nEnd.\n");
    let turn = claimed(&mut s);
    let ext = s.external(base);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let got: Vec<(Value, Value)> = of_write(&v, ext)
        .iter()
        .map(|c| {
            (
                c["changes_your_edit"].clone(),
                c["reverts_your_edit"].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (json!(id(mine, 0)), json!(true)),
            (json!(id(mine, 1)), json!(true))
        ]
    );
}

#[test]
fn whitespace_that_renders_the_same_is_no_change_wherever_it_is() {
    let cases = [
        (
            "a pure reflow",
            "# T\n\nOne two three four\nfive six seven eight\nnine ten.\n\nEnd.\n",
            "# T\n\nOne two three four five six\nseven eight nine ten.\n\nEnd.\n",
        ),
        (
            "table cell padding",
            "# T\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            "# T\n\n| a  | b  |\n|---|---|\n| 1  | 2  |\n",
        ),
        ("a final EOL added", "# T\n\nEnd.", "# T\n\nEnd.\n"),
        (
            "a reflow across inline code",
            "# T\n\nRun `cargo\ntest` now.\n",
            "# T\n\nRun `cargo test` now.\n",
        ),
        (
            "one line's CRLF made LF",
            "# T\r\n\r\nEnd.\r\n",
            "# T\r\n\r\nEnd.\n",
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new) in cases {
        let (s, ext) = agent_writes(base, new);
        let state = s.fold();
        if !of_write(&wire(&state), ext).is_empty()
            || !state.deletions.is_empty()
            || state.counts.unseen != 0
            || patch_on(&s, ext).is_empty()
        {
            bad.push(format!("{name}: {:?}", state.changes));
        }
    }
    // A snap's derived hunks are dropped the same way
    let mut s = Scenario::new("# T\n\nShip it. \n");
    let snap = s.snap_to("# T\n\nShip it.\n");
    if !of_write(&wire(&s.fold()), snap).is_empty() {
        bad.push("snap".to_string());
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn whitespace_inside_code_or_that_reshapes_blocks_is_a_change() {
    let cases = [
        (
            "a doubled space in a fence",
            "# T\n\n```\na b\n```\n",
            "# T\n\n```\na  b\n```\n",
        ),
        (
            "a trailing space in a fence",
            "# T\n\n```\nab\n```\n",
            "# T\n\n```\nab \n```\n",
        ),
        (
            "a blank line in a fence",
            "# T\n\n```\na\nb\n```\n",
            "# T\n\n```\na\n\nb\n```\n",
        ),
        (
            "a reflow inside a fence",
            "# T\n\n```\na b\nc\n```\n",
            "# T\n\n```\na\nb c\n```\n",
        ),
        (
            "a tight list made loose",
            "# T\n\n- a\n- b\n",
            "# T\n\n- a\n\n- b\n",
        ),
        (
            "a blank before a rule, making a setext heading",
            "# T\n\nPara.\n\n---\n",
            "# T\n\nPara.\n---\n",
        ),
        (
            "a reflow that moves a hard break",
            "# T\n\none two  \nthree four\n",
            "# T\n\none  \ntwo three four\n",
        ),
        (
            "a doubled space in inline code",
            "# T\n\nRun `a b` now.\n",
            "# T\n\nRun `a  b` now.\n",
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new) in cases {
        let (s, ext) = agent_writes(base, new);
        let state = s.fold();
        if of_write(&wire(&state), ext).len() != 1 || state.counts.unseen != 1 {
            bad.push(format!("{name}: {:?}", state.changes));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn a_label_counts_words_strictly_and_without_markers() {
    let para = "The quick brown fox jumps over the lazy dog today.\n";
    let cases: [(&str, String, String, &str); 10] = [
        (
            "one word",
            format!("# T\n\n{para}"),
            "# T\n\nThe quick brown fox leaps over the lazy dog today.\n".into(),
            "1 word changed",
        ),
        (
            "exactly a quarter shorter",
            "# T\n\none two three four five six seven eight\n".into(),
            "# T\n\none two three four five six\n".into(),
            "2 words changed",
        ),
        (
            "half as long again",
            "# T\n\none two three four\n".into(),
            "# T\n\none two three four five six\n".into(),
            "4 words → 6",
        ),
        // Counted with `##` it would be 4 → 3, exactly a quarter
        (
            "a heading's marker",
            "# T\n\n## Alpha beta gamma\n\nEnd.\n".into(),
            "# T\n\n## Alpha beta\n\nEnd.\n".into(),
            "3 words → 2",
        ),
        (
            "a task item's markers",
            "# T\n\n- [ ] alpha beta gamma\n- other\n".into(),
            "# T\n\n- [ ] alpha beta\n- other\n".into(),
            "3 words → 2",
        ),
        // Measured on the paragraph, not the hunk's one line
        (
            "a line cut from a paragraph",
            "# T\n\none two three four\nfive six seven eight\nnine ten eleven twelve\n\nEnd.\n"
                .into(),
            "# T\n\none two three four\nfive six seven eight\n\nEnd.\n".into(),
            "12 words → 8",
        ),
        (
            "a section of three items",
            "# T\n\n## Gone\n\n- a\n- b\n- c\n\n## Kept\n\nText.\n".into(),
            "# T\n\n## Kept\n\nText.\n".into(),
            "section removed · 4 blocks",
        ),
        (
            "a checkbox ticked",
            "# T\n\n- [ ] ship it\n- other\n".into(),
            "# T\n\n- [x] ship it\n- other\n".into(),
            "formatting changed",
        ),
        (
            "a heading level",
            "# T\n\n## Plan\n\nEnd.\n".into(),
            "# T\n\n### Plan\n\nEnd.\n".into(),
            "formatting changed",
        ),
        (
            "a table row removed",
            "# T\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n".into(),
            "# T\n\n| a | b |\n|---|---|\n| 1 | 2 |\n".into(),
            "table row removed",
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new, want) in cases {
        let (s, ext) = agent_writes(&base, &new);
        let mine = of_write(&wire(&s.fold()), ext);
        let got: Vec<&Value> = mine.iter().map(|c| &c["label"]).collect();
        if got != [want] {
            bad.push(format!("{name}: {got:?}, want {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn a_tombstone_says_what_it_removed_and_its_first_line_has_no_terminator() {
    let cases = [
        (
            DOC.to_string(),
            DOC.replace("## More\n\n- more item\n\n", ""),
            "section · 2 blocks",
            "## More",
        ),
        (
            DOC.to_string(),
            DOC.replace("- third item\n", ""),
            "list item",
            "- third item",
        ),
        (
            "# T\n\nGone para.\n\nEnd.\n".to_string(),
            "# T\n\nEnd.\n".to_string(),
            "paragraph",
            "Gone para.",
        ),
        (
            "# T\r\n\r\nGone para.\r\n\r\nEnd.\r\n".to_string(),
            "# T\r\n\r\nEnd.\r\n".to_string(),
            "paragraph",
            "Gone para.",
        ),
        (
            "# T\r\rGone para.\r\rEnd.\r".to_string(),
            "# T\r\rEnd.\r".to_string(),
            "paragraph",
            "Gone para.",
        ),
    ];
    let mut bad = Vec::new();
    for (base, new, want_label, want_first) in cases {
        let (s, ext) = agent_writes(&base, &new);
        let d = deletion(&wire(&s.fold()), &id(ext, 0));
        if d["label"] != want_label || d["first_line"] != want_first {
            bad.push(format!("{base:?}: {d}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn a_line_cut_from_a_paragraph_keeps_its_block() {
    let base = "# T\n\none two three\nfour five six\n\nEnd.\n";
    let (s, ext) = agent_writes(base, "# T\n\none two three\n\nEnd.\n");
    let v = wire(&s.fold());
    assert!(change(&v, &id(ext, 0))["range"]["sp"].is_string());
}

/// A section note whose write split in two sits at the first piece.
#[test]
fn a_section_note_split_across_two_changes_sits_at_the_first() {
    let base = "# T\n\n## Old heading\nBody text here.\n\nEnd.\n";
    let mut s = Scenario::new(base);
    let at = s.section("## Old heading");
    let x = s.comment(at, None, Some("redo this section"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external("# T\n\n## Rollout window\nShips Monday at nine.\n\nEnd.\n");
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(
        item(&v, x)["outcome"]["changes"],
        json!([id(ext, 0), id(ext, 1)])
    );
    assert_eq!(item(&v, x)["at"]["status"], "orphaned", "setup");
    assert_eq!(item(&v, x)["at"]["change"], id(ext, 0));
}

const COSTS: &str = "# T\n\nCosts have doubled since the CDN migration last spring.\n\nEnd.\n";
const COSTS_NEW: &str = "# T\n\nEgress spend rose 112% after the move.\n\nEnd.\n";

/// The agent's rewrite of a quoted note, handled; the scenario, the note, the write.
fn rewritten() -> (Scenario, u32, u32) {
    let mut s = Scenario::new(COSTS);
    let at = s.quote("have doubled since");
    let x = s.comment(at, None, Some("say the cost"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(COSTS_NEW);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    (s, x, ext)
}

#[test]
fn a_note_whose_change_a_later_turn_removed_is_lost() {
    let (mut s, x, _) = rewritten();
    s.note("cut it");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.external("# T\n\nEnd.\n");
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert!(lost(&v).contains(&u64::from(x)), "{}", v["lost"]);
    assert!(item(&v, x)["at"].get("change").is_none());
}

#[test]
fn a_note_whose_change_was_undone_names_no_change() {
    let (mut s, x, ext) = rewritten();
    s.undo(&id(ext, 0), COSTS);
    let v = wire(&s.fold());
    assert_ne!(item(&v, x)["at"]["status"], "orphaned", "setup");
    assert!(item(&v, x)["at"].get("change").is_none());
    assert!(lost(&v).is_empty());
}

/// Rewritten from outside a turn, the note has no outcome to sit at.
#[test]
fn a_note_whose_text_an_outside_write_rewrote_stays_lost() {
    let mut s = Scenario::new(COSTS);
    let at = s.quote("have doubled since");
    let x = s.comment(at, None, Some("say the cost"));
    s.submit();
    s.external(COSTS_NEW);
    let v = wire(&s.fold());
    assert_eq!(item(&v, x)["at"]["status"], "orphaned", "setup");
    assert!(item(&v, x)["at"].get("change").is_none());
    assert_eq!(lost(&v), [u64::from(x)]);
}

const WHY: &str = "# T\n\n## Why\n\nCosts have doubled since the CDN migration.\n\n## Plan\n\nShip it.\n\nWatch it.\n\nWrite it up.\n\nRest.\n";
const COST: &str = "Costs have doubled since the CDN migration.";

/// A lost note says where it was: the section that held it on the text it was made on.
#[test]
fn a_lost_note_names_the_section_it_was_made_in() {
    // name, the write, the anchor (quote or section heading), the section it was in
    let cases = [
        (
            "its words went",
            WHY.replace(COST, "Egress spend rose."),
            "quote",
            "Why",
        ),
        (
            "its whole section went",
            WHY.replace(&format!("## Why\n\n{COST}\n\n"), ""),
            "quote",
            "Why",
        ),
        (
            "its own heading went",
            WHY.replace("## Why\n\n", ""),
            "section",
            "Why",
        ),
    ];
    for (name, write, kind, want) in cases {
        let mut s = Scenario::new(WHY);
        let at = if kind == "quote" {
            s.quote("have doubled since")
        } else {
            s.section("## Why")
        };
        let x = s.comment(at, None, Some("say the cost"));
        s.submit();
        s.external(&write);
        let v = wire(&s.fold());
        assert_eq!(
            lost(&v),
            [u64::from(x)],
            "{name}: setup {}",
            item(&v, x)["at"]
        );
        assert_eq!(item(&v, x)["at"]["section"], want, "{name}");
    }
}

/// The section a lost note names is the one on the text it was made on, not where it last stood.
#[test]
fn a_lost_note_names_its_section_as_made_even_after_a_rename() {
    let mut s = Scenario::new(WHY);
    let at = s.quote("have doubled since");
    let x = s.comment(at, None, Some("say the cost"));
    s.submit();
    let renamed = WHY.replace("## Why", "## Reasons");
    s.external(&renamed);
    assert_eq!(
        item(&wire(&s.fold()), x)["at"]["section"],
        "Reasons",
        "setup"
    );
    s.external(&renamed.replace(COST, "Egress rose."));
    let v = wire(&s.fold());
    assert_eq!(lost(&v), [u64::from(x)], "setup");
    assert_eq!(item(&v, x)["at"]["section"], "Why");
}

#[test]
fn a_lost_note_outside_any_section_names_none_and_a_nested_one_names_its_own() {
    let doc = "Preamble words here.\n\n# T\n\n## Why\n\n### Sub\n\nDeep words here.\n";
    let mut s = Scenario::new(doc);
    let pre = s.quote("Preamble words");
    let pre = s.comment(pre, None, Some("a"));
    let deep = s.quote("Deep words");
    let deep = s.comment(deep, None, Some("b"));
    s.submit();
    s.external("Other.\n\n# T\n\n## Why\n\n### Sub\n\nShallow.\n");
    let v = wire(&s.fold());
    assert_eq!(lost(&v), [u64::from(pre), u64::from(deep)], "setup");
    assert!(
        item(&v, pre)["at"].get("section").is_none(),
        "{}",
        item(&v, pre)["at"]
    );
    assert_eq!(item(&v, deep)["at"]["section"], "Sub");
}

#[test]
fn a_lost_note_names_a_marked_up_heading_as_a_placed_one_did() {
    let doc = WHY.replace("## Why", "## Why *now*");
    let mut s = Scenario::new(&doc);
    let at = s.quote("have doubled since");
    let x = s.comment(at, None, Some("say the cost"));
    s.submit();
    let placed = item(&wire(&s.fold()), x)["at"]["section"].clone();
    assert!(placed.is_string(), "setup: {placed}");
    s.external(&doc.replace(COST, "Egress rose."));
    let v = wire(&s.fold());
    assert_eq!(lost(&v), [u64::from(x)], "setup");
    assert_eq!(item(&v, x)["at"]["section"], placed);
}

/// A note still in place keeps the section it is in now, not the one it was made in.
#[test]
fn a_placed_note_names_the_section_it_is_in_now() {
    let mut s = Scenario::new(WHY);
    let at = s.quote("have doubled since");
    let x = s.comment(at, None, Some("say the cost"));
    s.submit();
    s.external(&WHY.replace("## Why", "## Reasons"));
    let v = wire(&s.fold());
    assert_ne!(item(&v, x)["at"]["status"], "orphaned", "setup");
    assert_eq!(item(&v, x)["at"]["section"], "Reasons");
}

#[test]
fn a_covers_quote_across_two_pieces_credits_and_cites_both() {
    let base = "# T\n\n## Old heading\nBody text here.\n\nEnd.\n";
    let mut s = Scenario::new(base);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external("# T\n\n## New heading\nBody text changed.\n\nEnd.\n");
    s.done(CLAUDE, x, turn);
    s.covers(CLAUDE, x, "## New heading\nBody text changed.", turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(
        item(&v, x)["outcome"]["changes"],
        json!([id(ext, 0), id(ext, 1)])
    );
    for c in of_write(&v, ext) {
        assert_eq!(c["asked"]["kind"], "cited", "{c}");
    }
}

#[test]
fn covers_credit_is_by_place_in_the_text_and_listed_once_in_doc_order() {
    // name, covers quotes, the indexes into the write it credits
    let cases: [(&str, &[&str], &[u32]); 3] = [
        (
            "straddling unchanged and changed text",
            &["Middle.\n\nSecond paragraph, new."],
            &[1],
        ),
        (
            "cited last-first",
            &["Second paragraph, new.", "First paragraph, new."],
            &[0, 1],
        ),
        (
            "cited twice",
            &["First paragraph, new.", "paragraph, new."],
            &[0, 1],
        ),
    ];
    let mut bad = Vec::new();
    for (name, quotes, want) in cases {
        let mut s = Scenario::new(TWO);
        let x = s.note("tighten it");
        s.submit();
        let turn = s.claim(CLAUDE);
        let ext = s.external(TWO_NEW);
        for q in quotes {
            s.covers(CLAUDE, x, q, turn);
        }
        s.yield_turn(CLAUDE, turn);
        let got = item(&wire(&s.fold()), x)["outcome"]["changes"].clone();
        let want: Vec<String> = want.iter().map(|&j| id(ext, j)).collect();
        if got != json!(want) {
            bad.push(format!("{name}: {got}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn a_quote_first_found_in_untouched_text_still_credits_where_it_was_written() {
    let mut s = Scenario::new("# T\n\nSame words.\n\nOld para.\n");
    let x = s.note("repeat it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external("# T\n\nSame words.\n\nSame words.\n");
    s.covers(CLAUDE, x, "Same words.", turn);
    s.yield_turn(CLAUDE, turn);
    assert_eq!(
        item(&wire(&s.fold()), x)["outcome"]["changes"],
        json!([id(ext, 0)])
    );
}

#[test]
fn covers_bind_the_turns_own_writes_whatever_came_after() {
    // Two writes in one turn; the quote is in the first, which the second left alone
    let mut s = Scenario::new(TWO);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let first = s.external(&TWO.replace("First paragraph, old.", "First paragraph, new."));
    s.external(TWO_NEW);
    s.covers(CLAUDE, x, "First paragraph, new.", turn);
    s.yield_turn(CLAUDE, turn);
    assert_eq!(
        item(&wire(&s.fold()), x)["outcome"]["changes"],
        json!([id(first, 0)])
    );

    // A later turn rewrote the cited text
    let mut s = Scenario::new(TWO);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(TWO_NEW);
    s.covers(CLAUDE, x, "First paragraph, new.", turn);
    s.yield_turn(CLAUDE, turn);
    s.note("again");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.external(&TWO_NEW.replace("First paragraph, new.", "First paragraph, newer."));
    s.yield_turn(CLAUDE, turn);
    assert_eq!(
        item(&wire(&s.fold()), x)["outcome"]["changes"],
        json!([id(ext, 0)])
    );
}

#[test]
fn an_added_list_item_and_a_ticked_box_read_as_what_they_are() {
    let cases = [
        (
            "# T\n\n- one\n- two\n",
            "# T\n\n- one\n- one and a half\n- two\n",
            "list item added",
        ),
        (
            "# T\n\n- [ ] ship it\n- other\n",
            "# T\n\n- [x] ship it\n- other\n",
            "formatting changed",
        ),
    ];
    let mut bad = Vec::new();
    for (base, new, want) in cases {
        let (s, ext) = agent_writes(base, new);
        let mine = of_write(&wire(&s.fold()), ext);
        let got: Vec<&Value> = mine.iter().map(|c| &c["label"]).collect();
        if got != [want] {
            bad.push(format!("{base:?}: {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

// --- removals split at headings ---

/// The journey's draft and Claude's first write: the `Log each retry` bullet and the
/// `## Stacked on` section are cut together in one plain hunk.
const JOURNEY: &str = include_str!("../e2e/fixtures/review/journey/pr-draft.md");
const JOURNEY_ROUND1: &str = include_str!("../e2e/fixtures/review/journey/round1.md");

/// The block deletions of the write on line `op`: `(label, first_line)`.
fn removals(v: &Value, op: u32) -> Vec<(Value, Value)> {
    of_write(v, op)
        .iter()
        .filter_map(|c| {
            let d = v["deletions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["change"] == c["id"] && d["kind"] == "block")?;
            Some((d["label"].clone(), d["first_line"].clone()))
        })
        .collect()
}

#[test]
fn the_cut_bullet_and_the_cut_section_record_as_two_removals() {
    let mut s = Scenario::drafted_by(JOURNEY, CLAUDE);
    let stacked = s.comment(s.section("## Stacked on"), Some("no"), None);
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external(JOURNEY_ROUND1);
    s.done(CLAUDE, stacked, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let olds: Vec<Value> = of_write(&v, ext).iter().map(|c| c["old"].clone()).collect();
    assert_eq!(
        olds,
        [
            json!([5, 9]),
            json!([13, 14]),
            json!([15, 15]),
            json!([16, 16]),
            // The cut runs right before the heading line
            json!([17, 18]),
            json!([19, 22]),
            json!([26, 27]),
        ]
    );
    assert_eq!(
        removals(&v, ext),
        [
            (
                json!("list item"),
                json!("- Respect `Retry-After` when the server sends it")
            ),
            (
                json!("list item"),
                json!("- Log each retry at `debug` with attempt number and delay")
            ),
            (json!("section · 3 blocks"), json!("## Stacked on")),
        ]
    );
    // The `no` on the section asked for the section's removal only
    let section = id(ext, 5);
    assert_eq!(change(&v, &section)["asked"]["notes"], json!([stacked]));
    assert_ne!(change(&v, &id(ext, 4))["asked"]["kind"], "location");
    assert_eq!(item(&v, stacked)["outcome"]["changes"], json!([section]));
    assert_eq!(item(&v, stacked)["at"]["change"], section);
    let d = deletion(&v, &section);
    assert_eq!(
        d["head"].as_str().map(str::trim_end),
        Some("## Stacked on\n\n- #482 — extract `UploadClient` from the sync worker")
    );
    assert_eq!(d["more"], 1);
    let bullet = deletion(&v, &id(ext, 4));
    assert_eq!(
        bullet["head"].as_str().map(str::trim_end),
        Some("- Log each retry at `debug` with attempt number and delay")
    );
    assert_eq!(bullet["more"], 0);
}

/// A write whose only hunk is a pure removal is cut too, not only a hunk that also adds.
#[test]
fn a_pure_removal_across_a_heading_records_as_one_hunk_per_section() {
    let cases = [
        (
            "a list item, then a section",
            "# T\n\n- keep\n- gone item\n\n## Gone\n\nGone para.\n\n## Kept\n\nText.\n",
            "# T\n\n- keep\n\n## Kept\n\nText.\n",
            vec![
                (json!("list item"), json!("- gone item")),
                (json!("section · 2 blocks"), json!("## Gone")),
            ],
        ),
        (
            "a paragraph, then a setext section",
            "# T\n\nKeep.\n\nGone para.\n\nSection\n-------\n\nGone too.\n\n# End\n",
            "# T\n\nKeep.\n\n# End\n",
            vec![
                (json!("paragraph"), json!("Gone para.")),
                (json!("section · 2 blocks"), json!("Section")),
            ],
        ),
        (
            "two sections side by side",
            "# T\n\nKeep.\n\n## A\n\nPara a.\n\n## B\n\nPara b.\n\n## C\n\nText.\n",
            "# T\n\nKeep.\n\n## C\n\nText.\n",
            vec![
                (json!("section · 2 blocks"), json!("## A")),
                (json!("section · 2 blocks"), json!("## B")),
            ],
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new, want) in cases {
        let (s, ext) = agent_writes(base, new);
        assert!(
            hunk::diff(base, new)
                .iter()
                .all(|h| h.d.is_empty() || h.i.is_empty()),
            "setup: {name} is pure"
        );
        let got = removals(&wire(&s.fold()), ext);
        if got != want || patch_on(&s, ext).len() != want.len() {
            bad.push(format!("{name}: {got:?} {:?}", patch_on(&s, ext)));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Blank lines above a cut heading never make a piece of their own.
#[test]
fn a_section_cut_with_the_blank_line_above_it_stays_one_hunk() {
    let base = "# T\n\nKeep.\n\n## Gone\n\nGone text.\n";
    let new = "# T\n\nKeep.\n";
    assert_eq!(
        hunk::diff(base, new)[0].d[0],
        "\n",
        "setup: the hunk opens on a blank"
    );
    let (s, ext) = agent_writes(base, new);
    assert_eq!(patch_on(&s, ext).len(), 1, "{:?}", patch_on(&s, ext));
    assert_eq!(
        removals(&wire(&s.fold()), ext),
        [(json!("section · 2 blocks"), json!("## Gone"))]
    );

    // The ux-b write cuts `## Stacked on` from under a blank line too
    let (before, after) = real_first_write();
    let s = Scenario::new(&before);
    let patch = Chain::build(&s.parsed())
        .record(&after)
        .and_then(|r| r.patch)
        .unwrap();
    let blank_only = |h: &&Hunk| h.d.iter().chain(&h.i).all(|l| l.trim().is_empty());
    assert!(
        !patch.iter().any(|h| blank_only(&h)),
        "{:?}",
        patch.iter().filter(blank_only).collect::<Vec<_>>()
    );
}

/// Passes on the tree before the split: a pure removal already in a log keeps its id,
/// its seen state and its range, whatever headings it holds.
#[test]
fn a_stored_removal_across_a_heading_keeps_its_id() {
    let base = "# T\n\n- keep\n- gone item\n\n## Gone\n\nGone para.\n\n## Kept\n\nText.\n";
    let new = "# T\n\n- keep\n\n## Kept\n\nText.\n";
    let mut s = Scenario::new(base);
    let turn = claimed(&mut s);
    let ext = s.external_diffed(new);
    s.yield_turn(CLAUDE, turn);
    assert_eq!(patch_on(&s, ext).len(), 1, "setup");
    s.seen(&[&id(ext, 0)]);
    let v = wire(&s.fold());
    assert_eq!(ids(&v), [id(ext, 0)]);
    let c = change(&v, &id(ext, 0));
    assert_eq!(c["seen"], true);
    assert_eq!(c["old"], json!([4, 8]));

    // The journey's merged hunk, stored whole, keeps its id too
    let mut s = Scenario::drafted_by(JOURNEY, CLAUDE);
    let turn = claimed(&mut s);
    let ext = s.external_diffed(JOURNEY_ROUND1);
    s.yield_turn(CLAUDE, turn);
    s.seen(&[&id(ext, 1)]);
    let v = wire(&s.fold());
    assert_eq!(ids(&v), [id(ext, 0), id(ext, 1), id(ext, 2)]);
    let c = change(&v, &id(ext, 1));
    assert_eq!(c["seen"], true);
    assert_eq!(c["old"], json!([13, 22]));
}

#[test]
fn a_removed_section_heads_its_row_and_counts_the_blocks_after() {
    let (s, ext) = agent_writes(
        "# T\n\n## Gone\n\n- a\n- b\n- c\n\n## Kept\n\nText.\n",
        "# T\n\n## Kept\n\nText.\n",
    );
    let v = wire(&s.fold());
    let d = deletion(&v, &id(ext, 0));
    assert_eq!(d["label"], "section · 4 blocks", "setup");
    assert_eq!(
        d["head"].as_str().map(str::trim_end),
        Some("## Gone\n\n- a")
    );
    assert_eq!(d["more"], 2);

    let (s, ext) = agent_writes("# T\n\nGone para.\n\nEnd.\n", "# T\n\nEnd.\n");
    let d = deletion(&wire(&s.fold()), &id(ext, 0));
    assert_eq!(d["head"].as_str().map(str::trim_end), Some("Gone para."));
    assert_eq!(d["more"], 0);
}

// --- adversary ---

/// A setext heading splits a removal that also sits in a hunk that adds, as `#` does.
#[test]
fn a_setext_heading_splits_a_removal_beside_an_edit() {
    let base = "# T\n\nThe old paragraph says what the client does today.\n\n- gone item\n\nSection\n=======\n\nGone text.\n\n# End\n";
    let new = "# T\n\nThe new paragraph says what the client does today.\n\n# End\n";
    let plain = hunk::diff(base, new);
    assert!(
        plain.len() == 1 && !plain[0].d.is_empty() && !plain[0].i.is_empty(),
        "setup: one mixed hunk"
    );
    let (s, ext) = agent_writes(base, new);
    assert_eq!(
        removals(&wire(&s.fold()), ext),
        [
            (json!("list item"), json!("- gone item")),
            (json!("section · 2 blocks"), json!("Section")),
        ],
        "{:?}",
        patch_on(&s, ext)
    );
}

/// Guard, passes now: a `#` line in a code block and a thematic break are not headings.
#[test]
fn a_removed_code_block_or_rule_is_not_cut_as_a_heading() {
    let cases = [
        (
            "a fenced `# install` line",
            "# T\n\nKeep.\n\nGone para.\n\n```sh\n# install\nmake\n```\n\n## Kept\n\nText.\n",
            "# T\n\nKeep.\n\n## Kept\n\nText.\n",
        ),
        (
            "a rule under a blank line",
            "# T\n\nKeep.\n\nGone para.\n\n---\n\nMore gone.\n\n## Kept\n\nText.\n",
            "# T\n\nKeep.\n\n## Kept\n\nText.\n",
        ),
    ];
    let mut bad = Vec::new();
    for (name, base, new) in cases {
        let (s, ext) = agent_writes(base, new);
        if patch_on(&s, ext).len() != 1 {
            bad.push(format!("{name}: {:?}", patch_on(&s, ext)));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Interpretation of `head`/`more`: with no heading, the head is the first removed
/// block and `more` counts the rest.
#[test]
fn a_removal_of_two_paragraphs_heads_with_the_first() {
    let (s, ext) = agent_writes("# T\n\nGone one.\n\nGone two.\n\nEnd.\n", "# T\n\nEnd.\n");
    let d = deletion(&wire(&s.fold()), &id(ext, 0));
    assert_eq!(
        d["head"].as_str().map(str::trim_end),
        Some("Gone one."),
        "{d}"
    );
    assert_eq!(d["more"], 1, "{d}");
}

/// A nested list item is part of its parent block, in the head and in `more` alike.
#[test]
fn a_removed_section_counts_a_nested_item_as_part_of_its_parent() {
    let (s, ext) = agent_writes(
        "# T\n\n## S\n\n- a\n  - a1\n- b\n\n## Kept\n\nText.\n",
        "# T\n\n## Kept\n\nText.\n",
    );
    let d = deletion(&wire(&s.fold()), &id(ext, 0));
    assert_eq!(
        d["head"].as_str().map(str::trim_end),
        Some("## S\n\n- a\n  - a1"),
        "{d}"
    );
    assert_eq!(d["more"], 1, "{d}");
}
