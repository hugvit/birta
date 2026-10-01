use std::path::Path;

use birta::rev::Rev;
use birta::review::fold::hand_off;
use birta::review::ops::{ByKind, Scope, To, Verdict};
use birta::review::state::{AskedKind, AtKind, HoldKind, Item, ItemStatus, ReviewState, Stage};
use serde_json::json;

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, HOLD_MS, HUMAN, Scenario, T0, human, ts};

const CLAUDE: &str = "claude";

fn item(state: &ReviewState, n: u32) -> &Item {
    state
        .items
        .iter()
        .find(|i| i.n == n)
        .unwrap_or_else(|| panic!("no item {n} in {:?}", state.items))
}

fn status(state: &ReviewState, n: u32) -> ItemStatus {
    item(state, n).status
}

type Outcomes = Option<(bool, bool, Vec<String>)>;

/// `(done, no_change, changes)` of an item's outcome.
fn outcome_of(state: &ReviewState, n: u32) -> Outcomes {
    item(state, n)
        .outcome
        .as_ref()
        .map(|o| (o.done, o.no_change, o.changes.clone()))
}

fn outcome(done: bool, no_change: bool, changes: &[String]) -> Outcomes {
    Some((done, no_change, changes.to_vec()))
}

fn change_id(op: u32) -> String {
    format!("{op}.0")
}

// --- fresh and drafts (scenario 1) ---

#[test]
fn a_fresh_log_is_idle_and_counts_the_docs_items() {
    let state = Scenario::new(DOC).fold();
    assert_eq!(state.schema, "review-state/1");
    assert_eq!(state.doc, "pr.md");
    assert_eq!(state.rev, Rev::of(DOC.as_bytes()).as_str());
    assert_eq!(state.phrase.stage, Stage::Idle);
    assert!(state.phrase.sent.is_none());
    assert!(state.phrase.picked_up.is_none());
    assert!(state.phrase.replied.is_none());
    assert!(state.hold.is_none());
    assert!(state.phrase_before_hold.is_none());
    assert!(state.approve.is_none());
    assert!(state.items.is_empty());
    assert!(state.rounds.is_empty());
    assert!(state.later.is_empty());
    assert!(state.warnings.is_empty(), "{:?}", state.warnings);
    // 8 list items (one nested) and 2 body rows; the header row is not a stop
    assert_eq!(state.counts.items, 10);
    assert_eq!(state.counts.decided, 0);
    assert_eq!(state.counts.drafts, 0);
    assert_eq!(state.counts.attention, 0);
    assert_eq!(state.log.lines, 3);
    assert_eq!(state.log.bad, 0);
    assert_eq!(state.log.unknown_ops, 0);
    assert!(!state.log.read_only);
    assert!(state.log.baseline_available);
}

#[test]
fn bad_lines_and_unknown_ops_are_counted_not_fatal() {
    let mut s = Scenario::new(DOC);
    s.raw_text("{not json");
    s.raw(json!({"ts": ts(5), "by": human(), "op": "frobnicate", "x": 1}));
    let x = s.note("still read");
    let state = s.fold();
    assert_eq!(state.log.lines, 6);
    assert_eq!(state.log.bad, 1);
    assert_eq!(state.log.unknown_ops, 1);
    assert_eq!(status(&state, x), ItemStatus::Draft);
}

#[test]
fn scenario_1_drafts_stay_out_of_the_agents_view() {
    let mut s = Scenario::new(DOC);
    let note = s.note("tighten the intro");
    let verdict = s.verdict("- first item", "no");
    let state = s.fold();

    assert_eq!(state.phrase.stage, Stage::Drafts);
    assert!(state.phrase.sent.is_none());
    assert_eq!(state.counts.drafts, 2);
    assert_eq!(state.counts.sent_unclaimed, 0);
    assert!(state.rounds.is_empty());
    assert!(state.hold.is_none());
    let agent_view: Vec<_> = state
        .items
        .iter()
        .filter(|i| matches!(i.status, ItemStatus::Sent | ItemStatus::PickedUp))
        .collect();
    assert!(agent_view.is_empty(), "{agent_view:?}");

    let n = item(&state, note);
    assert_eq!(n.status, ItemStatus::Draft);
    assert_eq!(n.op, "comment");
    assert_eq!(n.by.kind, ByKind::Human);
    assert_eq!(n.by.name.as_deref(), Some(HUMAN));
    assert_eq!(n.cid.as_deref(), Some("c-4"));
    assert_eq!(n.ts, ts(note));
    assert_eq!(n.at.kind, AtKind::Doc);
    assert!(n.at.loc.is_none());
    assert_eq!(n.verdict, None);
    assert_eq!(n.body.as_deref(), Some("tighten the intro"));
    assert_eq!(n.sent_in, None);
    assert_eq!(n.turn, None);
    assert_eq!(n.id, None);
    assert!(n.amended.is_empty() && n.replies.is_empty());
    assert!(n.outcome.is_none());

    let v = item(&state, verdict);
    assert_eq!(v.status, ItemStatus::Draft);
    assert_eq!(v.verdict, Some(Verdict::No));
    assert_eq!(v.at.kind, AtKind::Anchor);
    assert_eq!(v.at.scope, Some(Scope::Block));
    assert_eq!(v.at.quote.as_deref(), Some("- first item"));
    assert_eq!(v.at.rev.as_deref(), Some(Rev::of(DOC.as_bytes()).as_str()));
}

// --- submit and withdraw (scenario 2) ---

