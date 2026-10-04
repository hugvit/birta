use birta::anchor;
use birta::rev::Rev;
use birta::review::doc::Doc;
use birta::review::fold;
use birta::review::hunk;
use birta::review::ops::ByKind;
use birta::review::state::{
    AnchorStatus, AskedKind, Attr, Change, DeletionKind, Item, ItemStatus, RangeStatus,
    ReversibleReason, ReviewState,
};

mod support {
    pub mod scenario;
}
use serde_json::json;
use support::scenario::{DOC, Scenario, agent};

const CLAUDE: &str = "claude";

fn id(op: u32, idx: u32) -> String {
    format!("{op}.{idx}")
}

fn ids(state: &ReviewState) -> Vec<&str> {
    state.changes.iter().map(|c| c.id.as_str()).collect()
}

fn change<'a>(state: &'a ReviewState, id: &str) -> &'a Change {
    state
        .changes
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no change {id} in {:?}", ids(state)))
}

fn item(state: &ReviewState, n: u32) -> &Item {
    state
        .items
        .iter()
        .find(|i| i.n == n)
        .unwrap_or_else(|| panic!("no item {n}"))
}

/// Bytes of lines `first..=last` of `text`, without the last one's terminator.
fn line_bytes(text: &str, first: u32, last: u32) -> (usize, usize) {
    let doc = Doc::new(text);
    (
        doc.lines.range(first).unwrap().start,
        doc.lines.range(last).unwrap().end,
    )
}

/// The sourcepos of the stop starting on `line` of `text`.
fn stop_sp(text: &str, line: u32) -> String {
    let doc = Doc::new(text);
    doc.blocks
        .stops
        .iter()
        .find(|s| s.lines.0 == line)
        .unwrap_or_else(|| panic!("no stop starts on line {line}"))
        .sp
        .clone()
}

/// A plain note on the doc, handed back and claimed, so writes after it are the agent's.
fn claimed(s: &mut Scenario) -> u32 {
    s.note("look it over");
    s.submit();
    s.claim(CLAUDE)
}

// --- asked (scenario 8) ---

#[test]
fn scenario_8_a_change_on_a_notes_block_is_asked_by_location() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- first item", "- first item, reworded");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();

    assert_eq!(ids(&state), [id(ext, 0)]);
    let c = change(&state, &id(ext, 0));
    assert_eq!((c.op, c.idx), (ext, 0));
    assert_eq!(c.asked.kind, AskedKind::Location);
    assert_eq!(c.asked.notes, [x]);
    assert_eq!(c.attr, Attr::Agent);
    assert_eq!(c.turn, Some(turn));
    assert_eq!(c.by.kind, ByKind::Agent);
    assert_eq!(c.by.name.as_deref(), Some(CLAUDE));
    assert_eq!(c.cause, None);
    assert_eq!((c.old, c.new), ((6, 6), (6, 6)));
    assert_eq!(c.range.status, RangeStatus::Ok);
    assert_eq!(c.range.lines, (6, 6));
    assert_eq!(c.range.bytes, line_bytes(s.text(), 6, 6));
    assert_eq!(c.range.sp, Some(stop_sp(s.text(), 6)));
    assert_eq!(c.op_hunks, 1);
    assert!(!c.seen && !c.settled && !c.is_rewrite);
    assert_eq!(c.undone_by, None);
    assert_eq!(state.counts.unasked, 0);
    assert_eq!(state.phrase.replied.as_ref().unwrap().unasked, 0);
}

#[test]
fn scenario_8_a_heading_verdict_asks_for_changes_anywhere_in_its_section() {
    let mut s = Scenario::new(DOC);
    let at = s.section("## Details");
    let x = s.comment(at, Some("no"), Some("redo this part"));
    s.submit();
    let turn = s.claim(CLAUDE);
    // One write, two hunks: one inside `## Details`, one in the sibling `## More`
    let text = s
        .text()
        .replace("- detail two\n", "- detail two, redone\n")
        .replace("- more item\n", "- more item, redone\n");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();

    let inside = change(&state, &id(ext, 0));
    assert_eq!(inside.asked.kind, AskedKind::Location);
    assert_eq!(inside.asked.notes, [x]);
    let outside = change(&state, &id(ext, 1));
    assert_eq!(outside.asked.kind, AskedKind::Unasked);
    assert!(outside.asked.notes.is_empty());
    assert_eq!((inside.op_hunks, outside.op_hunks), (2, 2));
    assert_eq!(state.counts.unasked, 1);
    assert_eq!(state.phrase.replied.as_ref().unwrap().unasked, 1);
}

#[test]
fn scenario_8_a_covers_citation_makes_a_change_cited() {
    let mut s = Scenario::new(DOC);
    let x = s.note("add a fourth item");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- third item", "- third item\n- fourth item");
    // Whitespace runs collapse when matching the cited text
    s.covers(CLAUDE, x, "fourth   item", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.asked.kind, AskedKind::Cited);
    assert_eq!(c.asked.notes, [x]);
    assert_eq!(state.counts.unasked, 0);
}

#[test]
fn covers_citing_text_that_exists_but_was_not_changed_in_the_turn_does_not_bind() {
    let mut s = Scenario::new(DOC);
    let x = s.note("add a fourth item");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- third item", "- third item\n- fourth item");
    // In the doc, but no hunk of this turn wrote it
    s.covers(CLAUDE, x, "Intro paragraph", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.asked.kind, AskedKind::Unasked);
    assert!(c.asked.notes.is_empty());
    assert_eq!(state.counts.unasked, 1);
}

#[test]
fn a_covers_in_a_later_turn_does_not_cite_an_earlier_change() {
    let mut s = Scenario::new(DOC);
    let x = s.note("add a fourth item");
    s.submit();
    let first = s.claim(CLAUDE);
    let ext = s.replace_line("- third item", "- third item\n- fourth item");
    s.yield_turn(CLAUDE, first);
    s.note("and now?");
    s.submit();
    let second = s.claim(CLAUDE);
    s.covers(CLAUDE, x, "fourth item", second);
    s.yield_turn(CLAUDE, second);
    let state = s.fold();
    assert_eq!(change(&state, &id(ext, 0)).asked.kind, AskedKind::Unasked);
}

#[test]
fn scenario_8_a_change_nobody_asked_for_is_unasked() {
    let mut s = Scenario::new(DOC);
    s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- appendix item", "- appendix item, tidied");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.asked.kind, AskedKind::Unasked);
    assert!(c.asked.notes.is_empty());
    assert_eq!(state.counts.unasked, 1);
    assert_eq!(state.phrase.replied.as_ref().unwrap().unasked, 1);
}

#[test]
fn scenario_8_an_unasked_op_overrides_location() {
    let mut s = Scenario::new(DOC);
    s.verdict("- first item", "no");
    s.verdict("- third item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = s
        .text()
        .replace("- first item\n", "- first item, reworded\n")
        .replace("- third item\n", "- third item, reworded too\n");
    let ext = s.external(&text);
    s.unasked(CLAUDE, "reworded too", turn);
    // Text no hunk wrote flips nothing
    s.unasked(CLAUDE, "Intro paragraph", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(change(&state, &id(ext, 0)).asked.kind, AskedKind::Location);
    let flagged = change(&state, &id(ext, 1));
    assert_eq!(flagged.asked.kind, AskedKind::Unasked);
    assert!(flagged.asked.notes.is_empty());
    assert_eq!(state.counts.unasked, 1);
}

#[test]
fn scenario_8_a_whole_doc_note_does_not_make_a_change_asked() {
    let mut s = Scenario::new(DOC);
    let x = s.note("tighten everything");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- first item", "- first item, tighter");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(change(&state, &id(ext, 0)).asked.kind, AskedKind::Unasked);
    assert_eq!(item(&state, x).status, ItemStatus::NotHandled);
}

#[test]
fn a_note_an_earlier_turn_settled_does_not_ask_for_a_change() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let first = s.claim(CLAUDE);
    s.done(CLAUDE, x, first);
    s.yield_turn(CLAUDE, first);
    s.note("anything else?");
    s.submit();
    let second = s.claim(CLAUDE);
    let ext = s.replace_line("- first item", "- first item, late fix");
    s.yield_turn(CLAUDE, second);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.turn, Some(second));
    assert_eq!(c.asked.kind, AskedKind::Unasked);
}

