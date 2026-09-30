use birta::review::Reject;
use birta::review::fold::{RoundKind, TurnIndex, scan};
use birta::review::ops::{self, Parsed};
use serde_json::{Value, json};

const HEADER: &str = r#"{"birta":"review/1","doc":"pr.md","about":"ops after the last hand-back are drafts; don't act on them"}"#;
const REV_A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";

/// Header, snap and owner on lines 1-3, then `ops` from line 4.
fn parse(ops: &[Value]) -> Parsed {
    let mut text = format!(
        "{HEADER}\n{}\n{}\n",
        json!({"ts": 1, "by": {"type": "birta"}, "op": "snap", "rev": "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391", "text": ""}),
        json!({"ts": 1, "by": {"type": "birta"}, "op": "owner", "drafted_by": null}),
    );
    for op in ops {
        text.push_str(&op.to_string());
        text.push('\n');
    }
    let parsed = ops::parse(&text);
    assert!(
        parsed.bad.is_empty() && parsed.unknown.is_empty() && !parsed.read_only,
        "test log must parse cleanly: {:?}",
        parsed.bad
    );
    parsed
}

fn idx(ops: &[Value], now: u64) -> TurnIndex {
    scan(&parse(ops), now)
}

fn human() -> Value {
    json!({"type": "human", "via": "browser"})
}

fn agent() -> Value {
    json!({"type": "agent", "name": "claude", "via": "cli"})
}

fn note(ts: u64) -> Value {
    json!({"ts": ts, "by": human(), "op": "comment", "at": "doc", "body": "tighten this"})
}

fn submit(ts: u64, upto: u32, not_before: u64) -> Value {
    json!({"ts": ts, "by": human(), "op": "submit", "upto": upto, "not_before": not_before, "hold_ms": 5000})
}

fn approve(ts: u64, not_before: u64) -> Value {
    json!({"ts": ts, "by": human(), "op": "approve", "rev": REV_A, "not_before": not_before, "hold_ms": 5000})
}

fn claim(ts: u64, round: u32) -> Value {
    json!({"ts": ts, "by": agent(), "op": "claim", "round": round})
}

fn withdraw(ts: u64, to: u32) -> Value {
    json!({"ts": ts, "by": human(), "op": "withdraw", "to": to})
}

// --- rounds ---

#[test]
fn submits_and_approves_are_rounds_with_their_claims_and_withdrawals() {
    // 4 note, 5 submit, 6 claim(5), 7 note, 8 submit, 9 withdraw(8), 10 approve
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            claim(8_000, 5),
            note(9_000),
            submit(10_000, 7, 15_000),
            withdraw(11_000, 8),
            approve(12_000, 17_000),
        ],
        20_000,
    );
    let rounds: Vec<_> = t
        .rounds
        .iter()
        .map(|r| (r.n, r.kind, r.not_before, r.claim, r.withdrawn))
        .collect();
    assert_eq!(
        rounds,
        [
            (5, RoundKind::Submit, 7_000, Some(6), None),
            (8, RoundKind::Submit, 15_000, None, Some(9)),
            (10, RoundKind::Approve, 17_000, None, None),
        ]
    );
}

// --- the claim CAS ---

#[test]
fn a_claim_before_not_before_is_refused_and_at_not_before_is_accepted() {
    let ops = [note(1_000), submit(2_000, 4, 7_000)];
    assert_eq!(idx(&ops, 6_999).check_claim(5, 6_999), Err(Reject::NotYet));
    assert_eq!(idx(&ops, 6_999).claimable_round(6_999), None);
    assert_eq!(idx(&ops, 7_000).check_claim(5, 7_000), Ok(()));
    assert_eq!(idx(&ops, 7_000).claimable_round(7_000), Some(5));
    // The clock that decides is the one passed in, not the one scan saw.
    assert_eq!(idx(&ops, 7_000).check_claim(5, 6_999), Err(Reject::NotYet));
    assert_eq!(idx(&ops, 6_999).claimable_round(7_000), Some(5));
}