#[test]
fn scenario_2_a_submit_sends_its_drafts() {
    let mut s = Scenario::new(DOC);
    let x = s.note("one");
    let y = s.verdict("- first item", "no");
    let sub = s.submit();
    let state = s.fold();

    for n in [x, y] {
        assert_eq!(status(&state, n), ItemStatus::Sent);
        assert_eq!(item(&state, n).sent_in, Some(sub));
        assert_eq!(item(&state, n).turn, None);
    }
    assert_eq!(state.phrase.stage, Stage::Sent);
    let sent = state.phrase.sent.as_ref().expect("sent facts");
    assert_eq!((sent.n, sent.ts, sent.items), (sub, ts(sub), 2));
    assert!(sent.can_withdraw);
    assert!(state.phrase.picked_up.is_none());
    assert_eq!(state.counts.drafts, 0);
    assert_eq!(state.counts.sent_unclaimed, 2);

    assert_eq!(state.rounds.len(), 1);
    let r = &state.rounds[0];
    assert_eq!(r.submit, sub);
    assert_eq!(r.ts, ts(sub));
    assert_eq!(r.by, HUMAN);
    assert_eq!(r.to, Some(To::One("*".to_string())));
    assert_eq!(r.items, [x, y]);
    assert_eq!(r.claim, None);
    assert_eq!(r.claimed_by, None);
    assert_eq!(r.r#yield, None);
    assert!(!r.released);
}

#[test]
fn scenario_2_withdraw_before_a_claim_gives_the_drafts_back() {
    let mut s = Scenario::new(DOC);
    let x = s.note("one");
    let sub = s.submit();
    s.withdraw(sub);
    let state = s.fold();

    assert_eq!(status(&state, x), ItemStatus::Draft);
    assert_eq!(item(&state, x).sent_in, None);
    assert_eq!(state.phrase.stage, Stage::Drafts);
    assert!(state.phrase.sent.is_none());
    assert_eq!(state.counts.drafts, 1);
    assert_eq!(state.counts.sent_unclaimed, 0);
    // A withdrawn round was never handed back
    assert!(state.rounds.is_empty(), "{:?}", state.rounds);
    assert!(state.warnings.is_empty(), "{:?}", state.warnings);
}

#[test]
fn scenario_2_withdraw_after_a_claim_is_ignored_with_a_warning() {
    let mut s = Scenario::new(DOC);
    let x = s.note("one");
    let sub = s.submit();
    let turn = s.claim(CLAUDE);
    s.withdraw(sub);
    let state = s.fold();

    assert_eq!(status(&state, x), ItemStatus::PickedUp);
    assert_eq!(item(&state, x).turn, Some(turn));
    assert_eq!(state.phrase.stage, Stage::PickedUp);
    assert_eq!(state.rounds.len(), 1);
    assert_eq!(state.rounds[0].claim, Some(turn));
    assert_eq!(state.warnings.len(), 1, "{:?}", state.warnings);
    assert!(
        state.warnings[0].contains("withdraw"),
        "{:?}",
        state.warnings
    );
}

#[test]
fn a_claim_takes_the_items_and_starts_the_silence_clock() {
    let mut s = Scenario::new(DOC);
    let x = s.note("one");
    let sub = s.submit();
    let turn = s.claim(CLAUDE);
    let state = s.fold();

    assert_eq!(status(&state, x), ItemStatus::PickedUp);
    assert_eq!(item(&state, x).sent_in, Some(sub));
    assert_eq!(item(&state, x).turn, Some(turn));
    assert_eq!(state.phrase.stage, Stage::PickedUp);
    let sent = state.phrase.sent.as_ref().expect("sent facts");
    assert!(!sent.can_withdraw);
    let picked = state.phrase.picked_up.as_ref().expect("picked-up facts");
    assert_eq!(
        (picked.n, picked.ts, picked.by.as_str()),
        (turn, ts(turn), CLAUDE)
    );
    assert_eq!(picked.silence.since_ts, ts(turn));
    assert_eq!(picked.silence.threshold_ms, 900_000);
    assert_eq!(state.counts.sent_unclaimed, 0);
    assert_eq!(state.rounds[0].claimed_by.as_deref(), Some(CLAUDE));
}

// --- the hold (scenario 3, and the phrase before the hold) ---

#[test]
fn scenario_3_a_note_typed_during_the_hold_stays_a_draft() {
    let mut s = Scenario::new(DOC);
    let x = s.note("sent");
    let sub = s.submit_held();
    let y = s.note("typed during the hold");
    let state = s.fold();

    assert_eq!(status(&state, x), ItemStatus::Sent);
    assert_eq!(status(&state, y), ItemStatus::Draft);
    assert_eq!(item(&state, y).sent_in, None);
    assert_eq!(state.counts.drafts, 1);
    assert_eq!(state.counts.sent_unclaimed, 1);
    assert_eq!(state.rounds[0].items, [x]);

    let hold = state.hold.as_ref().expect("a pending hold");
    assert_eq!(hold.n, sub);
    assert_eq!(hold.kind, HoldKind::HandBack);
    assert_eq!(hold.count, 1);
    assert_eq!(hold.not_before, ts(sub) + HOLD_MS);

    // The hand-back happens when the hold ends
    assert_eq!(state.phrase.stage, Stage::Sent);
    let sent = state.phrase.sent.as_ref().expect("sent facts");
    assert_eq!((sent.n, sent.ts, sent.items), (sub, ts(sub) + HOLD_MS, 1));
    assert!(sent.can_withdraw);
    assert_eq!(state.rounds[0].ts, ts(sub) + HOLD_MS);

    let before = state
        .phrase_before_hold
        .as_ref()
        .expect("phrase before hold");
    assert_eq!(before.stage, Stage::Drafts);
    assert!(before.sent.is_none());
}

#[test]
fn a_far_future_hold_is_clamped_to_five_seconds() {
    let mut s = Scenario::new(DOC);
    s.note("sent");
    let sub = s.submit_until(ts(5) + 10 * 60_000);
    let state = s.fold();
    let hold = state.hold.as_ref().expect("a pending hold");
    assert_eq!(hold.not_before, ts(sub) + 5_000);
    assert_eq!(state.phrase.sent.as_ref().unwrap().ts, ts(sub) + 5_000);
    assert_eq!(state.rounds[0].ts, ts(sub) + 5_000);
}

#[test]
fn the_phrase_before_a_second_hand_back_is_the_reply() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    s.note("next round");
    let sub = s.submit_held();
    let state = s.fold();

    assert_eq!(state.phrase.stage, Stage::Sent);
    assert_eq!(state.phrase.sent.as_ref().unwrap().n, sub);
    let hold = state.hold.as_ref().expect("a pending hold");
    assert_eq!((hold.n, hold.count), (sub, 1));
    let before = state
        .phrase_before_hold
        .as_ref()
        .expect("phrase before hold");
    assert_eq!(before.stage, Stage::Replied);
    let replied = before.replied.as_ref().expect("replied facts");
    assert_eq!((replied.turn, replied.done), (turn, 1));
}

#[test]
fn a_claimed_or_withdrawn_hold_is_over() {
    // The claim lands on the line whose ts is the hold's end
    let mut s = Scenario::new(DOC);
    s.note("sent");
    s.submit_until(ts(5) + 1_000);
    s.claim(CLAUDE);
    let state = s.fold();
    assert!(state.hold.is_none());
    assert!(state.phrase_before_hold.is_none());
    assert_eq!(state.phrase.stage, Stage::PickedUp);

    let mut s = Scenario::new(DOC);
    s.note("sent");
    let sub = s.submit_until(ts(5) + 1_000);
    s.withdraw(sub);
    let state = s.fold();
    assert!(state.hold.is_none());
    assert!(state.phrase_before_hold.is_none());
    assert_eq!(state.phrase.stage, Stage::Drafts);
}

#[test]
fn a_hand_back_with_no_hold_has_no_hold_fact() {
    // Due at its own ts, and a hand-written one with no ts at all
    let mut s = Scenario::new(DOC);
    s.note("sent");
    s.submit();
    let state = s.fold();
    assert!(state.hold.is_none());
    assert!(state.phrase_before_hold.is_none());
    assert_eq!(state.phrase.stage, Stage::Sent);

    let mut s = Scenario::new(DOC);
    s.note("sent");
    s.raw(json!({"by": human(), "op": "submit", "upto": 4, "not_before": T0 + 999_999, "hold_ms": 5000}));
    let state = s.fold();
    assert!(state.hold.is_none());
    assert_eq!(state.phrase.stage, Stage::Sent);
}

#[test]
fn an_approve_in_its_hold_is_an_approve_hold() {
    let mut s = Scenario::new(DOC);
    s.note("a draft");
    let ap = s.approve_held();
    let state = s.fold();
    let hold = state.hold.as_ref().expect("a pending hold");
    assert_eq!(hold.n, ap);
    assert_eq!(hold.kind, HoldKind::Approve);
    assert_eq!(hold.not_before, ts(ap) + HOLD_MS);
    assert_eq!(state.phrase.stage, Stage::Approved);
    assert_eq!(
        state.phrase_before_hold.as_ref().map(|p| p.stage),
        Some(Stage::Drafts)
    );
}

// --- claim CAS (scenario 4) ---