/// A note its turn left open is carried to the next claim, which it then asks.
#[test]
fn a_note_carried_from_an_earlier_turn_asks_for_its_change() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let first = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, first);
    s.note("anything else?");
    s.submit();
    let second = s.claim(CLAUDE);
    let ext = s.replace_line("- first item", "- first item, late fix");
    s.yield_turn(CLAUDE, second);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.turn, Some(second));
    assert_eq!(
        (c.asked.kind, c.asked.notes.clone()),
        (AskedKind::Location, vec![x])
    );
}

// --- attribution (scenario 9) ---

#[test]
fn scenario_9_a_write_with_no_open_claim_is_from_outside() {
    let mut s = Scenario::new(DOC);
    let before = s.replace_line("- first item", "- first item, before");
    let turn = claimed(&mut s);
    s.yield_turn(CLAUDE, turn);
    let after = s.replace_line("- third item", "- third item, after");
    let git = s.external_git(&s.replaced("- more item", "- more item, pulled"));
    let state = s.fold();
    for (n, cause) in [(before, None), (after, None), (git, Some("git"))] {
        let c = change(&state, &id(n, 0));
        assert_eq!(c.attr, Attr::Outside, "{n}");
        assert_eq!(c.asked.kind, AskedKind::Outside, "{n}");
        assert!(c.asked.notes.is_empty(), "{n}");
        assert_eq!(c.turn, None, "{n}");
        assert_eq!(c.cause.as_deref(), cause, "{n}");
    }
}

#[test]
fn scenario_9_with_two_open_claims_the_latest_takes_the_write() {
    let mut s = Scenario::new(DOC);
    s.verdict("- first item", "no");
    s.submit();
    s.claim(CLAUDE);
    s.verdict("- third item", "no");
    s.submit();
    let second = s.claim("other");
    let ext = s.replace_line("- appendix item", "- appendix item, whose?");
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.attr, Attr::Agent);
    assert_eq!(c.turn, Some(second));
    assert_eq!(c.by.name.as_deref(), Some("other"));
}

#[test]
fn scenario_9_birtas_own_edit_is_the_humans_even_during_a_claim() {
    let mut s = Scenario::new(DOC);
    claimed(&mut s);
    let text = s.replaced("- first item", "- first item, mine");
    let edit = s.edit(&text);
    let state = s.fold();
    let c = change(&state, &id(edit, 0));
    assert_eq!(c.attr, Attr::Human);
    assert_eq!(c.asked.kind, AskedKind::Human);
    assert_eq!(c.by.kind, ByKind::Human);
    assert_eq!(c.turn, None);
}

#[test]
fn unseen_counts_agent_and_outside_changes_but_not_your_own() {
    let mut s = Scenario::new(DOC);
    let outside = s.replace_line("- appendix item", "- appendix item, outside");
    let turn = claimed(&mut s);
    let agent = s.replace_line("- first item", "- first item, agent");
    let mine = s.edit(&s.replaced("- third item", "- third item, mine"));
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(state.changes.len(), 3, "{:?}", ids(&state));
    assert_eq!(state.counts.unseen, 2);
    // The doc note is not handled by a change: its row needs attention too
    assert_eq!(state.counts.not_handled, 1);
    assert_eq!(state.counts.attention, 3);

    let seen = [id(outside, 0), id(agent, 0), id(mine, 0)];
    s.seen(&seen.iter().map(String::as_str).collect::<Vec<_>>());
    let state = s.fold();
    assert_eq!(state.counts.unseen, 0);
    assert_eq!(state.counts.attention, 1);
}

// --- your edits and stet (scenario 10) ---

/// Your edit of the first item, then a note sent and claimed.
fn after_your_edit() -> (Scenario, u32, u32) {
    let mut s = Scenario::new(DOC);
    let edit = s.edit(&s.replaced("- first item", "- first item, mine"));
    let turn = claimed(&mut s);
    (s, edit, turn)
}

#[test]
fn scenario_10_an_agent_change_to_your_edit_is_flagged() {
    let (mut s, edit, turn) = after_your_edit();
    let ext = s.replace_line("- first item, mine", "- first item, theirs");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.changes_your_edit, Some(id(edit, 0)));
    assert!(!c.reverts_your_edit);
}

#[test]
fn scenario_10_an_agent_change_that_undoes_your_edit_reverts_it() {
    let (mut s, edit, turn) = after_your_edit();
    let ext = s.replace_line("- first item, mine", "- first item");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.changes_your_edit, Some(id(edit, 0)));
    assert!(c.reverts_your_edit);
}

#[test]
fn an_agent_change_elsewhere_leaves_your_edit_alone() {
    let (mut s, _, turn) = after_your_edit();
    let ext = s.replace_line("- third item", "- third item, theirs");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.changes_your_edit, None);
    assert!(!c.reverts_your_edit);
}

#[test]
fn an_edit_before_the_last_approve_is_not_flagged() {
    let mut s = Scenario::new(DOC);
    s.edit(&s.replaced("- first item", "- first item, mine"));
    s.approve();
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item, mine", "- first item, theirs");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(change(&state, &id(ext, 0)).changes_your_edit, None);
}

#[test]
fn scenario_10_a_change_on_a_sent_stet_breaks_it_and_a_draft_stet_does_not() {
    let mut s = Scenario::new(DOC);
    let sent = s.verdict("- first item", "stet");
    s.submit();
    // Made after the hand-back, so the agent never saw it
    s.verdict("- third item", "stet");
    let turn = s.claim(CLAUDE);
    let text = s
        .text()
        .replace("- first item\n", "- first item, changed\n")
        .replace("- third item\n", "- third item, changed\n");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(change(&state, &id(ext, 0)).breaks_stet, [sent]);
    assert!(change(&state, &id(ext, 1)).breaks_stet.is_empty());
}

// --- seen, settled, carry-forward and undo (scenario 11) ---

/// One agent change in a finished turn.
fn one_change() -> (Scenario, String) {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item", "- first item, reworded");
    s.yield_turn(CLAUDE, turn);
    (s, id(ext, 0))
}

#[test]
fn scenario_11_an_unseen_change_carries_across_a_new_hand_back() {
    let (mut s, c) = one_change();
    s.note("next round");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let ch = change(&state, &c);
    assert!(!ch.seen && !ch.settled);
    assert_eq!(state.counts.unseen, 1);
}

#[test]
fn scenario_11_seen_marks_a_change_and_unknown_ids_are_ignored() {
    let (mut s, c) = one_change();
    s.seen(&[c.as_str(), "99.0", "#99", "nh:99", "garbage"]);
    let state = s.fold();
    let ch = change(&state, &c);
    assert!(ch.seen);
    assert!(!ch.settled, "no hand-back since it was seen");
    assert_eq!(state.counts.unseen, 0);
    assert!(state.warnings.is_empty(), "{:?}", state.warnings);
}

#[test]
fn scenario_11_a_seen_change_settles_at_the_next_hand_back_or_approve() {
    let (mut s, c) = one_change();
    s.seen(&[c.as_str()]);
    s.note("next round");
    s.submit();
    let ch = change(&s.fold(), &c).clone();
    assert!(ch.seen && ch.settled);

    let (mut s, c) = one_change();
    s.seen(&[c.as_str()]);
    s.approve();
    assert!(change(&s.fold(), &c).settled);
}

#[test]
fn a_hand_back_before_the_change_was_seen_does_not_settle_it() {
    let (mut s, c) = one_change();
    s.note("next round");
    s.submit();
    s.seen(&[c.as_str()]);
    let ch = change(&s.fold(), &c).clone();
    assert!(ch.seen);
    assert!(!ch.settled);
}

#[test]
fn scenario_11_an_unseen_change_never_settles() {
    let (mut s, c) = one_change();
    s.approve();
    let state = s.fold();
    let ch = change(&state, &c);
    assert!(!ch.seen && !ch.settled);
    assert_eq!(state.counts.unseen, 1);
}

#[test]
fn scenario_11_an_undo_settles_the_change_it_reverts() {
    let (mut s, c) = one_change();
    let before = s.replaced("- first item, reworded", "- first item");
    let undo = s.undo(&c, &before);
    let state = s.fold();
    let ch = change(&state, &c);
    assert_eq!(ch.undone_by, Some(undo));
    assert!(ch.settled);
    assert_eq!(state.counts.unseen, 0);
    let u = change(&state, &id(undo, 0));
    assert_eq!(u.attr, Attr::Human);
}