#[test]
fn a_claimed_round_cannot_be_claimed_again() {
    let t = idx(
        &[note(1_000), submit(2_000, 4, 7_000), claim(8_000, 5)],
        9_000,
    );
    assert_eq!(t.check_claim(5, 9_000), Err(Reject::AlreadyClaimed));
    assert_eq!(t.claimable_round(9_000), None);
}

#[test]
fn a_withdrawn_round_is_not_claimable() {
    let t = idx(
        &[note(1_000), submit(2_000, 4, 7_000), withdraw(3_000, 5)],
        9_000,
    );
    assert!(matches!(t.check_claim(5, 9_000), Err(Reject::Invalid(_))));
    assert_eq!(t.claimable_round(9_000), None);
}

#[test]
fn a_claim_must_name_a_submit_or_approve_line() {
    let t = idx(&[note(1_000), submit(2_000, 4, 7_000)], 9_000);
    for round in [0, 1, 2, 3, 4, 6, 99] {
        assert!(
            matches!(t.check_claim(round, 9_000), Err(Reject::Invalid(_))),
            "round {round}"
        );
    }
}

#[test]
fn an_approve_is_claimable_like_a_submit() {
    let ops = [note(1_000), approve(2_000, 7_000)];
    assert_eq!(idx(&ops, 6_999).check_claim(5, 6_999), Err(Reject::NotYet));
    assert_eq!(idx(&ops, 7_000).claimable_round(7_000), Some(5));
    let claimed = idx(
        &[note(1_000), approve(2_000, 7_000), claim(8_000, 5)],
        9_000,
    );
    assert_eq!(claimed.check_claim(5, 9_000), Err(Reject::AlreadyClaimed));
}

#[test]
fn the_latest_due_round_is_the_claimable_one() {
    // 4 note, 5 submit, 6 note, 7 submit: both due and unclaimed.
    let ops = [
        note(1_000),
        submit(2_000, 4, 7_000),
        note(3_000),
        submit(4_000, 6, 9_000),
    ];
    assert_eq!(idx(&ops, 9_000).claimable_round(9_000), Some(7));
    // Before the second is due, the first still is.
    assert_eq!(idx(&ops, 8_000).claimable_round(8_000), Some(5));
}

#[test]
fn a_claim_on_a_later_round_takes_the_earlier_rounds_with_it() {
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            note(3_000),
            submit(4_000, 6, 9_000),
            claim(10_000, 7),
        ],
        11_000,
    );
    assert_eq!(t.check_claim(5, 11_000), Err(Reject::AlreadyClaimed));
    assert_eq!(t.claimable_round(11_000), None);
    assert_eq!(t.pending_deadline(), None);
}

#[test]
fn a_claim_on_an_earlier_round_leaves_the_later_one_claimable() {
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            claim(7_500, 5),
            note(8_000),
            submit(8_500, 7, 13_500),
        ],
        13_500,
    );
    assert_eq!(t.check_claim(8, 13_500), Ok(()));
    assert_eq!(t.claimable_round(13_500), Some(8));
    assert_eq!(t.claimable_round(13_499), None);
}

#[test]
fn a_withdrawn_later_round_leaves_the_earlier_one_claimable() {
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            note(3_000),
            submit(4_000, 6, 9_000),
            withdraw(5_000, 7),
        ],
        9_000,
    );
    assert_eq!(t.claimable_round(9_000), Some(5));
}

// --- withdraw ---