#[test]
fn scenario_4_the_losing_claim_and_its_turn_are_ignored() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    let sub = s.submit();
    let winner = s.claim("claude");
    let loser = s.claim_round("other", sub);
    s.done("other", x, loser);
    s.yield_turn("other", loser);
    let state = s.fold();

    assert_eq!(status(&state, x), ItemStatus::PickedUp);
    assert_eq!(item(&state, x).turn, Some(winner));
    assert!(item(&state, x).outcome.is_none());
    assert_eq!(state.rounds[0].claim, Some(winner));
    assert_eq!(state.rounds[0].claimed_by.as_deref(), Some("claude"));
    assert_eq!(state.rounds[0].r#yield, None);
    assert_eq!(state.phrase.stage, Stage::PickedUp);
    assert_eq!(state.phrase.picked_up.as_ref().unwrap().by, "claude");

    s.done("claude", x, winner);
    let y = s.yield_turn("claude", winner);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(state.rounds[0].r#yield, Some(y));
}

// --- a full turn (scenario 5) ---

/// Six verdicts sent in one round and answered in one turn.
fn full_turn() -> (Scenario, [u32; 6], u32, u32, u32) {
    let mut s = Scenario::new(DOC);
    let no = s.verdict("- first item", "no");
    let ask = s.verdict("- second item", "ask");
    let yes = s.verdict("- third item", "yes");
    let later = s.verdict("- detail one", "later");
    let stet = s.verdict("- detail two", "stet");
    let plain = s.note("tighten the intro");
    let sub = s.submit();
    let turn = s.claim(CLAUDE);
    s.done(CLAUDE, no, turn);
    s.agent_reply(CLAUDE, ask, "answer", "two cases need it");
    s.agent_reply(CLAUDE, plain, "didnt", "the intro is already short");
    s.turn_note(
        CLAUDE,
        "Cut the first item.\nAnswered the question.\nThird line.",
        turn,
    );
    let y = s.yield_turn(CLAUDE, turn);
    (s, [no, ask, yes, later, stet, plain], sub, turn, y)
}

#[test]
fn scenario_5_a_full_turn_sets_every_status_and_the_reply_facts() {
    let (s, [no, ask, yes, later, stet, plain], sub, turn, y) = full_turn();
    let state = s.fold();

    assert_eq!(status(&state, no), ItemStatus::Handled);
    assert_eq!(outcome_of(&state, no), outcome(true, true, &[]));
    assert_eq!(status(&state, ask), ItemStatus::Handled);
    assert_eq!(item(&state, ask).replies.len(), 1);
    assert_eq!(status(&state, yes), ItemStatus::Handled);
    assert_eq!(status(&state, later), ItemStatus::Later);
    assert_eq!(status(&state, stet), ItemStatus::Handled);
    assert_eq!(status(&state, plain), ItemStatus::Handled);
    for n in [no, ask, yes, later, stet, plain] {
        assert_eq!(item(&state, n).sent_in, Some(sub), "item {n}");
        assert_eq!(item(&state, n).turn, Some(turn), "item {n}");
        assert_eq!(item(&state, n).id, None, "item {n}");
    }

    let reply = &item(&state, ask).replies[0];
    // no, ask, yes, later, stet, plain, submit, claim, done, then the answer
    assert_eq!(reply.n, ask + 8);
    assert_eq!(reply.id, format!("#{}", reply.n));
    assert_eq!(reply.by.kind, ByKind::Agent);
    assert_eq!(reply.body.as_deref(), Some("two cases need it"));
    assert_eq!(reply.ts, ts(reply.n));
    assert!(!reply.seen);

    assert_eq!(state.phrase.stage, Stage::Replied);
    let r = state.phrase.replied.as_ref().expect("replied facts");
    assert_eq!(r.turn, turn);
    assert_eq!(r.ts, ts(y));
    assert_eq!(r.by, CLAUDE);
    assert_eq!(
        (
            r.done,
            r.answered,
            r.didnt,
            r.asked,
            r.unasked,
            r.not_handled
        ),
        (1, 1, 1, 0, 0, 0)
    );
    // Only the first two lines of the turn note are kept
    assert_eq!(
        r.note.as_deref(),
        Some("Cut the first item.\nAnswered the question.")
    );
    assert!(state.phrase.sent.is_some() && state.phrase.picked_up.is_some());

    assert_eq!(state.rounds[0].items, [no, ask, yes, later, stet, plain]);
    assert_eq!(state.rounds[0].claim, Some(turn));
    assert_eq!(state.rounds[0].r#yield, Some(y));
    assert_eq!(state.later, [later]);
    assert_eq!(state.counts.later, 1);
    assert_eq!(state.counts.not_handled, 0);
    assert_eq!(state.counts.open_asks, 0);
    assert_eq!(state.counts.drafts, 0);
    // Two unseen agent replies; no changes, blanks or not-handled rows
    assert_eq!(state.counts.attention, 2);
}

#[test]
fn scenario_5_shape() {
    let (s, ..) = full_turn();
    let json = serde_json::to_string_pretty(&s.fold()).unwrap();
    insta::assert_snapshot!("full_turn", json);
}

// --- which verdicts need an outcome (scenario 6, design call 8) ---

#[derive(Clone, Copy, Debug)]
enum Outcome {
    Nothing,
    ChangeOnBlock,
    Done,
    Answer,
}

/// A note on `- first item` with `verdict`, and the turn that answers it with `how`.
fn after_turn(verdict: Option<&str>, how: Outcome) -> (ReviewState, u32) {
    let mut s = Scenario::new(DOC);
    let at = s.block("- first item");
    let x = s.comment(at, verdict, Some("see here"));
    s.submit();
    let turn = s.claim(CLAUDE);
    match how {
        Outcome::Nothing => {}
        Outcome::ChangeOnBlock => {
            s.replace_line("- first item", "- first item, reworded");
        }
        Outcome::Done => {
            s.done(CLAUDE, x, turn);
        }
        Outcome::Answer => {
            s.agent_reply(CLAUDE, x, "answer", "because");
        }
    }
    s.yield_turn(CLAUDE, turn);
    (s.fold(), x)
}

#[test]
fn scenario_6_the_verdict_outcome_table() {
    use ItemStatus::{Handled as H, Later as L, NotHandled as NH};
    // Nothing, a change on the block, done, an answer
    let table: [(Option<&str>, [ItemStatus; 4]); 8] = [
        (None, [NH, H, H, H]),
        (Some("no"), [NH, H, H, H]),
        (Some("your_call"), [NH, H, H, H]),
        (Some("rewrite"), [NH, H, H, H]),
        (Some("ask"), [NH, NH, NH, H]),
        (Some("yes"), [H, H, H, H]),
        (Some("stet"), [H, H, H, H]),
        (Some("later"), [L, L, L, L]),
    ];
    let hows = [
        Outcome::Nothing,
        Outcome::ChangeOnBlock,
        Outcome::Done,
        Outcome::Answer,
    ];
    for (verdict, want) in table {
        for (how, want) in hows.into_iter().zip(want) {
            let (state, x) = after_turn(verdict, how);
            let it = item(&state, x);
            assert_eq!(it.status, want, "{verdict:?} after {how:?}");
            let nh = want == NH;
            assert_eq!(
                it.id.as_deref(),
                nh.then_some("nh:4"),
                "{verdict:?} after {how:?}"
            );
            assert_eq!(
                state.counts.not_handled,
                u32::from(nh),
                "{verdict:?} after {how:?}"
            );
        }
    }
}

#[test]
fn a_didnt_or_a_note_reply_handles_a_no() {
    for kind in ["didnt", "note"] {
        let mut s = Scenario::new(DOC);
        let x = s.verdict("- first item", "no");
        s.submit();
        let turn = s.claim(CLAUDE);
        s.agent_reply(CLAUDE, x, kind, "kept it");
        s.yield_turn(CLAUDE, turn);
        assert_eq!(status(&s.fold(), x), ItemStatus::Handled, "{kind}");
    }
}

#[test]
fn a_change_handles_a_note_through_its_block_not_just_its_quote() {
    // The quote is on line 3; the agent rewrites line 4 of the same paragraph
    let mut s = Scenario::new(DOC);
    let at = s.quote("Intro paragraph");
    let x = s.comment(at, None, Some("wordy"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("with a second line.", "and a shorter second.");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(
        outcome_of(&state, x),
        outcome(false, false, &[change_id(ext)])
    );

    let mut s = Scenario::new(DOC);
    let at = s.quote("Intro paragraph");
    let x = s.comment(at, None, Some("wordy"));
    s.submit();
    let turn = s.claim(CLAUDE);
    s.replace_line("- first item", "- first item, reworded");
    s.yield_turn(CLAUDE, turn);
    assert_eq!(status(&s.fold(), x), ItemStatus::NotHandled);
}

#[test]
fn a_heading_verdict_is_handled_by_a_change_anywhere_in_its_section() {
    let inside = {
        let mut s = Scenario::new(DOC);
        let at = s.section("## Details");
        let x = s.comment(at, Some("no"), Some("redo this part"));
        s.submit();
        let turn = s.claim(CLAUDE);
        s.replace_line("- detail two", "- detail two, redone");
        s.yield_turn(CLAUDE, turn);
        status(&s.fold(), x)
    };
    assert_eq!(inside, ItemStatus::Handled);

    // `## More` is a sibling section, so not covered
    let outside = {
        let mut s = Scenario::new(DOC);
        let at = s.section("## Details");
        let x = s.comment(at, Some("no"), Some("redo this part"));
        s.submit();
        let turn = s.claim(CLAUDE);
        s.replace_line("- more item", "- more item, redone");
        s.yield_turn(CLAUDE, turn);
        status(&s.fold(), x)
    };
    assert_eq!(outside, ItemStatus::NotHandled);
}

#[test]
fn a_change_is_matched_where_the_note_is_when_the_turn_starts() {
    // An outside write adds two lines on top before the claim, so the item is on line 8
    let prefaced = |s: &Scenario| format!("> draft\n\n{}", s.text());
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let text = prefaced(&s);
    s.external(&text);
    let turn = s.claim(CLAUDE);
    s.replace_line("- first item", "- first item, reworded");
    s.yield_turn(CLAUDE, turn);
    assert_eq!(status(&s.fold(), x), ItemStatus::Handled);

    // Line 6, where the item was when the note was made, now holds the paragraph
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let text = prefaced(&s);
    s.external(&text);
    let turn = s.claim(CLAUDE);
    s.replace_line("with a second line.", "and a shorter second.");
    s.yield_turn(CLAUDE, turn);
    assert_eq!(status(&s.fold(), x), ItemStatus::NotHandled);
}

/// A note its turn left open is carried to the next claim, so that turn's outcomes
/// settle it: the fold agrees with the round view, which lists it again.
#[test]
fn an_outcome_in_the_turn_a_note_was_carried_to_counts() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let first = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, first);
    let z = s.note("round two");
    s.submit();
    let second = s.claim(CLAUDE);
    s.done(CLAUDE, x, second);
    s.agent_reply(CLAUDE, x, "answer", "done now");
    s.replace_line("- first item", "- first item, reworded");
    s.done(CLAUDE, z, second);
    s.yield_turn(CLAUDE, second);
    let state = s.fold();

    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(item(&state, x).turn, Some(second));
    assert_eq!(status(&state, z), ItemStatus::Handled);
    assert_eq!(item(&state, z).turn, Some(second));
    // The reply facts count the turn's own items, carried ones included
    let replied = state.phrase.replied.as_ref().expect("replied facts");
    assert_eq!(replied.turn, second);
    assert_eq!(
        (replied.done, replied.answered, replied.not_handled),
        (2, 1, 0)
    );
    assert_eq!(state.counts.not_handled, 0);
}

#[test]
fn a_change_outside_the_turn_does_not_count() {
    // Before the claim
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    s.replace_line("- first item", "- first item, reworded");
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    assert_eq!(status(&s.fold(), x), ItemStatus::NotHandled);

    // After the yield
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    s.replace_line("- first item", "- first item, reworded");
    assert_eq!(status(&s.fold(), x), ItemStatus::NotHandled);
}

#[test]
fn a_reply_after_the_yield_that_is_not_late_does_not_count() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    s.agent_reply(CLAUDE, x, "answer", "too late");
    s.done(CLAUDE, x, turn);
    assert_eq!(status(&s.fold(), x), ItemStatus::NotHandled);
}

// --- done with no change (scenario 7) ---

#[test]
fn scenario_7_done_without_a_change_to_the_block_is_no_change() {
    let run = |edit: Option<&str>| {
        let mut s = Scenario::new(DOC);
        let x = s.verdict("- first item", "no");
        s.submit();
        let turn = s.claim(CLAUDE);
        let ext = edit.map(|line| s.replace_line(line, &format!("{line}, reworded")));
        s.done(CLAUDE, x, turn);
        s.yield_turn(CLAUDE, turn);
        let state = s.fold();
        assert_eq!(status(&state, x), ItemStatus::Handled);
        (outcome_of(&state, x), ext)
    };
    assert_eq!(run(None).0, outcome(true, true, &[]));
    let (got, ext) = run(Some("- first item"));
    assert_eq!(got, outcome(true, false, &[change_id(ext.unwrap())]));
    // A change to another block leaves this one's text as it was
    assert_eq!(run(Some("- third item")).0, outcome(true, true, &[]));
}

#[test]
fn a_change_alone_is_an_outcome_without_done() {
    let (state, x) = after_turn(Some("no"), Outcome::ChangeOnBlock);
    let ext = state.rounds[0].claim.unwrap() + 1;
    assert_eq!(
        outcome_of(&state, x),
        outcome(false, false, &[change_id(ext)])
    );
}

// --- approve (scenario 13, the fold's part) ---

#[test]
fn scenario_13_an_approve_covers_the_drafts_and_carries_later() {
    let mut s = Scenario::new(DOC);
    let later = s.verdict("- detail one", "later");
    s.verdict("- first item", "yes");
    let ap = s.approve();
    let state = s.fold();

    assert_eq!(state.phrase.stage, Stage::Approved);
    let a = state.approve.as_ref().expect("approve facts");
    assert_eq!(a.n, ap);
    assert_eq!(a.ts, ts(ap));
    assert_eq!(a.rev, Rev::of(DOC.as_bytes()).as_str());
    assert!(a.current_matches);
    assert!(!a.changed_since);
    assert_eq!(a.later, 1);
    assert!(a.can_withdraw);
    assert_eq!(state.counts.drafts, 0);
    assert_eq!(status(&state, later), ItemStatus::Later);
    assert_eq!(state.later, [later]);
    assert!(state.hold.is_none());
}

#[test]
fn scenario_13_an_approve_stands_when_the_doc_changes_after_it() {
    let mut s = Scenario::new(DOC);
    s.verdict("- first item", "yes");
    let ap = s.approve();
    s.replace_line("- third item", "- third item, edited");
    let state = s.fold();

    assert_eq!(state.phrase.stage, Stage::Approved);
    let a = state.approve.as_ref().expect("approve facts");
    assert_eq!(a.n, ap);
    assert_eq!(a.rev, Rev::of(DOC.as_bytes()).as_str());
    assert!(!a.current_matches);
    assert!(a.changed_since);
    assert_ne!(state.rev, a.rev);
}

#[test]
fn scenario_13_a_withdrawn_approve_gives_the_drafts_back() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "yes");
    let ap = s.approve();
    s.withdraw(ap);
    let state = s.fold();
    assert!(state.approve.is_none());
    assert_eq!(state.phrase.stage, Stage::Drafts);
    assert_eq!(status(&state, x), ItemStatus::Draft);
    assert_eq!(state.counts.drafts, 1);
}