// --- deletions and superseded changes (scenario 12) ---

#[test]
fn scenario_12_a_deleted_block_leaves_a_tick_below_the_stop_above() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let removed = "- second item\n  - nested item\n";
    let text = s.text().replace(removed, "");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();

    let c = change(&state, &id(ext, 0));
    assert_eq!((c.what.old_lines, c.what.new_lines), (2, 0));
    assert_eq!(c.what.words, None);
    assert_eq!(c.removed.as_deref(), Some(removed));
    assert_eq!(state.deletions.len(), 1, "{:?}", state.deletions);
    let d = &state.deletions[0];
    assert_eq!(d.change, id(ext, 0));
    assert_eq!(d.kind, DeletionKind::Block);
    // `- third item` now follows the gap, on line 7
    assert_eq!(d.at_line, 7);
    assert_eq!(d.at_byte, Some(line_bytes(s.text(), 7, 7).0));
    assert_eq!(d.removed, removed);
    assert_eq!(d.after_sp, Some(stop_sp(s.text(), 6)));
}

#[test]
fn a_deletion_at_the_top_has_no_stop_above() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let text = s.text().replacen("# Plan\n\n", "", 1);
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let d = &state.deletions[0];
    assert_eq!(d.change, id(ext, 0));
    assert_eq!(d.kind, DeletionKind::Block);
    assert_eq!(d.at_line, 1);
    assert_eq!(d.at_byte, Some(0));
    assert_eq!(d.after_sp, None);
}

#[test]
fn scenario_12_a_word_deletion_leaves_a_caret() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item", "- item");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.what.words, Some(1));
    assert_eq!(state.deletions.len(), 1, "{:?}", state.deletions);
    let d = &state.deletions[0];
    assert_eq!(d.change, id(ext, 0));
    assert_eq!(d.kind, DeletionKind::Words);
    assert_eq!(d.at_line, 6);
    let (start, end) = line_bytes(s.text(), 6, 6);
    let at = d.at_byte.expect("the caret's byte");
    assert!((start..=end).contains(&at), "{at} not in {start}..={end}");
    assert_eq!(d.removed.trim(), "first");
}

#[test]
fn a_word_caret_under_a_later_change_has_no_byte() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item", "- item");
    s.replace_line("- item", "- item!");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let d = state
        .deletions
        .iter()
        .find(|d| d.change == id(ext, 0))
        .expect("the word deletion stays");
    assert_eq!(d.kind, DeletionKind::Words);
    assert_eq!(d.at_byte, None);
}

#[test]
fn scenario_12_a_change_written_over_later_is_superseded_not_dropped() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let first = s.replace_line("- third item", "- third item\n- fourth item");
    let second = s.replace_line("- fourth item", "- fifth item");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let over = change(&state, &id(first, 0));
    assert_eq!(over.range.status, RangeStatus::Superseded);
    assert!(!over.seen);
    assert_eq!(change(&state, &id(second, 0)).range.status, RangeStatus::Ok);
    assert_eq!(state.counts.unseen, 2);
}

#[test]
fn a_change_pushed_down_by_a_later_insert_is_moved() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- third item", "- third item, reworded");
    let text = format!("> draft\n\n{}", s.text());
    s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.range.status, RangeStatus::Moved);
    assert_eq!(c.range.lines, (11, 11));
    assert_eq!(c.range.bytes, line_bytes(s.text(), 11, 11));
    assert_eq!(c.range.sp, Some(stop_sp(s.text(), 11)));
    // Old and new are in the texts the write went between
    assert_eq!((c.old, c.new), ((9, 9), (9, 9)));
}

// --- what the receipt says ---

#[test]
fn a_change_in_raw_html_touches_html() {
    let doc = "# T\n\n<div>\nhidden\n</div>\n\n<!-- a note -->\n\nplain\n";
    let mut s = Scenario::new(doc);
    let turn = claimed(&mut s);
    let text = s
        .text()
        .replace("hidden\n", "still hidden\n")
        .replace("<!-- a note -->\n", "<!-- another note -->\n")
        .replace("plain\n", "plain text\n");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let touches: Vec<bool> = (0..3)
        .map(|i| change(&state, &id(ext, i)).what.touches_html)
        .collect();
    assert_eq!(touches, [true, true, false]);
}

#[test]
fn only_a_small_edit_counts_words() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let small = s.replace_line("- first item", "- last item");
    let big = s.replace_line(
        "Intro paragraph here,",
        "An intro that says five new things,",
    );
    let text = s
        .text()
        .replace("- detail one\n- detail two\n", "- d1\n- d2\n- d3\n- d4\n");
    let long = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(change(&state, &id(small, 0)).what.words, Some(1));
    assert_eq!(change(&state, &id(big, 0)).what.words, None);
    assert_eq!(change(&state, &id(long, 0)).what.words, None);
}

#[test]
fn a_pure_insert_removes_nothing() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- third item", "- third item\n- fourth item");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.removed, None);
    assert_eq!((c.what.old_lines, c.what.new_lines), (0, 1));
    assert!(state.deletions.is_empty());
}

#[test]
fn a_removed_excerpt_is_capped() {
    let long = "word ".repeat(1_000);
    let doc = format!("# T\n\n{long}\n\nend\n");
    let mut s = Scenario::new(&doc);
    let turn = claimed(&mut s);
    let ext = s.external("# T\n\nend\n");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let removed = change(&state, &id(ext, 0)).removed.clone().unwrap();
    assert_eq!(removed.chars().count(), 2_000);
    assert!(long.starts_with(&removed));
}

// --- rewrite (scenario 13's threshold) ---

/// The ids of the news items of `kind`, in list order.
fn news_of(state: &ReviewState, kind: &str) -> Vec<String> {
    let v = serde_json::to_value(state).unwrap();
    v["news"]
        .as_array()
        .unwrap_or_else(|| panic!("no news list"))
        .iter()
        .filter(|n| n["kind"] == kind)
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect()
}