#[test]
fn withdraw_works_until_a_claim_names_the_round() {
    let held = [note(1_000), submit(2_000, 4, 7_000)];
    // During the hold, after it, and with no clock at all: only the claim matters.
    assert_eq!(idx(&held, 3_000).check_withdraw(5), Ok(()));
    assert_eq!(idx(&held, 60_000).check_withdraw(5), Ok(()));
    let claimed = idx(
        &[note(1_000), submit(2_000, 4, 7_000), claim(8_000, 5)],
        9_000,
    );
    assert_eq!(claimed.check_withdraw(5), Err(Reject::NotWithdrawable));
    let approved = idx(&[note(1_000), approve(2_000, 7_000)], 3_000);
    assert_eq!(approved.check_withdraw(5), Ok(()));
    let approve_claimed = idx(
        &[note(1_000), approve(2_000, 7_000), claim(8_000, 5)],
        9_000,
    );
    assert_eq!(
        approve_claimed.check_withdraw(5),
        Err(Reject::NotWithdrawable)
    );
}

#[test]
fn a_round_is_withdrawn_once() {
    let t = idx(
        &[note(1_000), submit(2_000, 4, 7_000), withdraw(3_000, 5)],
        4_000,
    );
    assert_eq!(t.check_withdraw(5), Err(Reject::NotWithdrawable));
}

#[test]
fn withdraw_must_name_a_submit_or_approve_line() {
    let t = idx(&[note(1_000), submit(2_000, 4, 7_000)], 3_000);
    for to in [0, 1, 2, 3, 4, 6, 99] {
        assert_eq!(
            t.check_withdraw(to),
            Err(Reject::NotWithdrawable),
            "to {to}"
        );
    }
}

// --- the submit guard ---

#[test]
fn a_submit_needs_a_human_draft_at_or_before_upto() {
    assert_eq!(idx(&[note(1_000)], 2_000).check_submit(4, 2_000), Ok(()));
    // The note on line 4 is past `upto`.
    assert_eq!(
        idx(&[note(1_000)], 2_000).check_submit(3, 2_000),
        Err(Reject::NothingToSend)
    );
    assert_eq!(
        idx(&[], 2_000).check_submit(3, 2_000),
        Err(Reject::NothingToSend)
    );
}

#[test]
fn a_human_reply_is_a_draft_too() {
    let reply = json!({"ts": 1_000, "by": human(), "op": "reply", "to": 3, "body": "yes, do it"});
    assert_eq!(idx(&[reply], 2_000).check_submit(4, 2_000), Ok(()));
}

#[test]
fn agent_notes_seen_and_owner_lines_are_not_drafts() {
    let ops = [
        json!({"ts": 1_000, "by": agent(), "op": "comment", "at": "doc", "verdict": "ask", "body": "which one?"}),
        json!({"ts": 1_000, "by": human(), "op": "seen", "ids": ["4.0"]}),
        json!({"ts": 1_000, "by": human(), "op": "owner", "drafted_by": "claude"}),
    ];
    assert_eq!(
        idx(&ops, 2_000).check_submit(6, 2_000),
        Err(Reject::NothingToSend)
    );
}

#[test]
fn a_second_submit_with_no_new_drafts_is_nothing_to_send() {
    let t = idx(&[note(1_000), submit(2_000, 4, 7_000)], 8_000);
    assert_eq!(t.check_submit(5, 8_000), Err(Reject::NothingToSend));
    let after_approve = idx(&[note(1_000), approve(2_000, 7_000)], 8_000);
    assert_eq!(
        after_approve.check_submit(5, 8_000),
        Err(Reject::NothingToSend)
    );
}

#[test]
fn a_note_kept_after_the_submit_is_a_new_draft() {
    let t = idx(&[note(1_000), submit(2_000, 4, 7_000), note(8_000)], 9_000);
    assert_eq!(t.check_submit(6, 9_000), Ok(()));
}

#[test]
fn a_withdrawn_submit_gives_its_drafts_back() {
    let t = idx(
        &[note(1_000), submit(2_000, 4, 7_000), withdraw(3_000, 5)],
        4_000,
    );
    assert_eq!(t.check_submit(4, 4_000), Ok(()));
}