#[test]
fn a_claimed_approve_opens_no_turn() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.approve();
    s.claim(CLAUDE);
    let state = s.fold();
    assert_eq!(state.phrase.stage, Stage::Approved);
    assert!(state.phrase.picked_up.is_none());
    assert!(!state.approve.as_ref().unwrap().can_withdraw);
    assert_ne!(status(&state, x), ItemStatus::PickedUp);
    assert_eq!(item(&state, x).turn, None);
}

// --- amend, retract, blanks (scenario 14) ---

#[test]
fn scenario_14_a_draft_can_be_amended_and_retracted() {
    let mut s = Scenario::new(DOC);
    let x = s.note("first wording");
    let am = s.amend(x, Some("second wording"), Some("no"));
    let y = s.note("never mind");
    s.retract(y);
    let state = s.fold();

    assert_eq!(state.items.len(), 2, "{:?}", state.items);
    let it = item(&state, x);
    assert_eq!(it.body.as_deref(), Some("second wording"));
    assert_eq!(it.verdict, Some(Verdict::No));
    assert_eq!(it.amended, [am]);
    assert_eq!(it.status, ItemStatus::Draft);
    assert_eq!(status(&state, y), ItemStatus::Retracted);
    assert_eq!(state.counts.drafts, 1);
    assert!(state.warnings.is_empty(), "{:?}", state.warnings);
}

#[test]
fn scenario_14_amend_and_retract_after_the_hand_back_are_ignored() {
    let mut s = Scenario::new(DOC);
    let x = s.note("first wording");
    s.submit();
    s.amend(x, Some("second wording"), None);
    s.retract(x);
    let state = s.fold();
    let it = item(&state, x);
    assert_eq!(it.body.as_deref(), Some("first wording"));
    assert!(it.amended.is_empty());
    assert_eq!(it.status, ItemStatus::Sent);
    assert_eq!(state.warnings.len(), 2, "{:?}", state.warnings);
}

#[test]
fn a_withdrawn_hand_back_makes_its_notes_amendable_again() {
    let mut s = Scenario::new(DOC);
    let x = s.note("first wording");
    let sub = s.submit();
    s.withdraw(sub);
    let am = s.amend(x, Some("second wording"), None);
    let state = s.fold();
    assert_eq!(item(&state, x).body.as_deref(), Some("second wording"));
    assert_eq!(item(&state, x).amended, [am]);
}

#[test]
fn an_amend_of_a_line_that_is_no_human_note_is_ignored() {
    let mut s = Scenario::new(DOC);
    let x = s.note("mine");
    s.submit();
    let turn = s.claim(CLAUDE);
    let blank = s.blank(CLAUDE, json!("doc"), "which order?", turn);
    s.yield_turn(CLAUDE, turn);
    s.amend(blank, Some("hijacked"), None);
    s.amend(999, Some("nowhere"), None);
    let state = s.fold();
    assert_eq!(item(&state, blank).body.as_deref(), Some("which order?"));
    assert!(item(&state, blank).amended.is_empty());
    assert_eq!(item(&state, x).body.as_deref(), Some("mine"));
    assert_eq!(state.warnings.len(), 2, "{:?}", state.warnings);
}

#[test]
fn retracting_the_only_draft_leaves_nothing_to_send() {
    let mut s = Scenario::new(DOC);
    let x = s.note("oops");
    s.retract(x);
    let state = s.fold();
    assert_eq!(state.phrase.stage, Stage::Idle);
    assert_eq!(state.counts.drafts, 0);
}