/// 90 one-line paragraphs, blank lines between.
fn paragraphs() -> String {
    (1..=90)
        .map(|i| format!("p{i} text\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A turn that edits paragraphs 1..=`edited` and grows the last one into 11 paragraphs,
/// so the doc ends with 100 blocks of which `edited + 11` were touched. The last hunk is
/// recorded as the edit and the ten added paragraphs.
fn grown(edited: usize) -> (Scenario, u32, u32) {
    let mut s = Scenario::new(&paragraphs());
    let turn = claimed(&mut s);
    let mut text = s.text().to_string();
    for i in 1..=edited {
        text = text.replacen(&format!("p{i} text\n"), &format!("p{i} edited\n"), 1);
    }
    let added: String = (1..=10).map(|k| format!("\nq{k} new\n")).collect();
    text = text.replacen("p90 text\n", &format!("p90 edited\n{added}"), 1);
    assert_eq!(hunk::diff(s.text(), &text).len(), edited + 1, "setup");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    (s, turn, ext)
}

#[test]
fn a_turn_touching_39_percent_of_a_grown_doc_is_no_rewrite() {
    let (s, _, _) = grown(28);
    let state = s.fold();
    assert!(state.rewrite.is_none(), "{:?}", state.rewrite);
    assert!(state.changes.iter().all(|c| !c.is_rewrite));
}

#[test]
fn a_turn_touching_41_percent_of_a_grown_doc_is_a_rewrite() {
    let (s, turn, _) = grown(30);
    let state = s.fold();
    let r = state.rewrite.as_ref().expect("a rewrite");
    assert_eq!(r.turn, turn);
    assert_eq!((r.blocks_changed, r.blocks_total), (41, 100));
    assert!((r.pct - 0.41).abs() < 1e-9, "{}", r.pct);
    assert_eq!(state.changes.len(), 32);
    assert!(state.changes.iter().all(|c| c.is_rewrite));
    // Each change is news of its own all the same
    let all: Vec<String> = state.changes.iter().map(|c| c.id.clone()).collect();
    assert_eq!(news_of(&state, "change"), all);
}

#[test]
fn a_rewrite_is_over_once_its_changes_are_seen() {
    let (mut s, _, _) = grown(30);
    let all: Vec<String> = s.fold().changes.iter().map(|c| c.id.clone()).collect();
    assert_eq!(all.len(), 32);
    s.seen(&all.iter().map(String::as_str).collect::<Vec<_>>());
    let state = s.fold();
    assert!(state.rewrite.is_none());
    assert_eq!(state.counts.unseen, 0);
    let v = serde_json::to_value(&state).unwrap();
    for c in v["changes"].as_array().unwrap() {
        assert_eq!(c["check"], "kept", "{}", c["id"]);
    }
}

// --- undo (reversible) ---

#[test]
fn a_single_hunk_change_is_reversible_in_current_text_coordinates() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- third item", "- third item, reworded");
    let text = format!("> draft\n\n{}", s.text());
    s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert!(c.reversible.ok);
    assert_eq!(c.reversible.reason, ReversibleReason::None);
    assert_eq!(c.op_hunks, 1);
    let reverse = c.reverse.as_ref().expect("a reverse hunk");
    // Line 9 when written, line 11 now
    assert_eq!(reverse.a, 10);
    let undone = hunk::apply(s.text(), std::slice::from_ref(reverse)).unwrap();
    assert_eq!(undone, format!("> draft\n\n{DOC}"));
}

#[test]
fn each_hunk_of_a_multi_hunk_write_is_reversible_on_its_own() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let text = s
        .text()
        .replace("- first item\n", "- first item, a\n")
        .replace("- more item\n", "- more item, b\n");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    for i in 0..2 {
        let c = change(&state, &id(ext, i));
        assert_eq!(c.op_hunks, 2);
        assert!(c.reversible.ok);
        assert_eq!(c.reversible.reason, ReversibleReason::None);
        assert!(c.reverse.is_some());
    }
    // Undoing the second alone leaves the first in place
    let reverse = change(&state, &id(ext, 1)).reverse.clone().unwrap();
    let undone = hunk::apply(s.text(), &[reverse]).unwrap();
    assert_eq!(undone, DOC.replace("- first item\n", "- first item, a\n"));
}

#[test]
fn a_change_written_over_later_is_not_reversible() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let text = s
        .text()
        .replace("- first item\n", "- first item, a\n")
        .replace("- more item\n", "- more item, b\n");
    let ext = s.external(&text);
    s.replace_line("- more item, b", "- more item, c");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let over = change(&state, &id(ext, 1));
    assert!(!over.reversible.ok);
    assert_eq!(over.reversible.reason, ReversibleReason::LaterOverlap);
    assert!(over.reverse.is_none());
    // Its sibling hunk is untouched
    assert_eq!(
        change(&state, &id(ext, 0)).reversible.reason,
        ReversibleReason::None
    );
}

#[test]
fn an_outside_change_is_not_reversible() {
    let mut s = Scenario::new(DOC);
    let ext = s.replace_line("- first item", "- first item, outside");
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert!(!c.reversible.ok);
    assert_eq!(c.reversible.reason, ReversibleReason::Outside);
}

#[test]
fn a_change_from_a_bare_snap_is_not_reversible() {
    let mut s = Scenario::new(DOC);
    let snap = s.snap_to(&DOC.replace("- first item\n", "- first item, restored\n"));
    let state = s.fold();
    let c = change(&state, &id(snap, 0));
    assert_eq!(c.attr, Attr::Outside);
    assert!(!c.reversible.ok);
    assert_eq!(c.reversible.reason, ReversibleReason::Snap);
}

// --- the flagship cut (scenario 15b) ---

/// `## Details` (lines 11-19) cut to four lines in one hunk, on a section verdict.
fn cut_to_four(rev_only: bool) -> (Scenario, u32, u32) {
    let mut s = Scenario::new(DOC);
    let at = s.section("## Details");
    let x = s.comment(at, Some("rewrite"), Some("far too long"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let cut = s.text().replace(
        "## Details\n\n- detail one\n- detail two\n\n| key | value |\n|-----|-------|\n| a | 1 |\n| b | 2 |\n",
        "## In short\nOne line.\nTwo lines.\nThree.\n",
    );
    assert_eq!(hunk::diff(s.text(), &cut).len(), 1, "setup");
    let ext = if rev_only {
        s.external_rev_only(&cut)
    } else {
        s.external(&cut)
    };
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    (s, x, ext)
}

#[test]
fn scenario_15b_cutting_nine_lines_to_four_keeps_receipts_undo_and_seen() {
    let (mut s, x, ext) = cut_to_four(false);
    let state = s.fold();
    // Recorded cut where blocks meet: the heading, and the body that replaced the rest
    let both = [id(ext, 0), id(ext, 1)];
    assert_eq!(ids(&state), both);
    let (heading, body) = (change(&state, &both[0]), change(&state, &both[1]));
    assert_eq!((heading.old, heading.new), ((11, 12), (11, 11)));
    assert_eq!((body.old, body.new), ((13, 19), (12, 14)));
    let removed = format!(
        "{}{}",
        heading.removed.as_deref().unwrap(),
        body.removed.as_deref().unwrap()
    );
    assert_eq!(
        removed,
        "## Details\n\n- detail one\n- detail two\n\n| key | value |\n|-----|-------|\n| a | 1 |\n| b | 2 |\n"
    );
    for c in [heading, body] {
        assert_eq!(c.asked.kind, AskedKind::Location, "{}", c.id);
        assert_eq!(c.asked.notes, [x], "{}", c.id);
        assert!(c.reversible.ok, "{}", c.id);
    }
    // Each reverts alone; the body first, so the heading's lines stay where they are
    let undone = hunk::apply(s.text(), &[body.reverse.clone().unwrap()]).unwrap();
    let undone = hunk::apply(&undone, &[heading.reverse.clone().unwrap()]).unwrap();
    assert_eq!(undone, DOC);
    assert_eq!(item(&state, x).status, ItemStatus::Handled);
    assert_eq!(item(&state, x).outcome.as_ref().unwrap().changes, both);

    s.seen(&[both[0].as_str(), both[1].as_str()]);
    let state = s.fold();
    assert!(state.changes.iter().all(|c| c.seen));
    assert_eq!(state.counts.unseen, 0);
}

/// A rev-only write's hunks are derived when the log is read, so they are never cut and
/// equal the plain diff.
#[test]
fn scenario_15b_a_rev_only_write_folds_to_the_plain_diff() {
    let (rev_only, x, ext) = cut_to_four(true);
    let state = rev_only.fold();
    let plain = hunk::diff(DOC, rev_only.text());
    assert_eq!(ids(&state), [id(ext, 0)]);
    let c = change(&state, &id(ext, 0));
    assert_eq!((c.old, c.new), ((11, 19), (11, 14)));
    assert_eq!(c.reverse, Some(hunk::reverse(&plain)[0].clone()));
    assert_eq!(item(&state, x).status, ItemStatus::Handled);
}

#[test]
fn a_rev_only_write_in_the_turn_handles_the_note_on_its_block() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external_rev_only(&s.replaced("- first item", "- first item, reworded"));
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(item(&state, x).status, ItemStatus::Handled);
    assert_eq!(
        item(&state, x).outcome.as_ref().unwrap().changes,
        [id(ext, 0)]
    );
}

// --- one hunk doesn't disturb the others ---

/// Four notes answered in one turn; `extra` adds a write on text none of them is on.
fn answered(extra: Option<&dyn Fn(&Scenario) -> String>) -> ReviewState {
    let mut s = Scenario::new(DOC);
    s.verdict("- first item", "no");
    let quote = s.quote("Intro paragraph");
    let q = s.comment(quote, None, Some("wordy"));
    let more = s.section("## More");
    s.comment(more, Some("no"), Some("cut this"));
    let d = s.note("is the table needed?");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.replace_line("- first item", "- first item, reworded");
    s.agent_reply(CLAUDE, d, "answer", "yes");
    s.done(CLAUDE, q, turn);
    if let Some(extra) = extra {
        let text = extra(&s);
        s.external(&text);
    }
    s.yield_turn(CLAUDE, turn);
    s.fold()
}

#[test]
fn an_unrelated_hunk_leaves_other_notes_as_they_were() {
    let facts = |state: &ReviewState| {
        state
            .items
            .iter()
            .map(|i| {
                (
                    i.n,
                    i.status,
                    i.outcome.as_ref().map(|o| (o.done, o.changes.clone())),
                )
            })
            .collect::<Vec<_>>()
    };
    let base = answered(None);
    let below = answered(Some(&|s: &Scenario| {
        s.replaced("- appendix item", "- appendix item\n- appendix two")
    }));
    let above = answered(Some(&|s: &Scenario| format!("> draft\n\n{}", s.text())));
    assert_eq!(facts(&below), facts(&base));
    assert_eq!(facts(&above), facts(&base));
    let asked = |state: &ReviewState| {
        let c = &state.changes[0];
        (c.id.clone(), c.asked.kind, c.asked.notes.clone())
    };
    assert_eq!(asked(&below), asked(&base));
    assert_eq!(asked(&above), asked(&base));
}

// --- notes placed on the current text (pass 6) ---

#[test]
fn pass_6_a_quote_note_is_placed_on_the_current_text() {
    let mut s = Scenario::new(DOC);
    let at = s.quote("Intro paragraph");
    let x = s.comment(at, None, Some("wordy"));
    let state = s.fold();
    let at = &item(&state, x).at;
    let start = DOC.find("Intro paragraph").unwrap();
    let range = (start, start + "Intro paragraph".len());
    assert_eq!(at.status, Some(AnchorStatus::Ok));
    assert_eq!(at.lines, Some((3, 3)));
    assert_eq!(at.range, Some(range));
    assert_eq!(at.block, Some((3, 4)));
    assert_eq!(at.section.as_deref(), Some("Plan"));
    let want = anchor::locate(DOC, range.0..range.1).expect("locatable");
    let loc = at.loc.as_ref().expect("a loc");
    assert_eq!(loc.sp, want.sp);
    assert_eq!(loc.off, Some(want.off));
    assert_eq!(loc.tl, Some(want.tl));
    assert_eq!(state.lost, Vec::<u32>::new());
}

#[test]
fn pass_6_a_note_follows_its_text_down() {
    let mut s = Scenario::new(DOC);
    let at = s.quote("Intro paragraph");
    let x = s.comment(at, None, Some("wordy"));
    let text = format!("> draft\n\n{}", s.text());
    s.external(&text);
    let state = s.fold();
    let at = &item(&state, x).at;
    let start = text.find("Intro paragraph").unwrap();
    let range = (start, start + "Intro paragraph".len());
    assert_eq!(at.status, Some(AnchorStatus::Moved));
    assert_eq!(at.lines, Some((5, 5)));
    assert_eq!(at.range, Some(range));
    assert_eq!(at.block, Some((5, 6)));
    let want = anchor::locate(&text, range.0..range.1).expect("locatable");
    let loc = at.loc.as_ref().expect("a loc");
    assert_eq!(
        (loc.sp.as_str(), loc.off),
        (want.sp.as_str(), Some(want.off))
    );
}

#[test]
fn pass_6_a_block_verdict_is_placed_on_its_stop() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- second item", "no");
    let state = s.fold();
    let at = &item(&state, x).at;
    assert_eq!(at.status, Some(AnchorStatus::Ok));
    assert_eq!(at.lines, Some((7, 8)));
    assert_eq!(at.block, Some((7, 8)));
    assert_eq!(at.range, Some(line_bytes(DOC, 7, 8)));
    assert_eq!(at.section.as_deref(), Some("Plan"));
    let loc = at.loc.as_ref().expect("a loc");
    assert_eq!(loc.sp, stop_sp(DOC, 7));
    assert_eq!((loc.off, loc.tl), (None, None));
}