#[test]
fn a_second_hand_back_during_a_hold_is_hold_pending() {
    let ops = [note(1_000), submit(2_000, 4, 7_000), note(3_000)];
    assert_eq!(
        idx(&ops, 6_999).check_submit(6, 6_999),
        Err(Reject::HoldPending)
    );
    assert_eq!(idx(&ops, 7_000).check_submit(6, 7_000), Ok(()));
    // The hold outranks having nothing new.
    assert_eq!(
        idx(&ops[..2], 6_999).check_submit(5, 6_999),
        Err(Reject::HoldPending)
    );
    // A withdrawn hold is no hold.
    let withdrawn = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            withdraw(2_500, 5),
            note(3_000),
        ],
        4_000,
    );
    assert_eq!(withdrawn.check_submit(7, 4_000), Ok(()));
}

// --- clock skew clamp and deadlines ---

#[test]
fn a_not_before_more_than_a_minute_past_its_ts_is_clamped_to_five_seconds() {
    let exact = [note(1_000), submit(2_000, 4, 62_000)];
    assert_eq!(
        idx(&exact, 61_999).check_claim(5, 61_999),
        Err(Reject::NotYet)
    );
    assert_eq!(idx(&exact, 62_000).check_claim(5, 62_000), Ok(()));
    let skewed = [note(1_000), submit(2_000, 4, 62_001)];
    assert_eq!(
        idx(&skewed, 6_999).check_claim(5, 6_999),
        Err(Reject::NotYet)
    );
    assert_eq!(idx(&skewed, 7_000).check_claim(5, 7_000), Ok(()));
    assert_eq!(idx(&skewed, 7_000).claimable_round(7_000), Some(5));
    assert_eq!(idx(&skewed, 7_000).pending_deadline(), Some(7_000));
    assert_eq!(idx(&skewed, 7_000).rounds[0].not_before, 7_000);
    assert_eq!(
        idx(&skewed, 7_000).check_submit(5, 7_000),
        Err(Reject::NothingToSend)
    );
    let far = [note(1_000), submit(2_000, 4, u64::MAX)];
    assert_eq!(idx(&far, 7_000).claimable_round(7_000), Some(5));
}

#[test]
fn an_approve_not_before_is_clamped_the_same_way() {
    let skewed = [note(1_000), approve(2_000, 62_001)];
    assert_eq!(
        idx(&skewed, 6_999).check_claim(5, 6_999),
        Err(Reject::NotYet)
    );
    assert_eq!(idx(&skewed, 7_000).claimable_round(7_000), Some(5));
    assert_eq!(idx(&skewed, 7_000).pending_deadline(), Some(7_000));
}

fn untimed(not_before: u64) -> Value {
    json!({"by": human(), "op": "submit", "upto": 4, "not_before": not_before, "hold_ms": 5000})
}

#[test]
fn a_hand_written_submit_without_ts_is_due_at_once() {
    for not_before in [0, 1_500, 61_000, u64::MAX] {
        let t = idx(&[note(1_000), untimed(not_before)], 1_000);
        assert_eq!(t.check_claim(5, 1_000), Ok(()), "{not_before}");
        assert_eq!(t.claimable_round(1_000), Some(5), "{not_before}");
        assert!(
            t.pending_deadline().is_some_and(|d| d <= 1_000),
            "{not_before}"
        );
        assert_eq!(
            t.check_submit(5, 1_000),
            Err(Reject::NothingToSend),
            "{not_before}"
        );
    }
}

#[test]
fn once_due_a_round_stays_due() {
    let logs = [
        vec![note(1_000), submit(2_000, 4, 7_000)],
        vec![note(1_000), submit(2_000, 4, 62_000)],
        vec![note(1_000), submit(2_000, 4, 62_001)],
        vec![note(1_000), approve(2_000, 62_001)],
        vec![note(1_000), untimed(61_001)],
        vec![note(1_000), untimed(30_000)],
    ];
    for ops in &logs {
        let due = |now: u64| idx(ops, now).claimable_round(now) == Some(5);
        for t in (0..130_000).step_by(500) {
            if due(t) {
                for later in [t + 1, t + 5_000, t + 120_000] {
                    assert!(due(later), "{ops:?}: due at {t}, not at {later}");
                }
            }
        }
        assert!(due(130_000), "{ops:?} never came due");
    }
}