#[test]
fn scenario_14_a_blank_is_open_until_the_human_answers() {
    let mut s = Scenario::new(DOC);
    let x = s.note("mine");
    s.submit();
    let turn = s.claim(CLAUDE);
    let at = s.block("- third item");
    let blank = s.blank(CLAUDE, at, "which order?", turn);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();

    let b = item(&state, blank);
    assert_eq!(b.by.kind, ByKind::Agent);
    assert_eq!(b.verdict, Some(Verdict::Ask));
    assert_eq!(b.status, ItemStatus::Open);
    assert_eq!(state.counts.open_asks, 1);
    assert_eq!(state.counts.attention, 1);
    assert_eq!(state.counts.drafts, 0);
    // An unanswered blank never makes the turn look unhandled
    assert_eq!(state.counts.not_handled, 0);
    assert_eq!(state.phrase.replied.as_ref().unwrap().asked, 1);

    let r = s.human_reply(blank, None, Some("alphabetical"));
    let state = s.fold();
    let b = item(&state, blank);
    assert_eq!(b.status, ItemStatus::Answered);
    assert_eq!(b.replies.len(), 1);
    assert_eq!(b.replies[0].n, r);
    assert_eq!(b.replies[0].id, format!("#{r}"));
    assert_eq!(b.replies[0].by.kind, ByKind::Human);
    assert_eq!(b.replies[0].body.as_deref(), Some("alphabetical"));
    assert_eq!(state.counts.open_asks, 0);
    assert_eq!(state.counts.attention, 0);
    // The answer goes back with the next hand-back
    assert_eq!(state.counts.drafts, 1);
}

// --- attention and seen ---

#[test]
fn attention_counts_unseen_replies_and_unseen_not_handled_rows() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    let y = s.note("is this needed?");
    s.submit();
    let turn = s.claim(CLAUDE);
    let reply = s.agent_reply(CLAUDE, y, "answer", "yes");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::NotHandled);
    assert_eq!(item(&state, x).id.as_deref(), Some("nh:4"));
    assert_eq!(state.counts.not_handled, 1);
    assert_eq!(state.counts.attention, 2);
    let replied = state.phrase.replied.as_ref().expect("replied facts");
    assert_eq!((replied.answered, replied.not_handled), (1, 1));

    let reply_id = format!("#{reply}");
    s.seen(&[reply_id.as_str()]);
    let state = s.fold();
    assert!(item(&state, y).replies[0].seen);
    assert_eq!(state.counts.attention, 1);

    s.seen(&["nh:4", "99.0", "#1234", "nh:999"]);
    let state = s.fold();
    assert_eq!(state.counts.attention, 0);
    assert_eq!(state.counts.not_handled, 1);
    assert_eq!(status(&state, x), ItemStatus::NotHandled);
}

#[test]
fn only_an_agents_open_question_is_attention_or_unanswered() {
    let mut s = Scenario::new(DOC);
    // Your own question waits on the agent, not on you
    s.verdict("- second item", "ask");
    s.submit();
    let turn = s.claim(CLAUDE);
    let blank = s.blank(CLAUDE, s.block("first item"), "keep it?", turn);
    let state = s.fold();
    assert_eq!(status(&state, blank), ItemStatus::Open, "setup");
    assert_eq!(state.counts.attention, 1);
    assert_eq!(state.counts.open_asks, 1);
}

// --- later carries ---

#[test]
fn a_later_item_carries_until_the_human_decides_it() {
    let mut s = Scenario::new(DOC);
    let later = s.verdict("- detail one", "later");
    s.submit();
    let first = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, first);
    s.note("round two");
    s.submit();
    let second = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, second);
    let state = s.fold();
    assert_eq!(status(&state, later), ItemStatus::Later);
    assert_eq!(state.later, [later]);
    assert_eq!(state.counts.later, 1);

    s.human_reply(later, Some("no"), Some("do it now"));
    let state = s.fold();
    assert!(state.later.is_empty(), "{:?}", state.later);
    assert_eq!(state.counts.later, 0);
    assert_ne!(status(&state, later), ItemStatus::Later);
}

// --- release (scenario 15) ---

/// Two notes sent and claimed by `claude`, then released by the human.
fn released() -> (Scenario, u32, u32, u32, u32) {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    let y = s.verdict("- third item", "no");
    let sub = s.submit();
    let turn = s.claim(CLAUDE);
    s.release(turn);
    (s, x, y, sub, turn)
}

#[test]
fn scenario_15_a_release_sends_the_items_again() {
    let (s, x, y, sub, _) = released();
    let state = s.fold();
    for n in [x, y] {
        assert_eq!(status(&state, n), ItemStatus::Sent, "item {n}");
        assert_eq!(item(&state, n).sent_in, Some(sub));
        assert_eq!(item(&state, n).turn, None);
    }
    assert_eq!(state.counts.not_handled, 0);
    assert_eq!(state.counts.sent_unclaimed, 2);
    assert_eq!(state.phrase.stage, Stage::Sent);
    assert!(state.phrase.picked_up.is_none());
    assert!(state.phrase.replied.is_none());
    let r = &state.rounds[0];
    assert_eq!(r.claim, None);
    assert_eq!(r.claimed_by, None);
    assert!(r.released);
    // A claim named the round, so it can't be withdrawn any more
    assert!(!state.phrase.sent.as_ref().unwrap().can_withdraw);
    assert_eq!(s.scan(ts(99)).claimable_round(ts(99)), Some(sub));
}

#[test]
fn scenario_15_a_late_respond_is_recorded_without_reclaiming() {
    let (mut s, x, y, sub, turn) = released();
    s.done(CLAUDE, x, turn);
    let late = s.late_reply(CLAUDE, y, "answer", "found it late");
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();

    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(item(&state, x).outcome.as_ref().map(|o| o.done), Some(true));
    assert_eq!(status(&state, y), ItemStatus::Handled);
    assert_eq!(item(&state, y).replies[0].n, late);
    assert_eq!(state.rounds[0].claim, None);
    assert_eq!(state.counts.not_handled, 0);
    let now = ts(99);
    assert_eq!(s.scan(now).check_claim(sub, now), Ok(()));
}