#[test]
fn pass_6_a_section_verdict_spans_its_section() {
    let mut s = Scenario::new(DOC);
    let at = s.section("## Details");
    let x = s.comment(at, Some("no"), None);
    let state = s.fold();
    let at = &item(&state, x).at;
    assert_eq!(at.status, Some(AnchorStatus::Ok));
    assert_eq!(at.lines, Some((11, 20)));
    assert_eq!(at.section.as_deref(), Some("Details"));
    assert_eq!(at.loc.as_ref().expect("a loc").sp, stop_sp(DOC, 11));
}

#[test]
fn pass_6_a_note_whose_text_is_gone_is_lost() {
    let mut s = Scenario::new(DOC);
    let at = s.quote("Intro paragraph here");
    let x = s.comment(at, None, Some("wordy"));
    let turn = claimed(&mut s);
    s.yield_turn(CLAUDE, turn);
    // A small write from outside, after the turn left the note unhandled
    let text = s
        .text()
        .replace("Intro paragraph here,\nwith a second line.\n\n", "");
    s.external(&text);
    let state = s.fold();
    assert_eq!(item(&state, x).status, ItemStatus::NotHandled, "setup");
    let at = &item(&state, x).at;
    assert_eq!(at.status, Some(AnchorStatus::Orphaned));
    assert!(at.lines.is_none() && at.range.is_none() && at.block.is_none());
    assert!(at.loc.is_none());
    assert_eq!(state.lost, [x]);
    assert_eq!(state.counts.lost, 1);
}

#[test]
fn a_retracted_note_is_never_lost() {
    let mut s = Scenario::new(DOC);
    let at = s.quote("Intro paragraph here");
    let x = s.comment(at, None, Some("wordy"));
    s.retract(x);
    let text = s
        .text()
        .replace("Intro paragraph here,\nwith a second line.\n\n", "");
    s.edit(&text);
    let state = s.fold();
    assert!(state.lost.is_empty());
    assert_eq!(state.counts.lost, 0);
}

#[test]
fn a_suspended_note_is_not_lost() {
    // A checkout that replaces most of the doc may be undone, so the note waits
    let mut s = Scenario::new(DOC);
    let at = s.quote("Intro paragraph here");
    let x = s.comment(at, None, Some("wordy"));
    s.external_git("# Other branch\n\nNothing alike.\n");
    let state = s.fold();
    assert_eq!(item(&state, x).at.status, Some(AnchorStatus::Suspended));
    assert!(state.lost.is_empty());
}

// --- determinism (scenario 18) ---

#[test]
fn changes_and_deletions_are_deterministic_and_ordered() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let text = s
        .text()
        .replace("- second item\n  - nested item\n", "")
        .replace("- more item\n", "- more item, b\n");
    s.external(&text);
    let text = s.text().replace("- detail one\n", "");
    s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let json = || serde_json::to_string(&s.fold()).unwrap();
    let first = json();
    // Folds on other threads start from other hash seeds
    let others: Vec<String> = (0..2)
        .map(|_| std::thread::scope(|t| t.spawn(json).join().unwrap()))
        .collect();
    assert!(others.iter().all(|o| *o == first));
    let state = s.fold();
    let keys: Vec<(u32, u32)> = state.changes.iter().map(|c| (c.op, c.idx)).collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "{keys:?}");
    assert_eq!(state.deletions.len(), 2);
    let order: Vec<u32> = state
        .deletions
        .iter()
        .map(|d| change(&state, &d.change).op)
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
}

// --- adversary killers ---

#[test]
fn undoing_a_later_hunk_alone_accounts_for_the_lines_its_sibling_added() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let grown = "- first item\n- inserted one\n- inserted two\n";
    let text = s
        .text()
        .replace("- first item\n", grown)
        .replace("- more item\n", "- more item, b\n");
    let hunks = hunk::diff(s.text(), &text);
    assert_eq!(hunks.len(), 2, "setup");
    assert!(hunks[0].d.is_empty() && hunks[0].i.len() == 2, "setup");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let want = hunk::reverse(&hunks);
    for i in 0..2u32 {
        let c = change(&state, &id(ext, i));
        assert_eq!(c.reverse.as_ref(), Some(&want[i as usize]), "{i}");
    }
    let r1 = change(&state, &id(ext, 1)).reverse.clone().unwrap();
    assert_eq!(
        hunk::apply(s.text(), &[r1]).unwrap(),
        DOC.replace("- first item\n", grown)
    );
    let r0 = change(&state, &id(ext, 0)).reverse.clone().unwrap();
    assert_eq!(
        hunk::apply(s.text(), &[r0]).unwrap(),
        DOC.replace("- more item\n", "- more item, b\n")
    );
}

#[test]
fn a_write_earlier_in_the_turn_moves_where_the_note_asks() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = format!("> draft\n\n{}", s.text());
    let top = s.external(&text);
    // The line the note sat on when the turn began
    let decoy_line = s.text().lines().nth(5).unwrap().to_string();
    assert_eq!(decoy_line, "with a second line.", "setup");
    let ext = s.replace_line("- first item", "- first item, reworded");
    let decoy = s.replace_line(&decoy_line, "with a better second line.");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.asked.kind, AskedKind::Location);
    assert_eq!(c.asked.notes, [x]);
    for n in [top, decoy] {
        assert_eq!(
            change(&state, &id(n, 0)).asked.kind,
            AskedKind::Unasked,
            "{n}"
        );
    }
    assert_eq!(state.counts.unasked, 2);
    assert_eq!(item(&state, x).status, ItemStatus::Handled);
    assert_eq!(
        item(&state, x).outcome.as_ref().unwrap().changes,
        [id(ext, 0)]
    );
}