#[test]
fn the_pending_deadline_is_the_earliest_unclaimed_unwithdrawn_not_before() {
    assert_eq!(idx(&[note(1_000)], 2_000).pending_deadline(), None);
    let two = [
        note(1_000),
        submit(2_000, 4, 9_000),
        note(3_000),
        submit(4_000, 6, 8_000),
    ];
    assert_eq!(idx(&two, 5_000).pending_deadline(), Some(8_000));
    let withdrawn = [
        note(1_000),
        submit(2_000, 4, 9_000),
        note(3_000),
        submit(4_000, 6, 8_000),
        withdraw(4_500, 7),
    ];
    assert_eq!(idx(&withdrawn, 5_000).pending_deadline(), Some(9_000));
    let claimed = [note(1_000), submit(2_000, 4, 7_000), claim(7_000, 5)];
    assert_eq!(idx(&claimed, 8_000).pending_deadline(), None);
    let approve_only = [note(1_000), approve(2_000, 7_000)];
    assert_eq!(idx(&approve_only, 3_000).pending_deadline(), Some(7_000));
}

// --- hand-edited and synced logs ---

#[test]
fn the_first_claim_of_a_round_is_its_claim() {
    let second =
        json!({"ts": 8_500, "by": {"type": "agent", "name": "pi"}, "op": "claim", "round": 5});
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            claim(8_000, 5),
            second,
        ],
        9_000,
    );
    assert_eq!(t.rounds[0].claim, Some(6));
}

#[test]
fn a_withdraw_after_a_claim_is_ignored() {
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            claim(8_000, 5),
            withdraw(9_000, 5),
        ],
        10_000,
    );
    assert_eq!(t.rounds[0].withdrawn, None);
    assert_eq!(t.rounds[0].claim, Some(6));
    assert_eq!(t.check_submit(4, 10_000), Err(Reject::NothingToSend));
    assert_eq!(t.check_submit(7, 10_000), Err(Reject::NothingToSend));
}

#[test]
fn a_ts_at_u64_max_does_not_panic() {
    for (ts, not_before) in [
        (u64::MAX, u64::MAX),
        (u64::MAX, 0),
        (u64::MAX - 1, u64::MAX),
    ] {
        let ops = [
            note(1_000),
            submit(ts, 4, not_before),
            approve(ts, not_before),
        ];
        let t = idx(&ops, 10_000);
        t.claimable_round(10_000);
        t.pending_deadline();
        let _ = t.check_claim(5, u64::MAX);
        let _ = t.check_submit(4, u64::MAX);
    }
}

// --- release ---

fn claim_as(ts: u64, round: u32, name: &str) -> Value {
    json!({"ts": ts, "by": {"type": "agent", "name": name, "via": "cli"}, "op": "claim", "round": round})
}

/// The human takes the round back from the claim on line `turn`.
fn release(ts: u64, turn: u32) -> Value {
    json!({"ts": ts, "by": human(), "op": "yield", "turn": turn, "release": true})
}

fn agent_yield(ts: u64, turn: u32) -> Value {
    json!({"ts": ts, "by": agent(), "op": "yield", "turn": turn})
}

#[test]
fn a_released_round_is_claimable_again() {
    // 4 note, 5 submit, 6 claim(5), 7 release(6)
    let ops = [
        note(1_000),
        submit(2_000, 4, 7_000),
        claim(8_000, 5),
        release(9_000, 6),
    ];
    let t = idx(&ops, 10_000);
    assert_eq!(t.rounds[0].claim, None);
    assert_eq!(t.check_claim(5, 10_000), Ok(()));
    assert_eq!(t.claimable_round(10_000), Some(5));
    assert_eq!(t.pending_deadline(), Some(7_000));
    // Before the release the same log refuses the claim
    assert_eq!(
        idx(&ops[..3], 10_000).check_claim(5, 10_000),
        Err(Reject::AlreadyClaimed)
    );
}