#[test]
fn scenario_15_after_a_release_a_new_claim_takes_the_round() {
    let (mut s, x, y, sub, _) = released();
    let second = s.claim_round("other", sub);
    s.done("other", y, second);
    let yld = s.yield_turn("other", second);
    let state = s.fold();

    assert_eq!(status(&state, y), ItemStatus::Handled);
    assert_eq!(item(&state, y).turn, Some(second));
    assert_eq!(status(&state, x), ItemStatus::NotHandled);
    assert_eq!(item(&state, x).turn, Some(second));
    let r = &state.rounds[0];
    assert_eq!(r.claim, Some(second));
    assert_eq!(r.claimed_by.as_deref(), Some("other"));
    assert_eq!(r.r#yield, Some(yld));
    let replied = state.phrase.replied.as_ref().unwrap();
    assert_eq!((replied.turn, replied.by.as_str()), (second, "other"));
}

#[test]
fn scenario_15_a_change_by_the_new_claimant_handles_its_block() {
    let (mut s, x, _, sub, _) = released();
    let second = s.claim_round("other", sub);
    let ext = s.replace_line("- first item", "- first item, reworded");
    s.yield_turn("other", second);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(
        outcome_of(&state, x),
        outcome(false, false, &[change_id(ext)])
    );
}

// --- owner (scenario 16) ---

#[test]
fn scenario_16_the_creation_owner_counts() {
    assert_eq!(Scenario::new(DOC).fold().owner.drafted_by, None);
    assert_eq!(
        Scenario::drafted_by(DOC, CLAUDE)
            .fold()
            .owner
            .drafted_by
            .as_deref(),
        Some(CLAUDE)
    );
    assert_eq!(Scenario::without_owner(DOC).fold().owner.drafted_by, None);
}

#[test]
fn scenario_16_later_owners_count_only_from_the_human_and_the_last_wins() {
    let mut s = Scenario::drafted_by(DOC, CLAUDE);
    s.owner(human(), None);
    assert_eq!(s.fold().owner.drafted_by, None);
    s.owner(human(), Some(CLAUDE));
    assert_eq!(s.fold().owner.drafted_by.as_deref(), Some(CLAUDE));

    s.owner(support::scenario::agent("other"), Some("other"));
    s.owner(support::scenario::birta_by(), None);
    s.raw(json!({"ts": ts(8), "op": "owner", "drafted_by": null}));
    let state = s.fold();
    assert_eq!(state.owner.drafted_by.as_deref(), Some(CLAUDE));
    assert_eq!(state.warnings.len(), 3, "{:?}", state.warnings);
}

#[test]
fn scenario_16_the_creation_line_owner_counts_whoever_wrote_it() {
    // Line 3 is where creation writes the owner
    let mut s = Scenario::without_owner(DOC);
    s.owner(support::scenario::agent(CLAUDE), Some(CLAUDE));
    s.owner(support::scenario::agent("other"), Some("other"));
    assert_eq!(s.fold().owner.drafted_by.as_deref(), Some(CLAUDE));
}

#[test]
fn scenario_16_without_a_creation_owner_only_the_human_decides() {
    let mut s = Scenario::without_owner(DOC);
    s.note("line 3 is a note");
    s.owner(support::scenario::agent(CLAUDE), Some(CLAUDE));
    assert_eq!(s.fold().owner.drafted_by, None);
    s.owner(human(), Some(CLAUDE));
    s.owner(support::scenario::agent("other"), None);
    assert_eq!(s.fold().owner.drafted_by.as_deref(), Some(CLAUDE));
}

// --- where notes point (scenario 17, as far as `fold` goes) ---

#[test]
fn scenario_17_doc_and_dir_notes_have_no_location() {
    let mut s = Scenario::new(DOC);
    let doc = s.note("whole doc");
    let dir = s.comment(json!("dir"), None, Some("whole folder"));
    let state = s.fold();
    assert_eq!(item(&state, doc).at.kind, AtKind::Doc);
    assert_eq!(item(&state, dir).at.kind, AtKind::Dir);
    for n in [doc, dir] {
        let at = &item(&state, n).at;
        assert!(at.loc.is_none() && at.status.is_none() && at.lines.is_none());
    }
}

// --- items and decided ---

#[test]
fn a_verdict_decides_its_item_and_the_items_nested_in_it() {
    let decided = |needle: &str| {
        let mut s = Scenario::new(DOC);
        s.verdict(needle, "yes");
        s.fold().counts.decided
    };
    assert_eq!(decided("- first item"), 1);
    assert_eq!(decided("- second item"), 2);
    assert_eq!(decided("nested item"), 1);
    assert_eq!(decided("| a |"), 1);
    // A paragraph is not an item
    assert_eq!(decided("Intro paragraph"), 0);
}

#[test]
fn a_heading_verdict_decides_the_items_in_its_section() {
    let decided = |heading: &str| {
        let mut s = Scenario::new(DOC);
        let at = s.section(heading);
        s.comment(at, Some("no"), None);
        s.fold().counts.decided
    };
    assert_eq!(decided("## Details"), 4);
    assert_eq!(decided("## More"), 1);
    assert_eq!(decided("# Plan"), 9);
    assert_eq!(decided("# Appendix"), 1);
}

#[test]
fn decided_counts_each_item_once_and_only_live_human_verdicts() {
    let mut s = Scenario::new(DOC);
    s.verdict("- second item", "yes");
    s.verdict("nested item", "no");
    let at = s.section("## Details");
    s.comment(at, Some("yes"), None);
    s.verdict("- detail one", "no");
    assert_eq!(s.fold().counts.decided, 6);

    let mut s = Scenario::new(DOC);
    let gone = s.verdict("- first item", "yes");
    s.retract(gone);
    s.comment(json!("doc"), Some("no"), Some("all of it"));
    s.note("plain, no verdict");
    let at = s.block("- third item");
    s.comment(at, None, Some("plain on an item"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let at = s.block("- detail one");
    s.blank(CLAUDE, at, "keep this?", turn);
    s.yield_turn(CLAUDE, turn);
    assert_eq!(s.fold().counts.decided, 0);
}

#[test]
fn an_amended_verdict_decides_its_item() {
    let mut s = Scenario::new(DOC);
    let at = s.block("- first item");
    let x = s.comment(at, None, Some("hmm"));
    s.amend(x, None, Some("yes"));
    assert_eq!(s.fold().counts.decided, 1);
}

// --- the hand-off line ---

#[test]
fn the_hand_off_line_names_the_absolute_doc_path() {
    assert_eq!(
        hand_off(Path::new("/work/pr.md")),
        "Take a look at my notes: `birta review /work/pr.md`"
    );
}

// --- adversary killers ---

#[test]
fn a_claim_of_a_later_round_takes_the_earlier_unclaimed_round() {
    let mut s = Scenario::new(DOC);
    let a = s.note("first round");
    let first = s.submit();
    let b = s.note("second round");
    s.submit();
    let turn = s.claim(CLAUDE);
    let state = s.fold();
    for n in [a, b] {
        assert_eq!(status(&state, n), ItemStatus::PickedUp, "item {n}");
        assert_eq!(item(&state, n).turn, Some(turn), "item {n}");
    }
    assert_eq!(item(&state, a).sent_in, Some(first));
    assert_eq!(state.counts.sent_unclaimed, 0);
}

#[test]
fn a_retracted_draft_is_not_sent() {
    let mut s = Scenario::new(DOC);
    let x = s.note("never mind");
    s.retract(x);
    let y = s.note("this one");
    s.submit();
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Retracted);
    assert_eq!(item(&state, x).sent_in, None);
    assert_eq!(state.rounds[0].items, [y]);
    assert_eq!(state.phrase.sent.as_ref().unwrap().items, 1);
}

#[test]
fn a_draft_amended_to_later_carries() {
    let mut s = Scenario::new(DOC);
    let at = s.block("- first item");
    let x = s.comment(at, None, Some("not now"));
    s.amend(x, None, Some("later"));
    s.submit();
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Later);
    assert_eq!(state.later, [x]);
    assert_eq!(state.counts.not_handled, 0);
}

#[test]
fn a_verdict_amend_after_the_hand_back_is_ignored() {
    let mut s = Scenario::new(DOC);
    let at = s.block("- first item");
    let x = s.comment(at, None, Some("hmm"));
    s.submit();
    s.amend(x, None, Some("yes"));
    let state = s.fold();
    assert_eq!(item(&state, x).verdict, None);
    assert_eq!(state.counts.decided, 0);
    assert_eq!(state.warnings.len(), 1, "{:?}", state.warnings);
}

#[test]
fn another_agents_late_reply_after_a_release_settles_nothing() {
    // After a release, only the released claim's agent may answer late
    let (mut s, x, _, _, _) = released();
    s.late_reply("other", x, "answer", "never claimed");
    assert_eq!(status(&s.fold(), x), ItemStatus::Sent);
}

#[test]
fn the_phrase_before_the_hold_keeps_an_earlier_unclaimed_round() {
    let mut s = Scenario::new(DOC);
    s.note("first round");
    let first = s.submit();
    s.note("second round");
    let held = s.submit_held();
    let state = s.fold();
    let hold = state.hold.as_ref().expect("a pending hold");
    assert_eq!((hold.n, hold.count), (held, 1));
    let before = state
        .phrase_before_hold
        .as_ref()
        .expect("phrase before hold");
    assert_eq!(before.stage, Stage::Sent);
    assert_eq!(before.sent.as_ref().unwrap().n, first);
}

#[test]
fn decided_pins_later_and_ask() {
    let decided = |verdict: &str| {
        let mut s = Scenario::new(DOC);
        s.verdict("- first item", verdict);
        s.fold().counts.decided
    };
    assert_eq!(decided("later"), 1);
    assert_eq!(decided("ask"), 0);
}

#[test]
fn a_later_item_survives_a_reply_without_a_verdict() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- detail one", "later");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.agent_reply(CLAUDE, x, "answer", "noted for later");
    s.yield_turn(CLAUDE, turn);
    s.human_reply(x, None, Some("still later"));
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Later);
    assert_eq!(state.later, [x]);
}

#[test]
fn a_seen_blank_still_needs_attention() {
    let mut s = Scenario::new(DOC);
    let x = s.note("mine");
    s.submit();
    let turn = s.claim(CLAUDE);
    let blank = s.blank(CLAUDE, json!("doc"), "which order?", turn);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let id = format!("#{blank}");
    s.seen(&[id.as_str()]);
    let state = s.fold();
    assert_eq!(state.counts.open_asks, 1);
    assert_eq!(state.counts.attention, 1);
}

#[test]
fn a_second_approve_is_the_one_reported() {
    let mut s = Scenario::new(DOC);
    s.approve();
    s.replace_line("- third item", "- third item, edited");
    let second = s.approve();
    let state = s.fold();
    let a = state.approve.as_ref().expect("approve facts");
    assert_eq!(a.n, second);
    assert!(a.current_matches);
    assert!(!a.changed_since);
}

#[test]
fn counts_items_reads_the_current_text() {
    let mut s = Scenario::new(DOC);
    s.replace_line("- third item", "- third item\n- fourth item");
    assert_eq!(s.fold().counts.items, 11);
}

#[test]
fn the_fold_is_deterministic_and_ordered() {
    let (mut s, ..) = full_turn();
    s.verdict("- more item", "later");
    s.verdict("| a |", "later");
    // The approve covers the two new ones, so all three carry
    s.approve();
    let json = || serde_json::to_string(&s.fold()).unwrap();
    assert_eq!(json(), json());
    let state = s.fold();
    assert!(state.items.windows(2).all(|w| w[0].n < w[1].n));
    assert_eq!(state.later.len(), 3, "{:?}", state.later);
    assert!(state.later.windows(2).all(|w| w[0] < w[1]));
}