#[test]
fn deleting_a_noted_block_is_asked_by_location_and_handles_the_note() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- third item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = s.text().replace("- third item\n", "");
    let hunks = hunk::diff(s.text(), &text);
    assert!(hunks.len() == 1 && hunks[0].i.is_empty(), "setup");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.asked.kind, AskedKind::Location);
    assert_eq!(c.asked.notes, [x]);
    assert_eq!(state.counts.unasked, 0);
    assert_eq!(item(&state, x).status, ItemStatus::Handled);
    assert_eq!(
        item(&state, x).outcome.as_ref().unwrap().changes,
        [id(ext, 0)]
    );
    // Its block is gone, so it is painted nowhere and decides nothing
    assert_eq!(item(&state, x).at.status, Some(AnchorStatus::Orphaned));
    assert!(item(&state, x).at.loc.is_none());
    assert_eq!(state.counts.decided, 0);
}

#[test]
fn a_note_its_own_turn_deleted_is_not_lost() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- third item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = s.text().replace("- third item\n", "");
    s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(item(&state, x).status, ItemStatus::Handled, "setup");
    assert_eq!(item(&state, x).at.status, Some(AnchorStatus::Orphaned));
    assert!(state.lost.is_empty());
    assert_eq!(state.counts.lost, 0);
}

#[test]
fn a_tick_and_a_caret_follow_a_later_insert_above() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let text = s.text().replace("- second item\n  - nested item\n", "");
    let tick = s.external(&text);
    let caret = s.replace_line("- more item", "- item");
    let text = format!("> draft\n\n{}", s.text());
    s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let now = |line: &str| s.text().lines().position(|l| l == line).unwrap() as u32 + 1;
    let find = |c: u32| {
        state
            .deletions
            .iter()
            .find(|d| d.change == id(c, 0))
            .unwrap_or_else(|| panic!("no deletion for {c}: {:?}", state.deletions))
    };
    let d = find(tick);
    let third = now("- third item");
    assert_eq!(d.kind, DeletionKind::Block);
    assert_eq!(d.at_line, third);
    assert_eq!(d.at_byte, Some(line_bytes(s.text(), third, third).0));
    assert_eq!(d.after_sp, Some(stop_sp(s.text(), now("- first item"))));
    let w = find(caret);
    let line = now("- item");
    let (start, end) = line_bytes(s.text(), line, line);
    assert_eq!((w.kind, w.at_line), (DeletionKind::Words, line));
    // Where "more" was, not the line start
    assert!(
        w.at_byte.is_some_and(|b| start < b && b <= end),
        "{:?}",
        w.at_byte
    );
}

#[test]
fn a_draft_whose_text_is_gone_is_lost() {
    let mut s = Scenario::new(DOC);
    let at = s.quote("Intro paragraph here");
    let x = s.comment(at, None, Some("wordy"));
    let text = s
        .text()
        .replace("Intro paragraph here,\nwith a second line.\n\n", "");
    s.edit(&text);
    let state = s.fold();
    assert_eq!(item(&state, x).at.status, Some(AnchorStatus::Orphaned));
    assert_eq!(state.lost, [x]);
    assert_eq!(state.counts.lost, 1);
}

#[test]
fn a_write_after_a_lost_claim_race_is_the_winners() {
    let mut s = Scenario::new(DOC);
    s.note("look it over");
    let sub = s.submit();
    let win = s.claim_round(CLAUDE, sub);
    s.claim_round("other", sub);
    let ext = s.replace_line("- first item", "- first item, whose?");
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.attr, Attr::Agent);
    assert_eq!(c.turn, Some(win));
    assert_eq!(c.by.name.as_deref(), Some(CLAUDE));
}

#[test]
fn unasked_counts_only_agent_changes_and_the_reply_counts_its_own_turn() {
    let mut s = Scenario::new(DOC);
    let first = claimed(&mut s);
    s.replace_line("- first item", "- first item, a");
    s.yield_turn(CLAUDE, first);
    s.note("again");
    s.submit();
    let second = s.claim(CLAUDE);
    s.replace_line("- third item", "- third item, b");
    s.yield_turn(CLAUDE, second);
    s.replace_line("- more item", "- more item, outside");
    s.edit(&s.replaced("- appendix item", "- appendix item, mine"));
    let state = s.fold();
    let kinds: Vec<AskedKind> = state.changes.iter().map(|c| c.asked.kind).collect();
    use AskedKind::{Human, Outside, Unasked};
    assert_eq!(kinds, [Unasked, Unasked, Outside, Human]);
    assert!(state.changes.iter().all(|c| !c.seen), "setup");
    assert_eq!(state.counts.unasked, 2);
    let replied = state.phrase.replied.as_ref().unwrap();
    assert_eq!((replied.turn, replied.unasked), (second, 1));
    assert_eq!(state.counts.unseen, 3);
}

#[test]
fn an_outside_change_written_over_later_is_later_overlap() {
    let mut s = Scenario::new(DOC);
    let out = s.replace_line("- first item", "- first item, outside");
    let turn = claimed(&mut s);
    s.replace_line("- first item, outside", "- first item, agent");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(out, 0));
    assert_eq!(c.attr, Attr::Outside, "setup");
    assert!(!c.reversible.ok && c.reverse.is_none());
    assert_eq!(c.reversible.reason, ReversibleReason::LaterOverlap);
}

#[test]
fn exactly_40_percent_is_no_rewrite_and_a_part_seen_rewrite_stands() {
    let (s, _, _) = grown(29);
    assert!(s.fold().rewrite.is_none(), "40/100");

    let (mut s, turn, _) = grown(30);
    let all: Vec<String> = s.fold().changes.iter().map(|c| c.id.clone()).collect();
    assert_eq!(all.len(), 32, "setup");
    s.seen(&all[..31].iter().map(String::as_str).collect::<Vec<_>>());
    let state = s.fold();
    assert_eq!(state.rewrite.as_ref().map(|r| r.turn), Some(turn));
    assert_eq!(state.counts.unseen, 1);
    let v = serde_json::to_value(&state).unwrap();
    let left: Vec<&serde_json::Value> = v["news"]
        .as_array()
        .unwrap_or_else(|| panic!("no news list"))
        .iter()
        .filter(|n| n["kind"] == "change" && n["check"].is_null())
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["id"], all[31]);
}

/// Seven of ten blocks is over the share but too few to call a rewrite.
#[test]
fn a_rewrite_needs_eight_blocks_touched_as_well_as_the_share() {
    let doc: String = (1..=10).map(|i| format!("p{i} text\n\n")).collect();
    for (edited, want) in [(7, false), (8, true)] {
        let mut s = Scenario::new(&doc);
        let turn = claimed(&mut s);
        edit_paragraphs(&mut s, 1..=edited);
        s.yield_turn(CLAUDE, turn);
        let state = s.fold();
        assert_eq!(state.changes.len(), edited, "setup");
        assert_eq!(
            state.rewrite.is_some(),
            want,
            "{edited} of 10: {:?}",
            state.rewrite
        );
    }
}

fn edit_paragraphs(s: &mut Scenario, which: std::ops::RangeInclusive<usize>) -> u32 {
    let mut text = s.text().to_string();
    for i in which {
        text = text.replacen(&format!("p{i} text\n"), &format!("p{i} edited\n"), 1);
    }
    s.external(&text)
}

#[test]
fn two_small_turns_do_not_add_up_to_a_rewrite() {
    // 25 of 90 blocks each: 28% apiece, 56% together
    let mut s = Scenario::new(&paragraphs());
    let first = claimed(&mut s);
    edit_paragraphs(&mut s, 1..=25);
    s.yield_turn(CLAUDE, first);
    s.note("again");
    s.submit();
    let second = s.claim(CLAUDE);
    edit_paragraphs(&mut s, 26..=50);
    s.yield_turn(CLAUDE, second);
    assert!(s.fold().rewrite.is_none());
}