#[test]
fn a_claim_after_a_release_is_the_live_one() {
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            claim(8_000, 5),
            release(9_000, 6),
            claim_as(10_000, 5, "other"),
        ],
        11_000,
    );
    assert_eq!(t.rounds[0].claim, Some(8));
    assert_eq!(t.check_claim(5, 11_000), Err(Reject::AlreadyClaimed));
    assert_eq!(t.claimable_round(11_000), None);
}

#[test]
fn a_claim_that_lost_the_race_does_not_come_back_after_a_release() {
    // 6 wins, 7 lost the race, 8 releases 6
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            claim(8_000, 5),
            claim_as(8_500, 5, "other"),
            release(9_000, 6),
        ],
        10_000,
    );
    assert_eq!(t.rounds[0].claim, None);
    assert_eq!(t.check_claim(5, 10_000), Ok(()));
}

#[test]
fn only_a_human_release_of_the_live_claim_releases() {
    let base = [note(1_000), submit(2_000, 4, 7_000), claim(8_000, 5)];
    let cases = [
        ("a release naming another line", release(9_000, 99)),
        (
            "an agent yield with release",
            json!({"ts": 9_000, "by": agent(), "op": "yield", "turn": 6, "release": true}),
        ),
        (
            "a human yield without release",
            json!({"ts": 9_000, "by": human(), "op": "yield", "turn": 6}),
        ),
        (
            "a release with no author",
            json!({"ts": 9_000, "op": "yield", "turn": 6, "release": true}),
        ),
    ];
    for (what, op) in cases {
        let mut ops = base.to_vec();
        ops.push(op);
        let t = idx(&ops, 10_000);
        assert_eq!(t.rounds[0].claim, Some(6), "{what}");
        assert_eq!(
            t.check_claim(5, 10_000),
            Err(Reject::AlreadyClaimed),
            "{what}"
        );
    }
}

#[test]
fn a_release_after_the_agent_yielded_changes_nothing() {
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            claim(8_000, 5),
            agent_yield(9_000, 6),
            release(10_000, 6),
        ],
        11_000,
    );
    assert_eq!(t.rounds[0].claim, Some(6));
    assert_eq!(t.check_claim(5, 11_000), Err(Reject::AlreadyClaimed));
    assert_eq!(t.claimable_round(11_000), None);
}

#[test]
fn a_released_round_is_still_sent() {
    let ops = [
        note(1_000),
        submit(2_000, 4, 7_000),
        claim(8_000, 5),
        release(9_000, 6),
    ];
    let t = idx(&ops, 10_000);
    // A claim named it, so it can't be withdrawn, and its notes stay handed back
    assert_eq!(t.check_withdraw(5), Err(Reject::NotWithdrawable));
    assert_eq!(t.check_submit(7, 10_000), Err(Reject::NothingToSend));
}

#[test]
fn releasing_a_later_round_frees_the_earlier_ones_it_took() {
    // 4 note, 5 submit, 6 note, 7 submit, 8 claim(7), 9 release(8)
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 3_000),
            note(4_000),
            submit(5_000, 6, 6_000),
            claim(7_000, 7),
            release(8_000, 8),
        ],
        9_000,
    );
    assert_eq!(t.claimable_round(9_000), Some(7));
    assert_eq!(t.check_claim(5, 9_000), Ok(()));
    assert_eq!(t.check_claim(7, 9_000), Ok(()));
}

#[test]
fn a_withdraw_line_after_a_release_is_ignored() {
    // Clearing the released claim must not reopen the round to a withdraw
    let t = idx(
        &[
            note(1_000),
            submit(2_000, 4, 7_000),
            claim(8_000, 5),
            release(9_000, 6),
            withdraw(10_000, 5),
        ],
        11_000,
    );
    assert_eq!(t.rounds[0].withdrawn, None);
    assert_eq!(t.check_claim(5, 11_000), Ok(()));
    assert_eq!(t.claimable_round(11_000), Some(5));
}