// --- two agents holding turns at once ---

/// `x` sent and claimed by `claude`, then `z` sent and claimed by `other`.
fn two_turns() -> (Scenario, u32, u32, u32, u32) {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let first = s.claim(CLAUDE);
    let z = s.verdict("- third item", "no");
    s.submit();
    let second = s.claim("other");
    (s, x, z, first, second)
}

#[test]
fn a_change_goes_to_the_latest_live_claim() {
    let (mut s, x, z, first, second) = two_turns();
    s.replace_line("- first item", "- first item, reworded");
    let ext = s.replace_line("- third item", "- third item, reworded");
    s.yield_turn("other", second);
    s.yield_turn(CLAUDE, first);
    let state = s.fold();
    assert_eq!(item(&state, x).turn, Some(first));
    assert_eq!(item(&state, z).turn, Some(second));
    assert_eq!(status(&state, x), ItemStatus::NotHandled);
    assert_eq!(status(&state, z), ItemStatus::Handled);
    assert_eq!(
        outcome_of(&state, z),
        outcome(false, false, &[change_id(ext)])
    );
}

#[test]
fn a_reply_goes_to_the_turn_its_author_claimed() {
    let (mut s, x, z, first, second) = two_turns();
    s.agent_reply("other", x, "answer", "not mine to answer");
    s.agent_reply(CLAUDE, z, "answer", "not mine either");
    s.yield_turn("other", second);
    s.yield_turn(CLAUDE, first);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::NotHandled);
    assert_eq!(status(&state, z), ItemStatus::NotHandled);

    let (mut s, x, z, first, second) = two_turns();
    s.agent_reply(CLAUDE, x, "answer", "mine");
    s.agent_reply("other", z, "answer", "mine too");
    s.yield_turn("other", second);
    s.yield_turn(CLAUDE, first);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(status(&state, z), ItemStatus::Handled);
}

// --- size ---

#[test]
#[ignore = "timing; run in CI with --ignored"]
fn a_ten_thousand_op_log_folds_in_time() {
    let mut s = Scenario::new(DOC);
    let notes: Vec<u32> = (0..5_000).map(|i| s.note(&format!("note {i}"))).collect();
    s.submit();
    let turn = s.claim(CLAUDE);
    for &n in &notes {
        s.done(CLAUDE, n, turn);
    }
    s.yield_turn(CLAUDE, turn);
    let (parsed, chain, doc) = s.build();
    let start = std::time::Instant::now();
    let state = birta::review::fold::fold(&parsed, &chain, &doc).state;
    let took = start.elapsed();
    assert_eq!(state.phrase.replied.as_ref().unwrap().done, 5_000);
    let bound = if cfg!(debug_assertions) { 20 } else { 2 };
    assert!(took.as_secs_f64() < f64::from(bound), "{took:?}");
}

#[test]
fn a_hunk_over_several_blocks_handles_the_note_on_its_last_one() {
    // One hunk replaces lines 6-9; the note is on line 9 only
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- third item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = s.text().replace(
        "- first item\n- second item\n  - nested item\n- third item\n",
        "- one\n- two\n  - three\n- four\n",
    );
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(
        outcome_of(&state, x),
        outcome(false, false, &[change_id(ext)])
    );
}

#[test]
fn a_late_reply_after_the_agents_own_yield_settles_without_reopening_the_turn() {
    let mut s = Scenario::new(DOC);
    let ask = s.comment(s.quote("detail one"), Some("ask"), Some("right?"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let yielded = s.yield_turn(CLAUDE, turn);
    let before = s.fold();
    assert_eq!(status(&before, ask), ItemStatus::NotHandled);

    let late = s.late_reply(CLAUDE, ask, "answer", "forgot this one");
    let after = s.fold();
    assert_eq!(status(&after, ask), ItemStatus::Handled);
    assert_eq!(item(&after, ask).replies[0].n, late);
    assert_eq!(after.phrase.stage, before.phrase.stage);
    assert_eq!(
        (after.rounds[0].claim, after.rounds[0].r#yield),
        (Some(turn), Some(yielded)),
        "still the same ended turn"
    );
    assert_eq!(s.scan(ts(99)).claimable_round(ts(99)), None);

    // Only the late flag lets it in, and only from the turn's own agent.
    let mut s = Scenario::new(DOC);
    let ask = s.comment(s.quote("detail one"), Some("ask"), Some("right?"));
    s.submit();
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    s.agent_reply(CLAUDE, ask, "answer", "not flagged late");
    s.late_reply("other", ask, "answer", "never claimed");
    assert_eq!(status(&s.fold(), ask), ItemStatus::NotHandled);
}

/// A doc note with a `covers` citing text the turn wrote.
fn covered(quote: &str) -> (ReviewState, u32) {
    let mut s = Scenario::new(DOC);
    let doc = s.note("say what the retries do");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = format!("{}\nRetries now use jitter.\n", s.text());
    s.external(&text);
    s.covers(CLAUDE, doc, quote, turn);
    s.yield_turn(CLAUDE, turn);
    (s.fold(), doc)
}

#[test]
fn a_covers_that_binds_handles_its_doc_note() {
    let (state, doc) = covered("Retries now use jitter");
    assert_eq!(status(&state, doc), ItemStatus::Handled);
    assert_eq!(state.counts.not_handled, 0);
}

#[test]
fn a_covers_citing_text_the_turn_did_not_write_is_no_outcome() {
    let (state, doc) = covered("Intro paragraph here");
    assert_eq!(status(&state, doc), ItemStatus::NotHandled);
    let (state, doc) = covered("jitter everywhere");
    assert_eq!(status(&state, doc), ItemStatus::NotHandled);
}

#[test]
fn a_late_done_after_the_agents_own_yield_counts_and_an_unflagged_one_does_not() {
    for late in [true, false] {
        let mut s = Scenario::new(DOC);
        let x = s.verdict("- first item", "no");
        s.submit();
        let turn = s.claim(CLAUDE);
        s.yield_turn(CLAUDE, turn);
        let stage = s.fold().phrase.stage;
        s.push(
            support::scenario::agent(CLAUDE),
            json!({"op": "done", "to": x, "turn": turn, "late": late}),
        );
        let state = s.fold();
        let want = if late {
            ItemStatus::Handled
        } else {
            ItemStatus::NotHandled
        };
        assert_eq!(status(&state, x), want, "late: {late}");
        assert_eq!(state.phrase.stage, stage, "late: {late}");
    }
}

#[test]
fn a_note_its_turn_left_open_is_carried_to_the_next_claim() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let first = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, first);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::NotHandled);
    assert_eq!(
        item(&state, x).turn,
        Some(first),
        "no later claim to carry it to"
    );

    let y = s.note("round two");
    s.submit();
    let second = s.claim("other");
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::PickedUp);
    assert_eq!(item(&state, x).turn, Some(second));
    assert_eq!(status(&state, y), ItemStatus::PickedUp);

    s.done("other", x, second);
    s.done("other", y, second);
    s.yield_turn("other", second);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(state.counts.not_handled, 0);
}

#[test]
fn a_carried_note_left_open_again_moves_on_once_more() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let first = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, first);
    s.note("round two");
    s.submit();
    let second = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, second);
    assert_eq!(status(&s.fold(), x), ItemStatus::NotHandled);
    s.note("round three");
    s.submit();
    let third = s.claim(CLAUDE);
    s.done(CLAUDE, x, third);
    s.yield_turn(CLAUDE, third);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert_eq!(item(&state, x).turn, Some(third));
}

#[test]
fn a_note_settled_only_by_a_covers_stays_with_its_turn() {
    let mut s = Scenario::drafted_by(DOC, CLAUDE);
    let doc = s.note("say how retries back off");
    s.submit();
    let first = s.claim(CLAUDE);
    let text = s.replaced(
        "- third item",
        "- third item\n\nRetries back off with jitter.",
    );
    s.external(&text);
    s.covers(CLAUDE, doc, "Retries back off with jitter", first);
    s.yield_turn(CLAUDE, first);
    let settled = item(&s.fold(), doc).clone();
    assert_eq!(
        (settled.status, settled.turn),
        (ItemStatus::Handled, Some(first))
    );

    let round_two = s.note("round two");
    s.submit();
    let second = s.claim(CLAUDE);
    let picked_up = item(&s.fold(), doc).clone();
    assert_eq!(
        (picked_up.status, picked_up.turn),
        (ItemStatus::Handled, Some(first)),
        "not carried to the next claim"
    );
    s.done(CLAUDE, round_two, second);
    s.yield_turn(CLAUDE, second);
    let state = s.fold();
    assert_eq!(
        (status(&state, doc), item(&state, doc).turn),
        (ItemStatus::Handled, Some(first))
    );
    assert_eq!(state.counts.not_handled, 0);
    let replied = state.phrase.replied.as_ref().unwrap();
    assert_eq!(
        (replied.turn, replied.done, replied.not_handled),
        (second, 1, 0),
        "round two's tally counts only its own note"
    );
}