#[test]
fn only_the_rewriting_turns_changes_are_marked() {
    let mut s = Scenario::new(&paragraphs());
    let first = claimed(&mut s);
    let small = edit_paragraphs(&mut s, 1..=1);
    s.yield_turn(CLAUDE, first);
    s.note("again");
    s.submit();
    let second = s.claim(CLAUDE);
    edit_paragraphs(&mut s, 2..=41);
    s.yield_turn(CLAUDE, second);
    let state = s.fold();
    let r = state.rewrite.as_ref().expect("40 of 90 is a rewrite");
    assert_eq!((r.turn, r.blocks_changed, r.blocks_total), (second, 40, 90));
    assert!(!change(&state, &id(small, 0)).is_rewrite);
    for c in &state.changes {
        assert_eq!(c.is_rewrite, c.turn == Some(second), "{}", c.id);
    }
}

#[test]
fn a_turn_that_cuts_most_of_the_doc_is_a_rewrite() {
    let mut s = Scenario::new(&paragraphs());
    let turn = claimed(&mut s);
    let kept: String = (1..=9)
        .map(|i| format!("p{i} text\n"))
        .collect::<Vec<_>>()
        .join("\n");
    let hunks = hunk::diff(s.text(), &kept);
    assert!(hunks.len() == 1 && hunks[0].i.is_empty(), "setup");
    s.external(&kept);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let r = state
        .rewrite
        .as_ref()
        .expect("cutting 81 of 90 is a rewrite");
    assert_eq!((r.turn, r.blocks_changed, r.blocks_total), (turn, 81, 90));
    assert_eq!(state.changes.len(), 1, "setup");
    // A turn's one big removal is one item of news
    assert_eq!(news_of(&state, "removal"), [state.changes[0].id.clone()]);
}

#[test]
fn a_removed_excerpt_of_multibyte_text_is_capped_by_chars() {
    let long = "ö ".repeat(1_500);
    let doc = format!("# T\n\n{long}\n\nend\n");
    let mut s = Scenario::new(&doc);
    let turn = claimed(&mut s);
    let ext = s.external("# T\n\nend\n");
    s.yield_turn(CLAUDE, turn);
    let removed = change(&s.fold(), &id(ext, 0)).removed.clone().unwrap();
    assert_eq!(removed.chars().count(), 2_000);
    assert!(long.starts_with(&removed));
}

#[test]
fn undoing_one_hunk_leaves_its_sibling_alone() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let text = s
        .text()
        .replace("- first item\n", "- first item, a\n")
        .replace("- more item\n", "- more item, b\n");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let before = s.replaced("- more item, b", "- more item");
    let undo = s.undo(&id(ext, 1), &before);
    let state = s.fold();
    assert_eq!(change(&state, &id(ext, 1)).undone_by, Some(undo));
    let sibling = change(&state, &id(ext, 0));
    assert_eq!(sibling.undone_by, None);
    assert!(!sibling.settled);
    assert_eq!(state.counts.unseen, 1);
}

#[test]
fn undoing_a_whole_write_marks_every_hunk_undone() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let text = s
        .text()
        .replace("- first item\n", "- first item, a\n")
        .replace("- detail two\n", "- detail two, b\n")
        .replace("- more item\n", "- more item, c\n");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    assert_eq!(change(&s.fold(), &id(ext, 1)).op_hunks, 3, "setup");

    // An undo as servers before per-change revert wrote it: one `x` reverting the whole write
    let undo = s.undo(&id(ext, 1), DOC);

    let state = s.fold();
    for idx in 0..3 {
        let c = change(&state, &id(ext, idx));
        assert_eq!((c.undone_by, c.settled), (Some(undo), true), "hunk {idx}");
    }
    assert_eq!(state.counts.unseen, 0);
}

#[test]
fn an_undo_marks_a_sibling_only_when_its_patch_reverts_it() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let text = s
        .text()
        .replace("- first item\n", "- first item, a\n")
        .replace("- more item\n", "- more item, b\n");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);

    // Names the first hunk and touches the second's line, but doesn't restore it.
    let partial = DOC.replace("- more item\n", "- more item, rewritten\n");
    let undo = s.undo(&id(ext, 0), &partial);

    let state = s.fold();
    assert_eq!(change(&state, &id(ext, 0)).undone_by, Some(undo));
    assert_eq!(change(&state, &id(ext, 1)).undone_by, None);
}

#[test]
fn a_covers_quoting_the_text_a_hunk_removed_does_not_cite() {
    let mut s = Scenario::new(DOC);
    let x = s.note("rename the first item");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- first item", "- opening item");
    assert!(!s.text().contains("first item"), "setup");
    s.covers(CLAUDE, x, "first item", turn);
    s.yield_turn(CLAUDE, turn);
    assert_eq!(
        change(&s.fold(), &id(ext, 0)).asked.kind,
        AskedKind::Unasked
    );
}

#[test]
fn a_covers_naming_a_note_the_turn_does_not_hold_does_not_cite() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    // Written during the turn, so still a draft
    let draft = s.note("add a fourth item");
    let ext = s.replace_line("- third item", "- third item\n- fourth item");
    s.covers(CLAUDE, draft, "fourth item", turn);
    s.covers(CLAUDE, 999, "fourth item", turn);
    s.yield_turn(CLAUDE, turn);
    let c = change(&s.fold(), &id(ext, 0)).clone();
    assert_eq!(c.asked.kind, AskedKind::Unasked);
    assert!(c.asked.notes.is_empty());
}

#[test]
fn a_word_swap_leaves_no_caret() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item", "- last item");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(change(&state, &id(ext, 0)).what.words, Some(1), "setup");
    assert!(state.deletions.is_empty(), "{:?}", state.deletions);
}

#[test]
fn a_claim_of_a_later_round_asks_for_the_earlier_rounds_notes() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    s.note("and the rest?");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- first item", "- first item, reworded");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(item(&state, x).turn, Some(turn), "setup");
    let c = change(&state, &id(ext, 0));
    assert_eq!(
        (c.asked.kind, c.asked.notes.clone()),
        (AskedKind::Location, vec![x])
    );
}

#[test]
fn a_draft_written_during_the_turn_does_not_ask() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let x = s.verdict("- first item", "no");
    let ext = s.replace_line("- first item", "- first item, reworded");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(item(&state, x).status, ItemStatus::Draft, "setup");
    assert_eq!(change(&state, &id(ext, 0)).asked.kind, AskedKind::Unasked);
}

#[test]
fn a_write_after_a_release_or_an_approve_claim_is_from_outside() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    s.release(turn);
    let released = s.replace_line("- first item", "- first item, late");

    let mut t = Scenario::new(DOC);
    let approve = t.approve();
    t.claim_round(CLAUDE, approve);
    let approved = t.replace_line("- first item", "- first item, after");

    for (state, n) in [(s.fold(), released), (t.fold(), approved)] {
        let c = change(&state, &id(n, 0));
        assert_eq!(
            (c.attr, c.asked.kind, c.turn),
            (Attr::Outside, AskedKind::Outside, None),
            "{n}"
        );
    }
}

#[test]
fn a_git_write_during_a_claim_is_from_outside() {
    let mut s = Scenario::new(DOC);
    claimed(&mut s);
    let git = s.external_git(&s.replaced("- first item", "- first item, pulled"));
    let state = s.fold();
    let c = change(&state, &id(git, 0));
    assert_eq!((c.attr, c.turn), (Attr::Outside, None));
    assert_eq!(c.cause.as_deref(), Some("git"));
}

// --- lead rulings on the adversary's open items ---

#[test]
fn after_sp_is_the_stop_above_even_across_a_blank_line() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let list = "- first item\n- second item\n  - nested item\n- third item\n\n";
    let ext = s.external(&s.text().replace(list, ""));
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let d = state
        .deletions
        .iter()
        .find(|d| d.change == id(ext, 0))
        .expect("a tick");
    // Myers may drop either blank line; the intro paragraph (3-4) is above either way
    assert_eq!(d.after_sp, Some(stop_sp(s.text(), 3)));
}

#[test]
fn after_sp_below_a_nested_item_is_the_innermost_stop() {
    // Parent 7-8 and nested 8 both end on line 8; the nested one is inner
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.external(&s.text().replace("- third item\n", ""));
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let d = state
        .deletions
        .iter()
        .find(|d| d.change == id(ext, 0))
        .expect("a tick");
    assert_eq!(d.after_sp, Some(stop_sp(s.text(), 8)));
}

