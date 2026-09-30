use birta::review::{
    self,
    ops::{By, ByKind, CommentKind, ReplyKind, Scope, To, Verdict},
    state::{
        AnchorStatus, ApproveState, Asked, AskedKind, AtKind, AtState, Attr, Change, ChangeRange,
        Counts, Deletion, DeletionKind, Hold, HoldKind, Item, ItemOutcome, ItemStatus, Loc,
        LogInfo, OwnerState, Phrase, PickedUp, RangeStatus, Replied, ReplyState, Reversible,
        ReversibleReason, ReviewState, Rewrite, Round, Sent, Silence, Stage, What,
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
            removed: Some("old text".to_string()),
            is_rewrite: false,
            reversible: Reversible {
                ok: false,
                reason: ReversibleReason::MultiHunk,
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
        }],
        changes: vec![],
        deletions: vec![Deletion {
            change: "9.0".to_string(),
            kind: DeletionKind::Words,
            at_line: 1,
            at_byte: None,
            removed: "gone".to_string(),
            after_sp: None,
        }],
        rewrite: None,
        later: vec![],
        lost: vec![],
        approve: None,
        rounds: vec![],
        warnings: vec![],
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
    let want = json!({
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
        "counts": {"drafts": 1, "sent_unclaimed": 2, "unseen": 3, "unasked": 4, "not_handled": 5, "open_asks": 6, "later": 7, "lost": 8, "items": 9, "decided": 10, "attention": 11},
        "items": [{
            "n": 12, "op": "comment", "by": {"type": "human", "name": "andres"}, "cid": "c-9f", "ts": 1_790_000_000_000u64,
            "at": {"kind": "anchor", "status": "moved", "scope": "quote", "quote": "we just fail", "shown": "we just fail",
                   "lines": [5, 5], "range": [212, 224], "block": [5, 7], "section": "Summary", "rev": REV,
                   "loc": {"sp": "5:1-7:20", "off": [8, 20], "tl": 64}},
            "verdict": "no", "body": "say why", "to": 3, "kind": "aside",
            "status": "handled", "sent_in": 31, "turn": 33, "amended": [40],
            "replies": [{"n": 36, "id": "#36", "by": {"type": "agent", "name": "claude"}, "kind": "answer", "verdict": "yes",
                         "body": "flaky S3 503s", "ts": 1_790_000_300_000u64, "seen": true}],
            "outcome": {"done": true, "no_change": false, "changes": ["37.0"]}
        }],
        "changes": [{
            "id": "37.0", "op": 37, "idx": 0, "turn": 33, "attr": "agent", "by": {"type": "agent", "name": "claude"}, "cause": "git",
            "old": [9, 17], "new": [9, 12],
            "range": {"status": "ok", "lines": [9, 12], "bytes": [301, 588], "sp": "9:1-12:40"},
            "asked": {"kind": "location", "notes": [12]},
            "changes_your_edit": "20.0", "reverts_your_edit": false, "breaks_stet": [14],
            "seen": false, "settled": false, "undone_by": 45,
            "what": {"old_lines": 9, "new_lines": 4, "words": 2, "touches_html": false},
            "removed": "old text", "is_rewrite": false,
            "reversible": {"ok": false, "reason": "multi_hunk"}, "op_hunks": 2
        }],
        "deletions": [{"change": "37.1", "kind": "block", "at_line": 41, "at_byte": 1230, "removed": "## Stacked on\n", "after_sp": "39:1-40:12"}],
        "rewrite": {"turn": 33, "pct": 0.625, "blocks_changed": 5, "blocks_total": 8},
        "later": [21, 22],
        "lost": [23],
        "approve": {"n": 44, "ts": 1_790_000_900_000u64, "rev": REV_SEEN, "current_matches": false, "changed_since": true, "later": 2, "can_withdraw": true},
        "rounds": [{"submit": 31, "ts": 1_790_000_005_000u64, "by": "andres", "to": "*", "batch": 7, "items": [12, 13, 14],
                    "claim": 33, "claimed_by": "claude", "yield": 38, "released": false}],
        "warnings": ["unknown op 'foo' x2"]
    });
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
        (ReversibleReason::MultiHunk, "multi_hunk"),
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
        removed: None,
        is_rewrite: false,
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
            "id",
            "idx",
            "is_rewrite",
            "new",
            "old",
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

// --- the contract fixtures in e2e/fixtures/review ---

const T: u64 = 1_790_000_000_000;
const PR_HAND_OFF: &str = "Take a look at my notes: `birta review /work/pr.md`";

fn fixture_base(stage: Stage, log_lines: u32) -> ReviewState {
    ReviewState {
        schema: review::SCHEMA_STATE.to_string(),
        doc: "pr.md".to_string(),
        rev: REV.to_string(),
        server_now: T + 600_000,
        writable: true,
        log_writable: true,
        hand_off: PR_HAND_OFF.to_string(),
        log: LogInfo {
            lines: log_lines,
            bad: 0,
            unknown_ops: 0,
            read_only: false,
            baseline_available: true,
            baseline_from: None,
        },
        owner: OwnerState {
            drafted_by: Some("claude".to_string()),
        },
        phrase: Phrase {
            stage,
            sent: None,
            picked_up: None,
            replied: None,
        },
        phrase_before_hold: None,
        hold: None,
        counts: Counts {
            items: 5,
            ..counts(0)
        },
        items: vec![],
        changes: vec![],
        deletions: vec![],
        rewrite: None,
        later: vec![],
        lost: vec![],
        approve: None,
        rounds: vec![],
        warnings: vec![],
    }
}

fn anchor_at(
    scope: Scope,
    quote: &str,
    lines: (u32, u32),
    sp: &str,
    off: Option<(usize, usize)>,
) -> AtState {
    AtState {
        kind: AtKind::Anchor,
        status: Some(AnchorStatus::Ok),
        scope: Some(scope),
        quote: Some(quote.to_string()),
        shown: Some(quote.to_string()),
        lines: Some(lines),
        range: None,
        block: Some(lines),
        section: Some("Why".to_string()),
        rev: Some(REV.to_string()),
        loc: Some(Loc {
            sp: sp.to_string(),
            off,
            tl: off.map(|_| 58),
        }),
    }
}

fn place_at(kind: AtKind) -> AtState {
    AtState {
        kind,
        status: None,
        scope: None,
        quote: None,
        shown: None,
        lines: None,
        range: None,
        block: None,
        section: None,
        rev: None,
        loc: None,
    }
}

fn note(n: u32, at: AtState, verdict: Option<Verdict>, body: &str) -> Item {
    Item {
        n,
        id: None,
        op: "comment".to_string(),
        by: human(),
        cid: Some(format!("c-{n}")),
        ts: T + u64::from(n) * 1000,
        at,
        verdict,
        body: (!body.is_empty()).then(|| body.to_string()),
        to: None,
        kind: None,
        status: ItemStatus::Draft,
        sent_in: None,
        turn: None,
        amended: vec![],
        replies: vec![],
        outcome: None,
    }
}

/// Three drafts: a quote note, a verdict on a list item, a note on the whole doc.
fn draft_items() -> Vec<Item> {
    vec![
        note(
            4,
            AtState {
                range: Some((212, 224)),
                ..anchor_at(
                    Scope::Quote,
                    "we just fail",
                    (5, 5),
                    "5:1-5:58",
                    Some((21, 33)),
                )
            },
            Some(Verdict::No),
            "say why",
        ),
        note(
            5,
            anchor_at(Scope::Block, "- retries on 503", (9, 9), "9:1-9:17", None),
            Some(Verdict::No),
            "drop this bullet",
        ),
        note(
            6,
            place_at(AtKind::Doc),
            None,
            "is the test ladder still needed?",
        ),
    ]
}

fn drafts_phrase() -> Phrase {
    Phrase {
        stage: Stage::Drafts,
        sent: None,
        picked_up: None,
        replied: None,
    }
}

fn sent_phrase(can_withdraw: bool) -> Phrase {
    Phrase {
        stage: Stage::Sent,
        sent: Some(Sent {
            n: 7,
            ts: T + 12_000,
            items: 3,
            can_withdraw,
        }),
        picked_up: None,
        replied: None,
    }
}

fn round(claim: bool, r#yield: bool) -> Round {
    Round {
        submit: 7,
        ts: T + 12_000,
        by: "andres".to_string(),
        to: Some(To::One("*".to_string())),
        batch: None,
        items: vec![4, 5, 6],
        claim: claim.then_some(8),
        claimed_by: claim.then(|| "claude".to_string()),
        r#yield: r#yield.then_some(13),
        released: false,
    }
}

fn picked_phrase() -> Phrase {
    Phrase {
        stage: Stage::PickedUp,
        picked_up: Some(PickedUp {
            n: 8,
            ts: T + 20_000,
            by: "claude".to_string(),
            silence: Silence {
                since_ts: T + 20_000,
                threshold_ms: 900_000,
            },
        }),
        ..sent_phrase(false)
    }
}

fn agent_change(
    idx: u32,
    asked: AskedKind,
    notes: Vec<u32>,
    lines: (u32, u32),
    sp: &str,
) -> Change {
    Change {
        id: format!("9.{idx}"),
        op: 9,
        idx,
        turn: Some(8),
        attr: Attr::Agent,
        by: agent(),
        cause: None,
        old: lines,
        new: lines,
        range: ChangeRange {
            status: RangeStatus::Ok,
            lines,
            bytes: (200, 260),
            sp: Some(sp.to_string()),
        },
        asked: Asked { kind: asked, notes },
        changes_your_edit: None,
        reverts_your_edit: false,
        breaks_stet: vec![],
        seen: false,
        settled: false,
        undone_by: None,
        what: What {
            old_lines: lines.1 - lines.0 + 1,
            new_lines: lines.1 - lines.0 + 1,
            words: Some(3),
            touches_html: false,
        },
        removed: None,
        is_rewrite: false,
        reversible: Reversible {
            ok: false,
            reason: ReversibleReason::MultiHunk,
        },
        op_hunks: 1,
    }
}

/// The agent's turn 8: one fix on note #4, an answer to #6, nothing for #5.
fn replied_state() -> ReviewState {
    let mut state = fixture_base(Stage::Replied, 13);
    let mut items = draft_items();
    for item in &mut items {
        item.sent_in = Some(7);
        item.turn = Some(8);
        item.status = ItemStatus::Handled;
    }
    items[0].outcome = Some(ItemOutcome {
        done: true,
        no_change: false,
        changes: vec!["9.0".to_string()],
    });
    items[1].status = ItemStatus::NotHandled;
    items[1].id = Some("nh:5".to_string());
    items[2].replies = vec![ReplyState {
        n: 11,
        id: "#11".to_string(),
        by: agent(),
        kind: Some(ReplyKind::Answer),
        verdict: None,
        body: Some("Yes: CI runs it on every push.".to_string()),
        ts: T + 300_000,
        seen: false,
    }];
    state.items = items;
    state.changes = vec![agent_change(
        0,
        AskedKind::Location,
        vec![4],
        (5, 5),
        "5:1-5:71",
    )];
    state.phrase = Phrase {
        stage: Stage::Replied,
        replied: Some(Replied {
            turn: 8,
            ts: T + 300_000,
            by: "claude".to_string(),
            done: 1,
            answered: 1,
            didnt: 0,
            asked: 0,
            unasked: 0,
            not_handled: 1,
            note: Some("Said why the upload fails.".to_string()),
        }),
        ..picked_phrase()
    };
    state.rounds = vec![round(true, true)];
    state.counts = Counts {
        items: 5,
        decided: 1,
        unseen: 1,
        not_handled: 1,
        attention: 3,
        ..counts(0)
    };
    state
}

fn contract_fixtures() -> Vec<(&'static str, ReviewState)> {
    let fresh = fixture_base(Stage::Idle, 3);

    let mut drafts = fixture_base(Stage::Drafts, 6);
    drafts.items = draft_items();
    drafts.counts = Counts {
        items: 5,
        drafts: 3,
        decided: 1,
        ..counts(0)
    };

    let mut sent = drafts.clone();
    sent.log.lines = 7;
    sent.phrase = sent_phrase(true);
    for item in &mut sent.items {
        item.status = ItemStatus::Sent;
        item.sent_in = Some(7);
    }
    sent.rounds = vec![round(false, false)];
    sent.counts = Counts {
        items: 5,
        sent_unclaimed: 3,
        decided: 1,
        ..counts(0)
    };

    let mut hold = sent.clone();
    hold.server_now = T + 9_000;
    hold.hold = Some(Hold {
        n: 7,
        kind: HoldKind::HandBack,
        count: 3,
        not_before: T + 12_000,
    });
    hold.phrase_before_hold = Some(drafts_phrase());

    let mut picked = sent.clone();
    picked.log.lines = 8;
    picked.phrase = picked_phrase();
    for item in &mut picked.items {
        item.status = ItemStatus::PickedUp;
        item.turn = Some(8);
    }
    picked.rounds = vec![round(true, false)];
    picked.counts.sent_unclaimed = 0;

    let replied = replied_state();

    let mut unseen = replied_state();
    unseen.log.lines = 13;
    unseen.changes.push(agent_change(
        1,
        AskedKind::Unasked,
        vec![],
        (11, 12),
        "11:1-12:40",
    ));
    let mut removal = agent_change(2, AskedKind::Unasked, vec![], (14, 14), "13:1-13:30");
    removal.old = (14, 16);
    removal.what.old_lines = 3;
    removal.what.new_lines = 0;
    removal.what.words = None;
    removal.removed = Some("## Stacked on\n\nNothing yet.\n".to_string());
    unseen.changes.push(removal);
    unseen.deletions = vec![Deletion {
        change: "9.2".to_string(),
        kind: DeletionKind::Block,
        at_line: 14,
        at_byte: Some(412),
        removed: "## Stacked on\n\nNothing yet.\n".to_string(),
        after_sp: Some("13:1-13:30".to_string()),
    }];
    unseen.counts = Counts {
        unseen: 3,
        unasked: 2,
        attention: 5,
        ..unseen.counts
    };

    let mut rewrite = replied_state();
    rewrite.changes = (0..3)
        .map(|idx| {
            let mut change = agent_change(
                idx,
                AskedKind::Unasked,
                vec![],
                (3 + idx * 4, 5 + idx * 4),
                "3:1-5:20",
            );
            change.is_rewrite = true;
            change
        })
        .collect();
    rewrite.changes[0].asked = Asked {
        kind: AskedKind::Location,
        notes: vec![4],
    };
    rewrite.rewrite = Some(Rewrite {
        turn: 8,
        pct: 0.75,
        blocks_changed: 6,
        blocks_total: 8,
    });
    rewrite.counts = Counts {
        unseen: 3,
        unasked: 2,
        attention: 5,
        ..rewrite.counts
    };

    let mut approved = replied_state();
    approved.log.lines = 17;
    for change in &mut approved.changes {
        change.seen = true;
        change.settled = true;
    }
    approved.items[2].replies[0].seen = true;
    let mut later = note(
        14,
        anchor_at(
            Scope::Block,
            "- metrics dashboard",
            (10, 10),
            "10:1-10:20",
            None,
        ),
        Some(Verdict::Later),
        "next PR",
    );
    later.status = ItemStatus::Later;
    approved.items.push(later);
    approved.later = vec![14];
    approved.approve = Some(ApproveState {
        n: 16,
        ts: T + 700_000,
        rev: REV.to_string(),
        current_matches: true,
        changed_since: false,
        later: 1,
        can_withdraw: false,
    });
    approved.phrase.stage = Stage::Approved;
    approved.server_now = T + 800_000;
    approved.counts = Counts {
        items: 5,
        decided: 2,
        later: 1,
        not_handled: 1,
        attention: 1,
        ..counts(0)
    };

    let mut lost = replied_state();
    lost.items[0].at.status = Some(AnchorStatus::Orphaned);
    lost.items[0].at.range = None;
    lost.items[0].at.loc = None;
    lost.lost = vec![4];
    lost.counts.lost = 1;

    let mut dir = fixture_base(Stage::Drafts, 5);
    dir.doc = "dirmode".to_string();
    dir.hand_off = "Take a look at my notes: `birta review /work/dirmode`".to_string();
    dir.owner.drafted_by = None;
    dir.writable = false;
    dir.items = vec![
        note(
            4,
            place_at(AtKind::Dir),
            None,
            "the sub/ docs repeat the index",
        ),
        note(
            5,
            place_at(AtKind::Dir),
            Some(Verdict::Ask),
            "which of these is current?",
        ),
    ];
    dir.counts = Counts {
        drafts: 2,
        ..counts(0)
    };

    vec![
        ("fresh", fresh),
        ("drafts", drafts),
        ("sent", sent),
        ("picked", picked),
        ("replied", replied),
        ("unseen", unseen),
        ("rewrite", rewrite),
        ("hold", hold),
        ("approved", approved),
        ("lost", lost),
        ("dir", dir),
    ]
}

#[test]
fn contract_fixtures_are_consistent() {
    for (name, state) in contract_fixtures() {
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
        assert_eq!(state.hold.is_some(), name == "hold", "{name}");
        assert_eq!(state.phrase_before_hold.is_some(), name == "hold", "{name}");
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
        }
        for deletion in &state.deletions {
            assert!(change_ids.contains(deletion.change.as_str()), "{name}");
        }
        let lost: Vec<u32> = state
            .items
            .iter()
            .filter(|i| i.at.status == Some(AnchorStatus::Orphaned))
            .map(|i| i.n)
            .collect();
        assert_eq!(state.lost, lost, "{name}");
        assert_eq!(state.counts.lost as usize, lost.len(), "{name}");
    }
}

/// The JSON the page's review code is built against. Rewrite with `BIRTA_UPDATE_FIXTURES=1`.
#[test]
fn contract_fixtures_match_the_committed_json() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("e2e/fixtures/review");
    let update = std::env::var_os("BIRTA_UPDATE_FIXTURES").is_some_and(|v| v == "1");
    let fixtures = contract_fixtures();
    let mut stale = Vec::new();
    for (name, state) in &fixtures {
        let json = serde_json::to_string_pretty(state).unwrap() + "\n";
        let path = dir.join(format!("{name}.json"));
        if update {
            fs::create_dir_all(&dir).unwrap();
            fs::write(&path, &json).unwrap();
        } else if fs::read_to_string(&path).ok().as_deref() != Some(json.as_str()) {
            stale.push(*name);
        }
    }
    assert!(
        stale.is_empty(),
        "out of date: {stale:?}; rerun with BIRTA_UPDATE_FIXTURES=1"
    );
    let on_disk: BTreeSet<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    let want: BTreeSet<String> = fixtures
        .iter()
        .map(|(name, _)| format!("{name}.json"))
        .collect();
    assert_eq!(on_disk, want, "stray or missing fixture files");
}