#[test]
fn a_covers_quote_inside_another_notes_hunk_still_handles_the_doc_note() {
    let mut s = Scenario::drafted_by(DOC, CLAUDE);
    let at = s.quote("second item");
    let spot = s.comment(at, None, Some("reword this"));
    let doc = s.note("say how retries back off");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = s.replaced("- second item", "- second item, retried with jitter");
    s.external(&text);
    s.done(CLAUDE, spot, turn);
    s.covers(CLAUDE, doc, "retried with jitter", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(
        (status(&state, spot), status(&state, doc)),
        (ItemStatus::Handled, ItemStatus::Handled)
    );
    assert_eq!(state.counts.not_handled, 0);
}

#[test]
fn covers_binds_to_the_later_hunk_that_wrote_its_quote() {
    let mut s = Scenario::drafted_by(DOC, CLAUDE);
    let doc = s.note("add a closing line");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = s
        .replaced("Intro paragraph here,", "Intro paragraph, reworded,")
        .replacen(
            "- appendix item\n",
            "- appendix item\n\nShips behind a flag.\n",
            1,
        );
    let ext = s.external(&text);
    s.covers(CLAUDE, doc, "Ships behind a flag", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    let cited: Vec<_> = state
        .changes
        .iter()
        .filter(|c| c.asked.kind == AskedKind::Cited)
        .collect();
    assert_eq!(cited.len(), 1, "{cited:#?}");
    assert_eq!(cited[0].id, format!("{ext}.1"));
    assert!(
        s.text()[cited[0].range.bytes.0..cited[0].range.bytes.1].contains("Ships behind a flag")
    );
    assert_eq!(status(&state, doc), ItemStatus::Handled);
}

#[test]
fn covers_on_an_outside_save_does_not_handle_the_note() {
    let mut s = Scenario::drafted_by(DOC, CLAUDE);
    let doc = s.note("link the issue");
    s.submit();
    let text = format!("{}\nCloses #408.\n", s.text());
    s.external(&text);
    let turn = s.claim(CLAUDE);
    s.covers(CLAUDE, doc, "Closes #408", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(status(&state, doc), ItemStatus::NotHandled);
    assert!(
        state
            .changes
            .iter()
            .all(|c| c.asked.kind != AskedKind::Cited)
    );
}

#[test]
fn an_omitted_quote_note_is_not_handled() {
    let mut s = Scenario::drafted_by(DOC, CLAUDE);
    let at = s.quote("detail two");
    let left = s.comment(at, None, Some("why two?"));
    let other = s.note("tighten the intro");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = s.replaced("Intro paragraph here,", "Intro, tightened,");
    s.external(&text);
    s.covers(CLAUDE, other, "Intro, tightened", turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(status(&state, left), ItemStatus::NotHandled);
    assert_eq!(status(&state, other), ItemStatus::Handled);
    assert_eq!(state.counts.not_handled, 1);
}

// --- dropping a verdict, and notes seen by their own id ---

#[test]
fn an_amend_that_drops_the_verdict_keeps_the_body_and_undecides_the_item() {
    let mut s = Scenario::new(DOC);
    let at = s.block("first item");
    let x = s.comment(at, Some("no"), Some("drop it"));
    assert_eq!(
        s.fold().counts.decided,
        1,
        "setup: the verdict decides its item"
    );
    let am = s.push(
        human(),
        json!({"op": "amend", "to": x, "drop_verdict": true}),
    );
    let state = s.fold();
    let it = item(&state, x);
    assert_eq!(it.verdict, None);
    assert_eq!(it.body.as_deref(), Some("drop it"));
    assert_eq!(it.amended, [am]);
    assert_eq!(it.status, ItemStatus::Draft);
    assert_eq!(state.counts.decided, 0);
    assert!(state.warnings.is_empty(), "{:?}", state.warnings);
}

#[test]
fn a_verdict_dropped_after_the_hand_back_is_ignored() {
    let mut s = Scenario::new(DOC);
    let at = s.block("first item");
    let x = s.comment(at, Some("no"), Some("drop it"));
    s.submit();
    s.push(
        human(),
        json!({"op": "amend", "to": x, "drop_verdict": true}),
    );
    let state = s.fold();
    assert_eq!(item(&state, x).verdict, Some(Verdict::No));
    assert_eq!(state.warnings.len(), 1, "{:?}", state.warnings);
}

/// `items[].seen` as the page reads it: absent counts as false.
fn note_seen(state: &ReviewState, n: u32) -> bool {
    serde_json::to_value(item(state, n)).unwrap()["seen"] == json!(true)
}

/// A note the agent's turn marked done, and that turn's claim.
fn handled_note() -> (Scenario, u32) {
    let mut s = Scenario::drafted_by(DOC, CLAUDE);
    let x = s.note("tighten the intro");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    (s, x)
}

#[test]
fn a_handled_note_seen_by_its_own_id_is_seen() {
    let (mut s, x) = handled_note();
    assert_eq!(status(&s.fold(), x), ItemStatus::Handled);
    assert!(!note_seen(&s.fold(), x), "unseen until the seen line");
    s.seen(&[&format!("#{x}")]);
    let state = s.fold();
    assert!(note_seen(&state, x));
    assert!(state.warnings.is_empty(), "{:?}", state.warnings);
}

#[test]
fn a_reply_after_the_seen_makes_the_note_unseen_again() {
    let (mut s, x) = handled_note();
    s.seen(&[&format!("#{x}")]);
    s.late_reply(CLAUDE, x, "note", "one more thing");
    assert!(!note_seen(&s.fold(), x));
    // Seeing it again counts from the newest outcome
    s.seen(&[&format!("#{x}")]);
    assert!(note_seen(&s.fold(), x));
}

#[test]
fn a_seen_before_the_notes_outcome_does_not_count() {
    let mut s = Scenario::drafted_by(DOC, CLAUDE);
    let x = s.note("tighten the intro");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.seen(&[&format!("#{x}")]);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::Handled);
    assert!(!note_seen(&state, x));
}

#[test]
fn only_the_human_marks_a_note_seen() {
    let (mut s, x) = handled_note();
    s.push(
        support::scenario::agent(CLAUDE),
        json!({"op": "seen", "ids": [format!("#{x}")]}),
    );
    assert!(!note_seen(&s.fold(), x));
    // Control: the same line from the human counts
    s.seen(&[&format!("#{x}")]);
    assert!(note_seen(&s.fold(), x));
}

/// A verdict the agent's turn left alone, and that turn's claim.
fn not_handled_note() -> (Scenario, u32) {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    (s, x)
}

#[test]
fn a_not_handled_row_seen_by_its_nh_id_is_seen_and_leaves_attention() {
    let (mut s, x) = not_handled_note();
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::NotHandled, "setup");
    assert!(!note_seen(&state, x), "unseen until the seen line");
    assert_eq!(state.counts.attention, 1, "setup: the row needs you");
    s.seen(&[&format!("nh:{x}")]);
    let state = s.fold();
    assert!(note_seen(&state, x));
    assert_eq!(state.counts.attention, 0);
    // Acknowledged, not handled: the status and the count stay
    assert_eq!(status(&state, x), ItemStatus::NotHandled);
    assert_eq!(state.counts.not_handled, 1);
    assert!(state.warnings.is_empty(), "{:?}", state.warnings);
}

#[test]
fn a_seen_before_the_turn_that_left_a_note_not_handled_does_not_count() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.seen(&[&format!("nh:{x}")]);
    s.yield_turn(CLAUDE, turn);
    let state = s.fold();
    assert_eq!(status(&state, x), ItemStatus::NotHandled);
    assert!(!note_seen(&state, x));
    assert_eq!(state.counts.attention, 1);
}

#[test]
fn only_the_human_acknowledges_a_not_handled_note() {
    let (mut s, x) = not_handled_note();
    s.push(
        support::scenario::agent(CLAUDE),
        json!({"op": "seen", "ids": [format!("nh:{x}")]}),
    );
    let state = s.fold();
    assert!(!note_seen(&state, x));
    assert_eq!(state.counts.attention, 1);
}
