use birta::review::{
    self,
    ops::{By, ByKind, CommentKind, ReplyKind, Scope, To, Verdict},
    state::{
        AnchorStatus, ApproveState, Asked, AskedKind, AtKind, AtState, Attr, Change, ChangeRange,
        Check, Counts, Deletion, DeletionKind, Hold, HoldKind, Item, ItemOutcome, ItemStatus, Loc,
        LogInfo, News, NewsKind, OwnerState, Phrase, PickedUp, RangeStatus, Replied, ReplyState,
        Reversible, ReversibleReason, ReviewState, Rewrite, Round, Sent, Silence, Stage, What,
    },
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const REV: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
const REV_SEEN: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
const HAND_OFF: &str = "Take a look at my notes: `birta review /work/pr-draft.md`";

fn human() -> By {
    By {
        kind: ByKind::Human,
        name: Some("andres".to_string()),
        via: None,
    }
}

fn agent() -> By {
    By {
        kind: ByKind::Agent,
        name: Some("claude".to_string()),
        via: None,
    }
}

fn counts(all: u32) -> Counts {
    Counts {
        drafts: all,
        sent_unclaimed: all,
        unseen: all,
        unasked: all,
        not_handled: all,
        open_asks: all,
        later: all,
        lost: all,
        items: all,
        decided: all,
        attention: all,
        to_check: all,
    }
}

fn replied_phrase() -> Phrase {
    Phrase {
        stage: Stage::Replied,
        sent: Some(Sent {
            n: 31,
            ts: 1_790_000_005_000,
            items: 4,
            can_withdraw: false,
        }),
        picked_up: Some(PickedUp {
            n: 33,
            ts: 1_790_000_060_000,
            by: "claude".to_string(),
            silence: Silence {
                since_ts: 1_790_000_060_000,
                threshold_ms: 900_000,
            },
        }),
        replied: Some(Replied {
            turn: 33,
            ts: 1_790_000_300_000,
            by: "claude".to_string(),
            done: 3,
            answered: 1,
            didnt: 0,
            asked: 0,
            unasked: 1,
            not_handled: 0,
            note: Some("Cut to 9 lines.".to_string()),
        }),
    }
}

/// Every optional field set, so every frozen name is on the wire.
fn full_state() -> ReviewState {
    ReviewState {
        schema: review::SCHEMA_STATE.to_string(),
        doc: "pr-draft.md".to_string(),
        rev: REV.to_string(),
        server_now: 1_790_000_910_000,
        writable: true,
        log_writable: true,
        hand_off: HAND_OFF.to_string(),
        log: LogInfo {
            lines: 57,
            bad: 1,
            unknown_ops: 2,
            read_only: false,
            baseline_available: true,
            baseline_from: Some(2),
        },
        owner: OwnerState {
            drafted_by: Some("claude".to_string()),
        },
        phrase: replied_phrase(),
        phrase_before_hold: Some(Phrase {
            stage: Stage::Drafts,
            sent: None,
            picked_up: None,
            replied: None,
        }),
        hold: Some(Hold {
            n: 44,
            kind: HoldKind::Approve,
            count: 2,
            not_before: 1_790_000_905_000,
        }),
        counts: Counts {
            drafts: 1,
            sent_unclaimed: 2,
            unseen: 3,
            unasked: 4,
            not_handled: 5,
            open_asks: 6,
            later: 7,
            lost: 8,
            items: 9,
            decided: 10,
            attention: 11,
            to_check: 12,
        },
        items: vec![Item {
            n: 12,
            id: None,
            op: "comment".to_string(),
            by: human(),
            cid: Some("c-9f".to_string()),
            ts: 1_790_000_000_000,
            at: AtState {
                kind: AtKind::Anchor,
                status: Some(AnchorStatus::Moved),
                scope: Some(Scope::Quote),
                quote: Some("we just fail".to_string()),
                shown: Some("we just fail".to_string()),
                lines: Some((5, 5)),
                range: Some((212, 224)),
                block: Some((5, 7)),
                section: Some("Summary".to_string()),
                rev: Some(REV.to_string()),
                change: Some("37.0".to_string()),
                loc: Some(Loc {
                    sp: "5:1-7:20".to_string(),
                    off: Some((8, 20)),
                    tl: Some(64),
                }),
            },
            verdict: Some(Verdict::No),
            body: Some("say why".to_string()),
            to: Some(3),
            kind: Some(CommentKind::Aside),
            status: ItemStatus::Handled,
            sent_in: Some(31),
            turn: Some(33),
            amended: vec![40],
            replies: vec![ReplyState {
                n: 36,
                id: "#36".to_string(),
                by: agent(),
                kind: Some(ReplyKind::Answer),
                verdict: Some(Verdict::Yes),
                body: Some("flaky S3 503s".to_string()),
                ts: 1_790_000_300_000,
                seen: true,
            }],
            outcome: Some(ItemOutcome {
                done: true,
                no_change: false,
                changes: vec!["37.0".to_string()],
            }),
            seen: true,
        }],
        changes: vec![Change {
            id: "37.0".to_string(),
            op: 37,
            idx: 0,
            turn: Some(33),
            attr: Attr::Agent,
            by: agent(),
            cause: Some("git".to_string()),
            old: (9, 17),
            new: (9, 12),
            range: ChangeRange {
                status: RangeStatus::Ok,
                lines: (9, 12),
                bytes: (301, 588),
                sp: Some("9:1-12:40".to_string()),
            },
            asked: Asked {
                kind: AskedKind::Location,
                notes: vec![12],
                answers: Vec::new(),
            },
            changes_your_edit: Some("20.0".to_string()),
            reverts_your_edit: false,
            breaks_stet: vec![14],
            seen: false,
            settled: false,
            undone_by: Some(45),
            what: What {
                old_lines: 9,
                new_lines: 4,
                words: Some(2),
                touches_html: false,
            },
            label: Some("2 words changed".to_string()),
            removed: Some("old text".to_string()),
            is_rewrite: false,
            old_text: "old text\n".to_string(),
            new_text: "new text\n".to_string(),
            check: Some(Check::Reverted),
            unasked_why: Some("while I was there".to_string()),
            flagged: false,
            span: None,
            reverse: None,
            reversible: Reversible {
                ok: false,
                reason: ReversibleReason::LaterOverlap,
            },
            op_hunks: 2,
        }],
        deletions: vec![Deletion {
            change: "37.1".to_string(),
            kind: DeletionKind::Block,
            at_line: 41,
            at_byte: Some(1230),
            removed: "## Stacked on\n".to_string(),
            after_sp: Some("39:1-40:12".to_string()),
            first_line: Some("## Stacked on".to_string()),
            label: Some("section · 2 blocks".to_string()),
            head: Some("## Stacked on\n\n- #412".to_string()),
            more: Some(1),
        }],
        rewrite: Some(Rewrite {
            turn: 33,
            pct: 0.625,
            blocks_changed: 5,
            blocks_total: 8,
        }),
        later: vec![21, 22],
        lost: vec![23],
        approve: Some(ApproveState {
            n: 44,
            ts: 1_790_000_900_000,
            rev: REV_SEEN.to_string(),
            current_matches: false,
            changed_since: true,
            later: 2,
            can_withdraw: true,
        }),
        rounds: vec![Round {
            submit: 31,
            ts: 1_790_000_005_000,
            by: "andres".to_string(),
            to: Some(To::One("*".to_string())),
            batch: Some(7),
            items: vec![12, 13, 14],
            claim: Some(33),
            claimed_by: Some("claude".to_string()),
            r#yield: Some(38),
            released: false,
        }],
        warnings: vec!["unknown op 'foo' x2".to_string()],
        news: vec![
            News {
                id: "37.0".to_string(),
                kind: NewsKind::Change,
                sp: Some("9:1-12:40".to_string()),
                after_sp: None,
                check: Some(Check::Reverted),
            },
            News {
                id: "37.1".to_string(),
                kind: NewsKind::Removal,
                sp: None,
                after_sp: Some(Some("39:1-40:12".to_string())),
                check: None,
            },
        ],
    }
}

/// Only required fields: what a doc with no review file folds to.
fn fresh_state() -> ReviewState {
    ReviewState {
        schema: review::SCHEMA_STATE.to_string(),
        doc: "pr-draft.md".to_string(),
        rev: REV.to_string(),
        server_now: 1_790_000_000_000,
        writable: false,
        log_writable: false,
        hand_off: HAND_OFF.to_string(),
        log: LogInfo {
            lines: 0,
            bad: 0,
            unknown_ops: 0,
            read_only: false,
            baseline_available: false,
            baseline_from: None,
        },
        owner: OwnerState { drafted_by: None },
        phrase: Phrase {
            stage: Stage::Idle,
            sent: None,
            picked_up: None,
            replied: None,
        },
        phrase_before_hold: None,
        hold: None,
        counts: counts(0),
        items: vec![Item {
            n: 3,
            id: None,
            op: "comment".to_string(),
            by: human(),
            cid: None,
            ts: 1,
            at: AtState {
                kind: AtKind::Doc,
                status: None,
                scope: None,
                quote: None,
                shown: None,
                lines: None,
                range: None,
                block: None,
                section: None,
                rev: None,
                change: None,
                loc: None,
            },
            verdict: None,
            body: Some("overall fine".to_string()),
            to: None,
            kind: None,
            status: ItemStatus::Draft,
            sent_in: None,
            turn: None,
            amended: vec![],
            replies: vec![],
            outcome: None,
            seen: false,
        }],
        changes: vec![],
        deletions: vec![Deletion {
            change: "9.0".to_string(),
            kind: DeletionKind::Words,
            at_line: 1,
            at_byte: None,
            removed: "gone".to_string(),
            after_sp: None,
            first_line: None,
            label: None,
            head: None,
            more: None,
        }],
        rewrite: None,
        later: vec![],
        lost: vec![],
        approve: None,
        rounds: vec![],
        warnings: vec![],
        news: vec![],
    }
}

fn to<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn keys(v: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

#[test]
fn a_full_state_serializes_to_the_frozen_shape() {
    let mut want = json!({
        "schema": "review-state/1",
        "doc": "pr-draft.md",
        "rev": REV,
        "server_now": 1_790_000_910_000u64,
        "writable": true,
        "log_writable": true,
        "hand_off": HAND_OFF,
        "log": {"lines": 57, "bad": 1, "unknown_ops": 2, "read_only": false, "baseline_available": true, "baseline_from": 2},
        "owner": {"drafted_by": "claude"},
        "phrase": {
            "stage": "replied",
            "sent": {"n": 31, "ts": 1_790_000_005_000u64, "items": 4, "can_withdraw": false},
            "picked_up": {"n": 33, "ts": 1_790_000_060_000u64, "by": "claude", "silence": {"since_ts": 1_790_000_060_000u64, "threshold_ms": 900_000}},
            "replied": {"turn": 33, "ts": 1_790_000_300_000u64, "by": "claude", "done": 3, "answered": 1, "didnt": 0, "asked": 0, "unasked": 1, "not_handled": 0, "note": "Cut to 9 lines."}
        },
        "phrase_before_hold": {"stage": "drafts"},
        "hold": {"n": 44, "kind": "approve", "count": 2, "not_before": 1_790_000_905_000u64},
        "counts": {"drafts": 1, "sent_unclaimed": 2, "unseen": 3, "unasked": 4, "not_handled": 5, "open_asks": 6, "later": 7, "lost": 8, "items": 9, "decided": 10, "attention": 11, "to_check": 12},
        "items": [{
            "n": 12, "op": "comment", "by": {"type": "human", "name": "andres"}, "cid": "c-9f", "ts": 1_790_000_000_000u64,
            "at": {"kind": "anchor", "status": "moved", "scope": "quote", "quote": "we just fail", "shown": "we just fail",
                   "lines": [5, 5], "range": [212, 224], "block": [5, 7], "section": "Summary", "rev": REV,
                   "change": "37.0", "loc": {"sp": "5:1-7:20", "off": [8, 20], "tl": 64}},
            "verdict": "no", "body": "say why", "to": 3, "kind": "aside",
            "status": "handled", "sent_in": 31, "turn": 33, "amended": [40],
            "replies": [{"n": 36, "id": "#36", "by": {"type": "agent", "name": "claude"}, "kind": "answer", "verdict": "yes",
                         "body": "flaky S3 503s", "ts": 1_790_000_300_000u64, "seen": true}],
            "outcome": {"done": true, "no_change": false, "changes": ["37.0"]},
            "seen": true
        }],
        "changes": [{
            "id": "37.0", "op": 37, "idx": 0, "turn": 33, "attr": "agent", "by": {"type": "agent", "name": "claude"}, "cause": "git",
            "old": [9, 17], "new": [9, 12],
            "range": {"status": "ok", "lines": [9, 12], "bytes": [301, 588], "sp": "9:1-12:40"},
            "asked": {"kind": "location", "notes": [12]},
            "changes_your_edit": "20.0", "reverts_your_edit": false, "breaks_stet": [14],
            "seen": false, "settled": false, "undone_by": 45,
            "what": {"old_lines": 9, "new_lines": 4, "words": 2, "touches_html": false},
            "label": "2 words changed", "removed": "old text", "is_rewrite": false,
            "reversible": {"ok": false, "reason": "later_overlap"}, "op_hunks": 2
        }],
        "deletions": [{"change": "37.1", "kind": "block", "at_line": 41, "at_byte": 1230, "removed": "## Stacked on\n", "after_sp": "39:1-40:12",
                       "first_line": "## Stacked on", "label": "section · 2 blocks"}],
        "rewrite": {"turn": 33, "pct": 0.625, "blocks_changed": 5, "blocks_total": 8},
        "later": [21, 22],
        "lost": [23],
        "approve": {"n": 44, "ts": 1_790_000_900_000u64, "rev": REV_SEEN, "current_matches": false, "changed_since": true, "later": 2, "can_withdraw": true},
        "rounds": [{"submit": 31, "ts": 1_790_000_005_000u64, "by": "andres", "to": "*", "batch": 7, "items": [12, 13, 14],
                    "claim": 33, "claimed_by": "claude", "yield": 38, "released": false}],
        "warnings": ["unknown op 'foo' x2"]
    });
    // Split out of the literal above, which is at the macro's recursion limit
    let change = &mut want["changes"][0];
    change["old_text"] = json!("old text\n");
    change["new_text"] = json!("new text\n");
    change["check"] = json!("reverted");
    change["unasked_why"] = json!("while I was there");
    want["deletions"][0]["head"] = json!("## Stacked on\n\n- #412");
    want["deletions"][0]["more"] = json!(1);
    want["news"] = json!([
        {"id": "37.0", "kind": "change", "sp": "9:1-12:40", "check": "reverted"},
        {"id": "37.1", "kind": "removal", "sp": null, "after_sp": "39:1-40:12", "check": null}
    ]);
    assert_eq!(serde_json::to_value(full_state()).unwrap(), want);
}

#[test]
fn absent_optionals_are_omitted_but_frozen_nulls_stay() {
    let got = serde_json::to_value(fresh_state()).unwrap();
    // `hold`, `phrase_before_hold`, `owner.drafted_by`, `at.loc` and `after_sp` are
    // `| null` in the frozen interface; every other optional is left out.
    assert_eq!(got["hold"], Value::Null);
    assert_eq!(got["phrase_before_hold"], Value::Null);
    assert_eq!(got["owner"], json!({"drafted_by": null}));
    assert_eq!(got["items"][0]["at"], json!({"kind": "doc", "loc": null}));
    assert_eq!(
        got["deletions"][0],
        json!({"change": "9.0", "kind": "words", "at_line": 1, "removed": "gone", "after_sp": null})
    );
    assert_eq!(got["phrase"], json!({"stage": "idle"}));
    assert_eq!(
        got["log"],
        json!({"lines": 0, "bad": 0, "unknown_ops": 0, "read_only": false, "baseline_available": false})
    );
    assert_eq!(
        keys(&got),
        [
            "changes",
            "counts",
            "deletions",
            "doc",
            "hand_off",
            "hold",
            "items",
            "later",
            "log",
            "log_writable",
            "lost",
            "news",
            "owner",
            "phrase",
            "phrase_before_hold",
            "rev",
            "rounds",
            "schema",
            "server_now",
            "warnings",
            "writable",
        ],
        "`rewrite` and `approve` are omitted when absent"
    );
    assert_eq!(
        keys(&got["items"][0]),
        [
            "amended", "at", "body", "by", "n", "op", "replies", "status", "ts"
        ],
        "an item omits id, cid, verdict, to, kind, sent_in, turn and outcome when absent"
    );
    assert_eq!(got["writable"], json!(false));
    assert_eq!(got["log_writable"], json!(false));
}

#[test]
fn state_enums_use_their_snake_case_names() {
    let stages = [
        (Stage::Idle, "idle"),
        (Stage::Drafts, "drafts"),
        (Stage::Sent, "sent"),
        (Stage::PickedUp, "picked_up"),
        (Stage::Replied, "replied"),
        (Stage::Approved, "approved"),
    ];
    for (v, want) in stages {
        assert_eq!(to(&v), want);
    }
    let statuses = [
        (ItemStatus::Draft, "draft"),
        (ItemStatus::Sent, "sent"),
        (ItemStatus::PickedUp, "picked_up"),
        (ItemStatus::Handled, "handled"),
        (ItemStatus::NotHandled, "not_handled"),
        (ItemStatus::Open, "open"),
        (ItemStatus::Answered, "answered"),
        (ItemStatus::Later, "later"),
        (ItemStatus::Retracted, "retracted"),
        (ItemStatus::Withdrawn, "withdrawn"),
    ];
    for (v, want) in statuses {
        assert_eq!(to(&v), want);
    }
    for (v, want) in [
        (AnchorStatus::Ok, "ok"),
        (AnchorStatus::Moved, "moved"),
        (AnchorStatus::Fuzzy, "fuzzy"),
        (AnchorStatus::Orphaned, "orphaned"),
        (AnchorStatus::Suspended, "suspended"),
    ] {
        assert_eq!(to(&v), want);
    }
    for (v, want) in [
        (AtKind::Doc, "doc"),
        (AtKind::Dir, "dir"),
        (AtKind::Anchor, "anchor"),
    ] {
        assert_eq!(to(&v), want);
    }
    for (v, want) in [
        (HoldKind::HandBack, "hand_back"),
        (HoldKind::Approve, "approve"),
    ] {
        assert_eq!(to(&v), want);
    }
    for (v, want) in [
        (AskedKind::Location, "location"),
        (AskedKind::Cited, "cited"),
        (AskedKind::Unasked, "unasked"),
        (AskedKind::Human, "human"),
        (AskedKind::Outside, "outside"),
    ] {
        assert_eq!(to(&v), want);
    }
    for (v, want) in [
        (Attr::Agent, "agent"),
        (Attr::Human, "human"),
        (Attr::Outside, "outside"),
    ] {
        assert_eq!(to(&v), want);
    }
    for (v, want) in [
        (RangeStatus::Ok, "ok"),
        (RangeStatus::Moved, "moved"),
        (RangeStatus::Superseded, "superseded"),
    ] {
        assert_eq!(to(&v), want);
    }
    for (v, want) in [
        (ReversibleReason::LaterOverlap, "later_overlap"),
        (ReversibleReason::Snap, "snap"),
        (ReversibleReason::Outside, "outside"),
        (ReversibleReason::None, "none"),
    ] {
        assert_eq!(to(&v), want);
    }
    for (v, want) in [
        (DeletionKind::Block, "block"),
        (DeletionKind::Words, "words"),
    ] {
        assert_eq!(to(&v), want);
    }
}

#[test]
fn nested_optionals_are_omitted() {
    let change = Change {
        id: "9.0".to_string(),
        op: 9,
        idx: 0,
        turn: None,
        attr: Attr::Outside,
        by: By::default(),
        cause: None,
        old: (1, 1),
        new: (1, 2),
        range: ChangeRange {
            status: RangeStatus::Ok,
            lines: (1, 2),
            bytes: (0, 10),
            sp: None,
        },
        asked: Asked {
            kind: AskedKind::Outside,
            notes: vec![],
            answers: Vec::new(),
        },
        changes_your_edit: None,
        reverts_your_edit: false,
        breaks_stet: vec![],
        seen: false,
        settled: false,
        undone_by: None,
        what: What {
            old_lines: 1,
            new_lines: 2,
            words: None,
            touches_html: false,
        },
        label: None,
        removed: None,
        is_rewrite: false,
        old_text: String::new(),
        new_text: String::new(),
        check: None,
        unasked_why: None,
        flagged: false,
        span: None,
        reverse: None,
        reversible: Reversible {
            ok: true,
            reason: ReversibleReason::None,
        },
        op_hunks: 1,
    };
    let v = serde_json::to_value(&change).unwrap();
    assert_eq!(
        keys(&v),
        [
            "asked",
            "attr",
            "breaks_stet",
            "by",
            "check",
            "id",
            "idx",
            "is_rewrite",
            "new",
            "new_text",
            "old",
            "old_text",
            "op",
            "op_hunks",
            "range",
            "reversible",
            "reverts_your_edit",
            "seen",
            "settled",
            "what",
        ]
    );
    assert_eq!(keys(&v["range"]), ["bytes", "lines", "status"]);
    assert_eq!(keys(&v["what"]), ["new_lines", "old_lines", "touches_html"]);
    assert_eq!(v["by"], json!({"type": "unknown"}));

    let reply = ReplyState {
        n: 20,
        id: "#20".to_string(),
        by: agent(),
        kind: None,
        verdict: None,
        body: None,
        ts: 1,
        seen: false,
    };
    assert_eq!(
        keys(&serde_json::to_value(&reply).unwrap()),
        ["by", "id", "n", "seen", "ts"]
    );

    let round = Round {
        submit: 31,
        ts: 1,
        by: "andres".to_string(),
        to: None,
        batch: None,
        items: vec![],
        claim: None,
        claimed_by: None,
        r#yield: None,
        released: false,
    };
    assert_eq!(
        keys(&serde_json::to_value(&round).unwrap()),
        ["by", "items", "released", "submit", "ts"]
    );

    let loc = Loc {
        sp: "3:1-3:9".to_string(),
        off: None,
        tl: None,
    };
    assert_eq!(
        serde_json::to_value(&loc).unwrap(),
        json!({"sp": "3:1-3:9"})
    );

    let phrase = Phrase {
        stage: Stage::Replied,
        sent: None,
        picked_up: None,
        replied: Some(Replied {
            turn: 8,
            ts: 1,
            by: "claude".to_string(),
            done: 0,
            answered: 0,
            didnt: 0,
            asked: 0,
            unasked: 0,
            not_handled: 0,
            note: None,
        }),
    };
    let v = serde_json::to_value(&phrase).unwrap();
    assert_eq!(keys(&v), ["replied", "stage"]);
    assert!(v["replied"].get("note").is_none());
}

#[test]
fn a_not_handled_row_carries_its_seen_id() {
    let item = Item {
        status: ItemStatus::NotHandled,
        id: Some("nh:12".to_string()),
        ..fresh_state().items[0].clone()
    };
    let v = serde_json::to_value(&item).unwrap();
    assert_eq!(v["id"], "nh:12");
    assert_eq!(v["status"], "not_handled");
}

// --- the contract fixtures in e2e/fixtures/review, folded from real logs ---

mod support {
    pub mod scenario;
}
use support::scenario::{Scenario, T0, ts};

const CLAUDE: &str = "claude";
const PR_HAND_OFF: &str = "Take a look at my notes: `birta review /work/pr.md`";
/// Well after every line the fixtures write, so no hold is still running.
const LATER: u64 = T0 + 600_000;

/// The draft the fixtures review. Stops: headings 1, 3, 8, 14; paragraphs 5-6, 16;
/// items 10, 11, 12.
const PR: &str = "\
# Upload retries

## Why

When the upload hits a 503 we just fail and the user has to start again.
Retrying three times with backoff fixes most of these.

## What changes

- retries on 503
- backoff of 1, 2 and 4 seconds
- metrics dashboard

## Stacked on

Nothing yet.
";

/// What the server adds to a fold before sending it.
fn served(mut state: ReviewState, server_now: u64) -> ReviewState {
    state.server_now = server_now;
    state.hand_off = PR_HAND_OFF.to_string();
    state
}

/// Three drafts: a quote note, a verdict on a list item, a note on the whole doc.
fn drafted() -> (Scenario, [u32; 3]) {
    let mut s = Scenario::drafted_by(PR, CLAUDE).nameless();
    let quote = s.quote("we just fail");
    let q = s.comment(quote, Some("no"), Some("say why"));
    let item = s.block("- retries on 503");
    let v = s.comment(item, Some("no"), Some("drop this bullet"));
    let d = s.note("is the test ladder still needed?");
    (s, [q, v, d])
}

/// The drafts handed back and claimed. A claim can't land inside a hold, so this
/// hand-back is due at once.
fn picked() -> (Scenario, [u32; 3], u32, u32) {
    let (mut s, notes) = drafted();
    let sub = s.submit();
    let turn = s.claim(CLAUDE);
    (s, notes, sub, turn)
}

/// The agent's turn: a fix on the quote's paragraph, an answer, nothing for the bullet.
/// `more` writes before the yield. Returns the ids the human would mark seen: the
/// fix's change and the answer.
fn replied(more: impl FnOnce(&mut Scenario)) -> (Scenario, [u32; 3], [String; 2]) {
    let (mut s, [q, v, d], _, turn) = picked();
    let fix = s.replace_line(
        "Retrying three times with backoff fixes most of these.",
        "S3 answers 503 under load; retrying three times fixes most of these.",
    );
    more(&mut s);
    s.done(CLAUDE, q, turn);
    let answer = s.agent_reply(CLAUDE, d, "answer", "Yes: CI runs it on every push.");
    s.turn_note(CLAUDE, "Said why the upload fails.", turn);
    s.yield_turn(CLAUDE, turn);
    (s, [q, v, d], [format!("{fix}.0"), format!("#{answer}")])
}

/// Every in-flow case the other fixtures lack: a fix under its own note, a note cited from
/// another block, a note done with no change, open and answered blanks, an unasked change,
/// two notes an outside write left without a place, and a write git made.
fn flowed() -> Scenario {
    let mut s = Scenario::drafted_by(PR, CLAUDE).nameless();
    s.comment(s.quote("Nothing"), None, Some("cut this"));
    s.comment(s.quote("yet"), None, Some("and this"));
    let fixed = s.comment(s.quote("backoff of 1, 2 and 4"), Some("no"), Some("cap it"));
    s.comment(
        s.block("- retries on 503"),
        Some("no"),
        Some("drop this bullet"),
    );
    let answered = s.note("is the test ladder still needed?");
    let tidy = s.comment(s.block("- metrics dashboard"), None, Some("tidy this"));
    let cited = s.note("say why it fails");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.replace_line(
        "Retrying three times with backoff fixes most of these.",
        "S3 answers 503 under load; retrying three times fixes most of these.",
    );
    s.covers(CLAUDE, cited, "S3 answers 503 under load", turn);
    s.replace_line(
        "- backoff of 1, 2 and 4 seconds",
        "- backoff of 1, 2 and 4 seconds, capped at 8",
    );
    s.done(CLAUDE, fixed, turn);
    s.done(CLAUDE, tidy, turn);
    s.agent_reply(CLAUDE, answered, "answer", "Yes: CI runs it on every push.");
    s.replace_line("## Stacked on", "## Stacked on nothing");
    s.unasked(CLAUDE, "Stacked on nothing", turn);
    s.blank(CLAUDE, s.block("## Why"), "is 503 the only code?", turn);
    let asked = s.blank(
        CLAUDE,
        s.block("# Upload retries"),
        "keep this title?",
        turn,
    );
    s.turn_note(CLAUDE, "Capped the backoff.", turn);
    s.yield_turn(CLAUDE, turn);
    s.human_reply(asked, None, Some("yes, keep it"));
    let text = s.replaced("## What changes", "## What changed");
    s.external_git(&text);
    // Last, since a note that loses its place to a git write waits for the checkout to return
    s.replace_line("Nothing yet.", "Something else now.");
    s
}

/// Each fixture's state, and the doc text it is about (none for the folder).
fn contract_fixtures() -> Vec<(&'static str, ReviewState, Option<String>)> {
    let fresh = Scenario::drafted_by(PR, CLAUDE).nameless();

    let (drafts, _) = drafted();

    let (mut sent, _) = drafted();
    let sub = sent.submit_held();

    let (picked, ..) = picked();

    let (replied_log, ..) = replied(|_| {});

    let (unseen, ..) = replied(|s| {
        s.replace_line(
            "- backoff of 1, 2 and 4 seconds",
            "- backoff of 1, 2 and 4 seconds, capped at 8",
        );
        let text = s.text().replace("\n\nNothing yet.\n", "\n");
        s.external(&text);
    });

    let (rewrite, ..) = replied(|s| {
        let text = s
            .text()
            .replace("- retries on 503\n", "- retries on 502, 503 and 504\n")
            .replace("- backoff of 1, 2 and 4 seconds\n", "- jittered backoff\n")
            .replace("- metrics dashboard\n", "- a retry counter\n")
            .replace("Nothing yet.\n", "The S3 client bump.\n");
        s.external(&text);
        // A rewrite touches eight blocks at least; a second write keeps the first's ids,
        // and leaves `## Why` as a block the turn didn't touch
        let text = s
            .text()
            .replace("## What changes\n", "## What this changes\n")
            .replace("## Stacked on\n", "## Stacked on top of\n");
        s.external(&format!("{text}\nShips behind a flag.\n"));
    });

    let (mut approved, _, seen) = replied(|_| {});
    approved.seen(&[seen[0].as_str(), seen[1].as_str()]);
    approved.verdict("- metrics dashboard", "later");
    approved.approve_held();

    let (mut lost, ..) = replied(|_| {});
    lost.replace_line(
        "When the upload hits a 503 we just fail and the user has to start again.",
        "When the upload hits a 503 the user has to start again.",
    );

    let mut dir = Scenario::folder("dirmode").nameless();
    dir.comment(json!("dir"), None, Some("the sub/ docs repeat the index"));
    dir.comment(
        json!("dir"),
        Some("ask"),
        Some("which of these is current?"),
    );
    let mut dir_state = served(birta::review::fold::fold_dir(&dir.parsed()), LATER);
    dir_state.hand_off = "Take a look at my notes: `birta review /work/dirmode`".to_string();
    dir_state.writable = false;

    let flow = flowed();

    let hold = served(sent.fold(), ts(sub) + 2_000);
    let doc = |s: &Scenario| Some(s.text().to_string());
    vec![
        ("fresh", served(fresh.fold(), LATER), doc(&fresh)),
        ("drafts", served(drafts.fold(), LATER), doc(&drafts)),
        ("sent", served(sent.fold(), LATER), doc(&sent)),
        ("picked", served(picked.fold(), LATER), doc(&picked)),
        (
            "replied",
            served(replied_log.fold(), LATER),
            doc(&replied_log),
        ),
        ("unseen", served(unseen.fold(), LATER), doc(&unseen)),
        ("rewrite", served(rewrite.fold(), LATER), doc(&rewrite)),
        ("hold", hold, doc(&sent)),
        ("approved", served(approved.fold(), LATER), doc(&approved)),
        ("lost", served(lost.fold(), LATER), doc(&lost)),
        ("flow", served(flow.fold(), LATER), doc(&flow)),
        ("dir", dir_state, None),
    ]
}

#[test]
fn contract_fixtures_are_consistent() {
    for (name, state, text) in contract_fixtures() {
        if let Some(text) = &text {
            assert_eq!(
                state.rev,
                birta::rev::Rev::of(text.as_bytes()).as_str(),
                "{name}"
            );
        }
        let v = serde_json::to_value(&state).unwrap();
        let want_stage = match name {
            "fresh" => "idle",
            "drafts" | "dir" => "drafts",
            "sent" | "hold" => "sent",
            "picked" => "picked_up",
            "approved" => "approved",
            _ => "replied",
        };
        assert_eq!(v["phrase"]["stage"], want_stage, "{name}");
        // The fold is clock-free: the page picks the phrase by `server_now`
        let held = matches!(name, "sent" | "hold" | "approved");
        assert_eq!(state.hold.is_some(), held, "{name}");
        assert_eq!(state.phrase_before_hold.is_some(), held, "{name}");
        if let Some(hold) = &state.hold {
            assert_eq!(state.server_now < hold.not_before, name == "hold", "{name}");
        }
        assert!(state.counts.decided <= state.counts.items, "{name}");
        let change_ids: BTreeSet<&str> = state.changes.iter().map(|c| c.id.as_str()).collect();
        for item in &state.items {
            let not_handled = item.status == ItemStatus::NotHandled;
            assert_eq!(
                item.id.clone(),
                not_handled.then(|| format!("nh:{}", item.n)),
                "{name}"
            );
            for id in item.outcome.iter().flat_map(|o| &o.changes) {
                assert!(change_ids.contains(id.as_str()), "{name}: {id}");
            }
            assert_eq!(item.at.kind == AtKind::Dir, name == "dir", "{name}");
            let placed =
                item.at.kind == AtKind::Anchor && item.at.status != Some(AnchorStatus::Orphaned);
            assert_eq!(item.at.loc.is_some(), placed, "{name}: item {}", item.n);
        }
        for deletion in &state.deletions {
            assert!(change_ids.contains(deletion.change.as_str()), "{name}");
        }
        // Lost notes are orphaned; only a note its own turn handled away may be left out
        let orphaned = |i: &&Item| i.at.status == Some(AnchorStatus::Orphaned);
        for item in state.items.iter().filter(orphaned) {
            let listed = state.lost.contains(&item.n);
            assert!(listed || item.status == ItemStatus::Handled, "{name}");
        }
        for n in &state.lost {
            assert!(
                state.items.iter().filter(orphaned).any(|i| i.n == *n),
                "{name}"
            );
        }
        assert_eq!(state.counts.lost as usize, state.lost.len(), "{name}");
        assert_eq!(state.log.bad + state.log.unknown_ops, 0, "{name}");
    }
}

/// Each fixture shows what its name says, so the page's specs have something to find.
#[test]
fn each_contract_fixture_shows_its_case() {
    let fixtures = contract_fixtures();
    let get = |name: &str| &fixtures.iter().find(|(n, ..)| *n == name).unwrap().1;
    assert_eq!(get("fresh").counts.items, 3);
    assert_eq!(get("drafts").counts.drafts, 3);
    assert_eq!(get("sent").counts.sent_unclaimed, 3);
    assert_eq!(get("picked").phrase.picked_up.as_ref().unwrap().by, CLAUDE);

    let replied = get("replied");
    assert_eq!(replied.changes.len(), 1);
    assert_eq!(replied.changes[0].asked.kind, AskedKind::Location);
    assert_eq!(replied.counts.not_handled, 1);
    assert_eq!(replied.counts.unseen, 1);

    let unseen = get("unseen");
    assert_eq!(unseen.changes.len(), 3);
    assert_eq!(unseen.counts.unasked, 2);
    assert_eq!(unseen.counts.unseen, 3);
    // The fix dropped "with backoff"; the last paragraph went whole
    let kinds: Vec<DeletionKind> = unseen.deletions.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, [DeletionKind::Words, DeletionKind::Block]);
    assert!(unseen.rewrite.is_none(), "{:?}", unseen.rewrite);

    // A turn that touched most blocks has no rewrite row: each change is its own news
    assert!(get("rewrite").rewrite.is_none());
    assert!(get("rewrite").changes.iter().all(|c| !c.is_rewrite));

    let approved = get("approved");
    let approve = approved.approve.as_ref().unwrap();
    assert!(approve.current_matches && !approve.changed_since);
    assert_eq!(approved.later.len(), 1);
    assert_eq!(approved.changes.len(), 1);
    assert!(approved.changes.iter().all(|c| c.seen && c.settled));
    assert!(
        approved
            .items
            .iter()
            .flat_map(|i| &i.replies)
            .all(|r| r.seen)
    );
    assert_eq!(approved.hold.as_ref().unwrap().kind, HoldKind::Approve);

    assert_eq!(get("lost").lost.len(), 1);
    assert_eq!(get("dir").items.len(), 2);

    assert!(get("sent").phrase.sent.as_ref().unwrap().can_withdraw);
    assert_eq!(get("hold").hold.as_ref().unwrap().kind, HoldKind::HandBack);
    let rewrite = serde_json::to_value(get("rewrite")).unwrap();
    let news: BTreeSet<&str> = rewrite["news"]
        .as_array()
        .unwrap_or_else(|| panic!("no news list"))
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    for c in &get("rewrite").changes {
        if c.range.status != RangeStatus::Superseded {
            assert!(
                news.contains(c.id.as_str()),
                "{} is not news: {news:?}",
                c.id
            );
        }
    }
    let lost = get("lost");
    let q = lost
        .items
        .iter()
        .find(|i| i.at.quote.as_deref() == Some("we just fail"))
        .unwrap();
    assert_eq!(q.at.status, Some(AnchorStatus::Orphaned));
    assert_eq!(lost.lost, [q.n]);
    assert!(get("replied").changes[0].reversible.ok);
    assert!(get("unseen").deletions[0].after_sp.is_some());

    let flow = get("flow");
    assert_eq!(flow.lost.len(), 2);
    let asked = |kind| flow.changes.iter().filter(|c| c.asked.kind == kind).count();
    assert_eq!(
        [AskedKind::Cited, AskedKind::Location, AskedKind::Unasked].map(asked),
        [1, 1, 1]
    );
    let causes: Vec<Option<&str>> = flow
        .changes
        .iter()
        .filter(|c| c.attr == Attr::Outside)
        .map(|c| c.cause.as_deref())
        .collect();
    assert_eq!(causes, [Some("git"), None]);
    assert!(
        flow.items
            .iter()
            .any(|i| i.outcome.as_ref().is_some_and(|o| o.done && o.no_change))
    );
    let blanks: Vec<ItemStatus> = flow
        .items
        .iter()
        .filter(|i| i.by.kind == ByKind::Agent && i.verdict == Some(Verdict::Ask))
        .map(|i| i.status)
        .collect();
    assert_eq!(blanks, [ItemStatus::Open, ItemStatus::Answered]);
    // Replies from the page carry no name
    let from_page = flow
        .items
        .iter()
        .flat_map(|i| &i.replies)
        .find(|r| r.by.kind == ByKind::Human)
        .unwrap();
    assert_eq!(from_page.by.name, None);
}

/// The JSON the page's review code is built against, and the doc each is about.
/// Rewrite with `BIRTA_UPDATE_FIXTURES=1`.
#[test]
fn contract_fixtures_match_the_committed_json() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("e2e/fixtures/review");
    let update = std::env::var_os("BIRTA_UPDATE_FIXTURES").is_some_and(|v| v == "1");
    let fixtures = contract_fixtures();
    let mut stale = Vec::new();
    for (name, state, text) in &fixtures {
        let json = serde_json::to_string_pretty(state).unwrap() + "\n";
        let files = [
            (format!("{name}.json"), Some(json)),
            (format!("{name}.md"), text.clone()),
        ];
        for (file, want) in files {
            let Some(want) = want else { continue };
            let path = dir.join(&file);
            if update {
                fs::create_dir_all(&dir).unwrap();
                fs::write(&path, &want).unwrap();
            } else if fs::read_to_string(&path).ok().as_deref() != Some(want.as_str()) {
                stale.push(file);
            }
        }
    }
    assert!(
        stale.is_empty(),
        "out of date: {stale:?}; rerun with BIRTA_UPDATE_FIXTURES=1"
    );
    let on_disk: BTreeSet<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json") || name.ends_with(".md"))
        .collect();
    let want: BTreeSet<String> = fixtures
        .iter()
        .flat_map(|(name, _, text)| {
            let doc = text.as_ref().map(|_| format!("{name}.md"));
            std::iter::once(format!("{name}.json")).chain(doc)
        })
        .collect();
    assert_eq!(on_disk, want, "stray or missing fixture files");
}