#[test]
fn a_deletion_at_the_end_has_its_gap_after_the_last_line() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.external(&s.text().replace("\n# Appendix\n\n- appendix item\n", ""));
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let d = state
        .deletions
        .iter()
        .find(|d| d.change == id(ext, 0))
        .expect("a tick");
    let last = s.text().lines().count() as u32;
    assert_eq!(d.at_line, last + 1);
    assert_eq!(d.at_byte, Some(s.text().len()));
    assert_eq!(d.after_sp, Some(stop_sp(s.text(), last)));
}

#[test]
fn an_insert_right_below_a_change_leaves_it_in_place_and_reversible() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item", "- first item, reworded");
    s.replace_line(
        "- first item, reworded",
        "- first item, reworded\n- added below",
    );
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.range.status, RangeStatus::Ok);
    assert_eq!(c.range.lines, (6, 6));
    assert!(c.reversible.ok);
    assert_eq!(c.reversible.reason, ReversibleReason::None);
}

#[test]
fn a_settled_unasked_change_is_not_counted() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- appendix item", "- appendix item, tidied");
    s.yield_turn(CLAUDE, turn);
    assert_eq!(s.fold().counts.unasked, 1, "setup");
    s.seen(&[id(ext, 0).as_str()]);
    s.approve();
    let state = s.fold();
    assert!(change(&state, &id(ext, 0)).settled, "setup");
    assert_eq!(state.counts.unasked, 0);
}

#[test]
fn a_seen_from_an_agent_is_ignored() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item", "- first item, reworded");
    s.yield_turn(CLAUDE, turn);
    s.push(agent(CLAUDE), json!({"op": "seen", "ids": [id(ext, 0)]}));
    let state = s.fold();
    assert!(!change(&state, &id(ext, 0)).seen);
    assert_eq!(state.counts.unseen, 1);
}

#[test]
fn a_withdrawn_hand_back_settles_nothing() {
    let mut s = Scenario::new(DOC);
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item", "- first item, reworded");
    s.yield_turn(CLAUDE, turn);
    s.seen(&[id(ext, 0).as_str()]);
    s.note("next");
    let sub = s.submit_held();
    s.withdraw(sub);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert!(c.seen && !c.settled);
}

#[test]
fn your_edit_flag_clears_once_the_change_is_seen() {
    let mut s = Scenario::new(DOC);
    s.edit(&s.replaced("- first item", "- first item, mine"));
    let turn = claimed(&mut s);
    let ext = s.replace_line("- first item, mine", "- first item, theirs");
    s.yield_turn(CLAUDE, turn);
    assert!(
        change(&s.fold(), &id(ext, 0)).changes_your_edit.is_some(),
        "setup"
    );
    s.seen(&[id(ext, 0).as_str()]);
    let state = s.fold();
    let c = change(&state, &id(ext, 0));
    assert_eq!(c.changes_your_edit, None);
    assert!(!c.reverts_your_edit);
}

// --- hostile lines ---

/// Folds the log on `now`, which the kit's own text may not have followed.
fn fold_on(s: &Scenario, now: &str) -> ReviewState {
    let (parsed, chain, _) = s.build();
    fold::fold(&parsed, &chain, &Doc::new(now)).state
}

#[test]
fn notes_on_lines_at_the_end_of_u32_fold() {
    let mut s = Scenario::new(DOC);
    let rev = Rev::of(DOC.as_bytes());
    let mut notes = Vec::new();
    for scope in ["quote", "block", "section"] {
        for lines in [[u32::MAX, u32::MAX], [1, u32::MAX]] {
            let at = json!({"quote": "item", "pre": "", "suf": "", "lines": lines,
                            "rev": rev, "scope": scope});
            notes.push(s.comment(at, Some("no"), Some("fix")));
        }
    }
    s.submit();
    let turn = s.claim(CLAUDE);
    s.replace_line("- first item", "- first item, reworded");
    let text = s.text().replace("- third item\n", "");
    s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    for n in notes {
        item(&state, n);
    }
}

#[test]
fn a_forged_hunk_after_a_break_does_not_break_the_fold() {
    let max = u32::MAX;
    // (patch, whether the line is bad)
    let forged = [
        (json!([{"a": max, "d": ["x\n"], "i": []}]), true),
        (
            json!([{"a": 0, "d": ["x\n"], "i": []}, {"a": 0, "d": [], "i": ["y\n"]}]),
            true,
        ),
        (
            json!([{"a": 0, "d": [], "i": ["x\n", "y\n"]},
                {"a": max - 3, "d": [], "i": ["p\n", "q\n"]}]),
            true,
        ),
        (json!([{"a": max - 2, "d": ["x\n"], "i": ["y\n"]}]), false),
        (
            json!([{"a": 0, "d": [], "i": ["x\n", "y\n"]},
                {"a": max - 4, "d": [], "i": ["z\n"]}]),
            false,
        ),
    ];
    for (patch, bad) in forged {
        let mut s = Scenario::new(DOC);
        s.verdict("- first item", "no");
        s.submit();
        let turn = s.claim(CLAUDE);
        let (a, b, c) = (Rev::of(DOC.as_bytes()), Rev::of(b"b\n"), Rev::of(b"c\n"));
        // The first patch doesn't apply, so the second is kept without being applied
        s.raw(
            json!({"by": {"type": "birta"}, "op": "external", "rev": a, "to_rev": b,
                     "patch": [{"a": 0, "d": ["nope\n"], "i": []}]}),
        );
        s.raw(
            json!({"by": {"type": "birta"}, "op": "external", "rev": b, "to_rev": c,
                     "patch": patch}),
        );
        s.yield_turn(CLAUDE, turn);
        let state = fold_on(&s, "c\n");
        assert_eq!(state.log.bad, u32::from(bad), "{patch}");
    }
}

#[test]
fn many_deletions_are_ordered_in_linear_time() {
    // One stop, so placing each deletion is cheap and only their ordering is at stake
    let lines = 40_000;
    let body: String = (0..lines).map(|i| format!("line {i}\n")).collect();
    let mut s = Scenario::new(&format!("```\n{body}```\n"));
    let kept: String = (0..lines)
        .filter(|i| i % 2 == 0)
        .map(|i| format!("line {i}\n"))
        .collect();
    s.external(&format!("```\n{kept}```\n"));
    let (parsed, chain, doc) = s.build();
    let start = std::time::Instant::now();
    let state = fold::fold(&parsed, &chain, &doc).state;
    let took = start.elapsed();
    assert_eq!(state.changes.len(), lines / 2, "setup");
    assert_eq!(state.deletions.len(), lines / 2, "setup");
    eprintln!("fold of {} deletions: {took:?}", lines / 2);
    assert!(took < std::time::Duration::from_millis(1_000), "{took:?}");
}

#[test]
fn an_edit_to_a_moved_note_is_asked_by_location() {
    let mut unasked = Vec::new();
    for quote in [false, true] {
        for next_turn in [false, true] {
            for one_write in [false, true] {
                let case = format!("quote {quote}, next turn {next_turn}, one write {one_write}");
                let mut s = Scenario::new(DOC);
                let x = if quote {
                    let at = s.quote("third item");
                    s.comment(at, Some("no"), Some("move it"))
                } else {
                    s.verdict("- third item", "no")
                };
                s.submit();
                let turn = s.claim(CLAUDE);
                let moved = s
                    .text()
                    .replace("- third item\n", "")
                    .replace("- more item\n", "- more item\n- third item\n");
                if one_write {
                    s.external(&moved);
                } else {
                    s.external(&s.text().replace("- third item\n", ""));
                    s.external(&moved);
                }
                let turn = if next_turn {
                    s.release(turn);
                    s.claim(CLAUDE)
                } else {
                    turn
                };
                let edit = s.replace_line("- third item", "- third item, reworded");
                s.yield_turn(CLAUDE, turn);
                let state = s.fold();
                let c = change(&state, &id(edit, 0));
                let outcome = item(&state, x).outcome.as_ref().unwrap();
                if c.asked.kind != AskedKind::Location
                    || c.asked.notes != [x]
                    || !outcome.changes.contains(&id(edit, 0))
                {
                    unasked.push(case);
                }
            }
        }
    }
    assert!(unasked.is_empty(), "{unasked:#?}");
}
