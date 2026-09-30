use std::collections::HashSet;

use birta::rev::Rev;
use birta::review::chain::{self, Cause, Chain, Mapped, Recorded, Step};
use birta::review::hunk;
use birta::review::ops::{self, Hunk, Parsed};
use serde_json::{Value, json};

const HEADER: &str = r#"{"birta":"review/1","doc":"pr.md","about":"ops after the last hand-back are drafts; don't act on them"}"#;

fn rev(text: &str) -> Rev {
    Rev::of(text.as_bytes())
}

fn h(a: u32, d: &[&str], i: &[&str]) -> Hunk {
    Hunk {
        a,
        d: d.iter().map(|s| s.to_string()).collect(),
        i: i.iter().map(|s| s.to_string()).collect(),
    }
}

fn snap(text: &str) -> Value {
    json!({"by": {"type": "birta"}, "op": "snap", "rev": rev(text), "text": text})
}

fn checkpoint(text: &str) -> Value {
    json!({"by": {"type": "birta"}, "op": "snap", "rev": rev(text), "text": text, "checkpoint": true})
}

fn edit(old: &str, new: &str) -> Value {
    patched("edit", old, new, json!(hunk::diff(old, new)))
}

fn external(old: &str, new: &str) -> Value {
    patched("external", old, new, json!(hunk::diff(old, new)))
}

/// A write recorded by rev only, as for a doc too large to diff.
fn rev_only(old: &str, new: &str) -> Value {
    patched("external", old, new, Value::Null)
}

/// An edit from `old` to `new` that carries `patch` as written.
fn patched(op: &str, old: &str, new: &str, patch: Value) -> Value {
    let mut v = json!({"by": {"type": "human", "via": "browser"}, "op": op, "rev": rev(old), "to_rev": rev(new), "patch": patch});
    if op == "edit" {
        v["act"] = json!("edit");
    } else {
        v["by"] = json!({"type": "birta"});
    }
    v
}

fn note() -> Value {
    json!({"by": {"type": "human"}, "op": "comment", "at": "doc", "body": "a note"})
}

/// Header on line 1, then `ops` from line 2.
fn parse(ops: &[Value]) -> Parsed {
    let mut text = format!("{HEADER}\n");
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

fn build(ops: &[Value]) -> Chain {
    Chain::build(&parse(ops))
}

fn text(chain: &Chain, of: &str) -> Option<String> {
    chain.text(&rev(of)).map(|t| t.to_string())
}

fn step_lines(steps: &[Step]) -> Vec<u32> {
    steps.iter().map(|s| s.n).collect()
}

const T0: &str = "# Plan\n\nWe ship on Friday.\n";
const T1: &str = "# Plan\n\nWe ship on Friday.\nThen we rest.\n";
const T2: &str = "# Plan\n\nWe ship on Monday.\nThen we rest.\n";
const T3: &str = "# Plan\n\nWe ship on Monday.\n";

// --- texts and revs ---

#[test]
fn every_logged_rev_has_its_text() {
    let chain = build(&[
        snap(T0),
        note(),
        edit(T0, T1),
        external(T1, T2),
        note(),
        edit(T2, T3),
    ]);
    for t in [T0, T1, T2, T3] {
        assert_eq!(text(&chain, t).as_deref(), Some(t));
    }
    assert_eq!(chain.head(), Some(&rev(T3)));
    assert_eq!(chain.broken_before(), None);
    assert_eq!(text(&chain, "never logged\n"), None);
}

#[test]
fn texts_returns_only_the_needed_revs_it_has() {
    let chain = build(&[snap(T0), edit(T0, T1), edit(T1, T2)]);
    let needed: HashSet<Rev> = [rev(T0), rev(T2), rev("never logged\n")].into();
    let texts = chain.texts(&needed);
    assert_eq!(texts.len(), 2);
    assert_eq!(&*texts[&rev(T0)], T0);
    assert_eq!(&*texts[&rev(T2)], T2);
}

#[test]
fn rev_at_line_is_the_rev_after_the_chain_ops_up_to_that_line() {
    let chain = build(&[snap(T0), note(), edit(T0, T1), note(), external(T1, T2)]);
    let at = |n| chain.rev_at_line(n).cloned();
    assert_eq!(at(1), None);
    assert_eq!(at(2), Some(rev(T0)));
    assert_eq!(at(3), Some(rev(T0)));
    assert_eq!(at(4), Some(rev(T1)));
    assert_eq!(at(5), Some(rev(T1)));
    assert_eq!(at(6), Some(rev(T2)));
    assert_eq!(at(100), Some(rev(T2)));
}

#[test]
fn a_rev_that_repeats_is_found_at_its_latest_occurrence() {
    let chain = build(&[snap(T0), edit(T0, T1), edit(T1, T0), edit(T0, T2)]);
    assert_eq!(text(&chain, T0).as_deref(), Some(T0));
    assert_eq!(chain.rev_at_line(4), Some(&rev(T0)));
    assert_eq!(step_lines(chain.steps(&rev(T0), &rev(T2)).unwrap()), [5]);
    assert_eq!(step_lines(chain.steps(&rev(T1), &rev(T2)).unwrap()), [4, 5]);
    assert_eq!(step_lines(chain.steps(&rev(T0), &rev(T1)).unwrap()), [3]);
}

#[test]
fn a_log_without_a_snap_has_no_head() {
    let chain = build(&[note()]);
    assert_eq!(chain.head(), None);
    assert_eq!(chain.rev_at_line(10), None);
    assert_eq!(text(&chain, T0), None);
    assert!(chain.steps(&rev(T0), &rev(T0)).is_none());
    assert_eq!(chain.record(T0), None);
}

// --- steps ---

#[test]
fn steps_run_between_two_revs_in_log_order() {
    let chain = build(&[
        snap(T0),
        edit(T0, T1),
        note(),
        external(T1, T2),
        edit(T2, T3),
    ]);
    let steps = chain.steps(&rev(T0), &rev(T3)).unwrap();
    assert_eq!(step_lines(steps), [3, 5, 6]);
    let pairs: Vec<(Rev, Rev)> = steps
        .iter()
        .map(|s| (s.from.clone(), s.to.clone()))
        .collect();
    assert_eq!(
        pairs,
        [(rev(T0), rev(T1)), (rev(T1), rev(T2)), (rev(T2), rev(T3))]
    );
    assert_eq!(steps[1].hunks, Some(hunk::diff(T1, T2)));

    assert_eq!(chain.steps(&rev(T1), &rev(T1)).map(<[Step]>::len), Some(0));
    assert!(chain.steps(&rev(T3), &rev(T0)).is_none(), "backwards");
    assert!(chain.steps(&rev("unknown\n"), &rev(T3)).is_none());
    assert!(chain.steps(&rev(T0), &rev("unknown\n")).is_none());

    // "We ship on Friday." is replaced at T2; "# Plan" never moves
    assert_eq!(chain::map_line(steps, 1), Mapped::Same(1));
    assert_eq!(chain::map_line(steps, 3), Mapped::Overlapped);
}

#[test]
fn a_step_that_does_not_start_at_the_head_is_a_gap() {
    let gap = [snap(T0), edit(T1, T2)];
    let chain = build(&gap);
    assert_eq!(chain.head(), Some(&rev(T2)));
    assert_eq!(text(&chain, T2), None);
    assert!(chain.steps(&rev(T0), &rev(T2)).is_none());
    assert!(chain.broken_before().is_some());

    let chain = build(&[snap(T0), edit(T1, T2), checkpoint(T2), edit(T2, T3)]);
    assert_eq!(text(&chain, T2).as_deref(), Some(T2));
    assert_eq!(text(&chain, T3).as_deref(), Some(T3));
    assert_eq!(chain.broken_before(), Some(4));
}

// --- breaks and checkpoints ---

const A0: &str = "a\nb\nc\n";
const A1: &str = "a\nB\nc\n";
const A2: &str = "a\nB\nc\nd\n";
const A3: &str = "a\nB\nc\nd\ne\n";

/// A patch whose old lines match A0 but whose result is not A1.
fn corrupt_edit() -> Value {
    patched("edit", A0, A1, json!([h(1, &["b\n"], &["B!\n"])]))
}

#[test]
fn a_patch_that_does_not_hash_to_its_rev_breaks_the_chain_until_a_checkpoint() {
    let chain = build(&[snap(A0), corrupt_edit(), edit(A1, A2)]);
    assert_eq!(text(&chain, A0).as_deref(), Some(A0));
    assert_eq!(text(&chain, A1), None, "only the sha1 check catches this");
    assert_eq!(text(&chain, "a\nB!\nc\n"), None);
    assert_eq!(chain.head(), Some(&rev(A2)));
    assert_eq!(text(&chain, A2), None);
    assert!(chain.broken_before().is_some());

    let chain = build(&[
        snap(A0),
        corrupt_edit(),
        edit(A1, A2),
        checkpoint(A2),
        edit(A2, A3),
    ]);
    assert_eq!(text(&chain, A0).as_deref(), Some(A0));
    assert_eq!(text(&chain, A1), None);
    assert_eq!(text(&chain, A2).as_deref(), Some(A2));
    assert_eq!(text(&chain, A3).as_deref(), Some(A3));
    assert_eq!(chain.broken_before(), Some(5));
    let steps = chain.steps(&rev(A2), &rev(A3)).unwrap();
    assert_eq!(steps[0].hunks, Some(hunk::diff(A2, A3)));
}

#[test]
fn a_patch_whose_old_lines_do_not_match_breaks_the_chain_until_a_checkpoint() {
    let mismatched = patched("edit", A0, A1, json!([h(1, &["x\n"], &["B\n"])]));
    let chain = build(&[snap(A0), mismatched.clone(), edit(A1, A2)]);
    assert_eq!(text(&chain, A0).as_deref(), Some(A0));
    assert_eq!(text(&chain, A1), None);
    assert_eq!(text(&chain, A2), None);
    assert!(chain.broken_before().is_some());

    let chain = build(&[snap(A0), mismatched, edit(A1, A2), checkpoint(A2)]);
    assert_eq!(text(&chain, A2).as_deref(), Some(A2));
    assert_eq!(chain.broken_before(), Some(5));
}

#[test]
fn a_patch_with_multi_line_entries_breaks_the_chain() {
    // Its bytes would splice to the right text, but its line counts are wrong
    let chain = build(&[
        snap(A0),
        patched("edit", A0, "a\n", json!([h(1, &["b\nc\n"], &[])])),
    ]);
    assert_eq!(text(&chain, "a\n"), None);
    assert!(chain.broken_before().is_some());
}

#[test]
fn steps_do_not_cross_a_broken_step() {
    let chain = build(&[
        snap(A0),
        corrupt_edit(),
        edit(A1, A2),
        checkpoint(A2),
        edit(A2, A3),
    ]);
    assert!(chain.steps(&rev(A0), &rev(A3)).is_none());
    assert!(chain.steps(&rev(A0), &rev(A1)).is_none());
    assert_eq!(step_lines(chain.steps(&rev(A2), &rev(A3)).unwrap()), [6]);
}

#[test]
fn text_of_a_rev_survives_a_later_broken_step_to_it() {
    let chain = build(&[
        snap(A0),
        edit(A0, A1),
        patched("edit", A1, A0, json!([h(1, &["B\n"], &["X\n"])])),
        edit(A0, A2),
    ]);
    assert_eq!(chain.rev_at_line(4), Some(&rev(A0)));
    assert_eq!(text(&chain, A0).as_deref(), Some(A0));
    // The head's text is known again by its rev, so later patches apply
    assert_eq!(text(&chain, A2).as_deref(), Some(A2));
    assert_eq!(chain.broken_before(), Some(4));
}

#[test]
fn steps_from_a_repeated_rev_to_itself_are_empty() {
    let chain = build(&[snap(A0), edit(A0, A1), edit(A1, A0)]);
    assert_eq!(chain.steps(&rev(A0), &rev(A0)).map(<[Step]>::len), Some(0));
}

#[test]
fn a_snap_whose_text_does_not_hash_to_its_rev_supplies_no_text() {
    let tampered =
        json!({"by": {"type": "birta"}, "op": "snap", "rev": rev(A0), "text": "tampered\n"});
    let chain = build(&[tampered.clone(), edit(A0, A1)]);
    assert_eq!(chain.head(), Some(&rev(A1)));
    assert_eq!(text(&chain, A0), None);
    assert_eq!(text(&chain, "tampered\n"), None);
    assert_eq!(text(&chain, A1), None);
    assert!(chain.broken_before().is_some());

    let late = json!({"by": {"type": "birta"}, "op": "snap", "rev": rev(A1), "text": A0, "checkpoint": true});
    let chain = build(&[tampered, edit(A0, A1), late]);
    assert_eq!(text(&chain, A1), None);
}

#[test]
fn a_checkpoint_every_twenty_patches_keeps_every_text() {
    let texts: Vec<String> = (0..=25).map(|i| format!("# Log\n\nentry {i}\n")).collect();
    let mut ops = vec![snap(&texts[0])];
    for i in 1..=25 {
        ops.push(edit(&texts[i - 1], &texts[i]));
        if i == 20 {
            ops.push(checkpoint(&texts[i]));
        }
    }
    let chain = build(&ops);
    for t in &texts {
        assert_eq!(text(&chain, t).as_deref(), Some(t.as_str()));
    }
    assert_eq!(chain.broken_before(), None);
    // A checkpoint of the head's own text is not a step
    let steps = chain.steps(&rev(&texts[19]), &rev(&texts[21])).unwrap();
    assert_eq!(step_lines(steps), [22, 24]);
}

// --- rev-only writes and snap-derived hunks ---

const B0: &str = "1\n2\n3\n4\n5\n6\n7\n8\n";
const B1: &str = "1\nA\nB\nC\n3\n4\n6\n7\nZ\n8\n";

#[test]
fn a_rev_only_step_has_no_text_or_hunks_until_a_snap_supplies_them() {
    let chain = build(&[snap(B0), rev_only(B0, B1)]);
    assert_eq!(chain.head(), Some(&rev(B1)));
    assert_eq!(text(&chain, B1), None);
    let steps = chain.steps(&rev(B0), &rev(B1)).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].hunks, None);
    assert_eq!(chain::map_line(steps, 1), Mapped::Overlapped);

    let derived = build(&[snap(B0), rev_only(B0, B1), checkpoint(B1)]);
    assert_eq!(text(&derived, B1).as_deref(), Some(B1));
    let stored = build(&[snap(B0), external(B0, B1)]);
    let derived_steps = derived.steps(&rev(B0), &rev(B1)).unwrap();
    // Same op line and same hunks, so change ids `3.0`..`3.2` match either way
    assert_eq!(derived_steps, stored.steps(&rev(B0), &rev(B1)).unwrap());
    assert_eq!(derived_steps[0].n, 3);
    assert_eq!(derived_steps[0].hunks.as_ref().map(Vec::len), Some(3));
}

#[test]
fn a_snap_to_a_new_rev_is_a_step_with_derived_hunks() {
    let chain = build(&[snap(B0), note(), snap(B1)]);
    assert_eq!(chain.head(), Some(&rev(B1)));
    let steps = chain.steps(&rev(B0), &rev(B1)).unwrap();
    assert_eq!(step_lines(steps), [4]);
    assert_eq!(steps[0].hunks, Some(hunk::diff(B0, B1)));
}

// --- map_line ---

fn step(hunks: Vec<Hunk>) -> Step {
    Step {
        n: 3,
        from: rev("from\n"),
        to: rev("to\n"),
        hunks: Some(hunks),
        cause: Cause::Edit,
    }
}

#[test]
fn map_line_around_a_replaced_block() {
    // Old lines 3-5 become one line
    let steps = [step(vec![h(2, &["c\n", "d\n", "e\n"], &["C\n"])])];
    let map = |l| chain::map_line(&steps, l);
    assert_eq!(map(1), Mapped::Same(1));
    assert_eq!(map(2), Mapped::Same(2));
    assert_eq!(map(3), Mapped::Overlapped);
    assert_eq!(map(4), Mapped::Overlapped);
    assert_eq!(map(5), Mapped::Overlapped);
    assert_eq!(map(6), Mapped::Shifted(4));
    assert_eq!(map(7), Mapped::Shifted(5));
}

#[test]
fn map_line_shifts_only_lines_after_an_insertion_or_deletion() {
    let insert = [step(vec![h(1, &[], &["x\n", "y\n"])])];
    assert_eq!(
        chain::map_line(&insert, 1),
        Mapped::Same(1),
        "insert after line 1"
    );
    assert_eq!(
        chain::map_line(&insert, 2),
        Mapped::Shifted(4),
        "insert before line 2"
    );

    let delete = [step(vec![h(1, &["b\n"], &[])])];
    assert_eq!(chain::map_line(&delete, 1), Mapped::Same(1));
    assert_eq!(chain::map_line(&delete, 2), Mapped::Overlapped);
    assert_eq!(chain::map_line(&delete, 3), Mapped::Shifted(2));
}

#[test]
fn map_line_reads_every_hunk_of_a_step_in_old_text_coordinates() {
    let steps = [step(hunk::diff(B0, B1))];
    let expected = [
        (1, Mapped::Same(1)),
        (2, Mapped::Overlapped),
        (3, Mapped::Shifted(5)),
        (4, Mapped::Shifted(6)),
        (5, Mapped::Overlapped),
        (6, Mapped::Shifted(7)),
        (7, Mapped::Shifted(8)),
        (8, Mapped::Shifted(10)),
    ];
    for (line, want) in expected {
        assert_eq!(chain::map_line(&steps, line), want, "line {line}");
    }
}

#[test]
fn map_line_composes_steps_in_order() {
    let first = step(hunk::diff(B0, B1));
    // Two lines on top of B1, then B1's line 5 ("3") removed
    let second = step(vec![h(0, &[], &["p\n", "q\n"])]);
    let third = step(vec![h(6, &["3\n"], &[])]);
    let steps = [first, second, third];
    assert_eq!(chain::map_line(&steps, 1), Mapped::Shifted(3));
    assert_eq!(chain::map_line(&steps, 3), Mapped::Overlapped);
    assert_eq!(chain::map_line(&steps, 6), Mapped::Shifted(8));

    // A line pushed down and back up again ends where it started
    let there = step(vec![h(0, &[], &["x\n"])]);
    let back = step(vec![h(0, &["x\n"], &[])]);
    assert_eq!(chain::map_line(&[there, back], 2), Mapped::Same(2));
}

#[test]
fn map_line_through_nothing_is_same_and_through_a_rev_only_step_is_overlapped() {
    assert_eq!(chain::map_line(&[], 7), Mapped::Same(7));
    let mut unknown = step(vec![]);
    unknown.hunks = None;
    assert_eq!(chain::map_line(&[unknown], 7), Mapped::Overlapped);
}

// --- what a write records ---

#[test]
fn record_limits_are_the_planned_sizes() {
    assert_eq!(chain::SNAP_OVER, 256 * 1024);
    assert_eq!(chain::REV_ONLY_OVER, 1024 * 1024);
    assert_eq!(chain::CHECKPOINT_EVERY, 20);
}

#[test]
fn record_has_nothing_for_the_head_text() {
    let chain = build(&[snap(T0), edit(T0, T1)]);
    assert_eq!(chain.record(T1), None);
}

#[test]
fn record_patches_a_small_write() {
    let chain = build(&[snap(T0), edit(T0, T1)]);
    assert_eq!(
        chain.record(T2),
        Some(Recorded {
            rev: rev(T1),
            to_rev: rev(T2),
            patch: Some(hunk::diff(T1, T2)),
            checkpoint: None,
        })
    );
}

/// "a\n" plus one line whose patch serializes to exactly `len` bytes of JSON.
fn with_patch_json_of(len: usize) -> String {
    let json_len = |k: usize| {
        let new = format!("a\n{}\n", "x".repeat(k));
        serde_json::to_vec(&hunk::diff("a\n", &new)).unwrap().len()
    };
    let k = len - json_len(0);
    assert_eq!(json_len(k), len);
    format!("a\n{}\n", "x".repeat(k))
}

#[test]
fn record_snaps_instead_of_a_patch_over_256_kib_of_json() {
    let chain = build(&[snap("a\n")]);
    let at_limit = with_patch_json_of(chain::SNAP_OVER);
    let recorded = chain.record(&at_limit).unwrap();
    assert_eq!(recorded.patch, Some(hunk::diff("a\n", &at_limit)));
    assert_eq!(recorded.checkpoint, None);

    let over = with_patch_json_of(chain::SNAP_OVER + 1);
    assert_eq!(
        chain.record(&over),
        Some(Recorded {
            rev: rev("a\n"),
            to_rev: rev(&over),
            patch: None,
            checkpoint: Some(over.clone()),
        })
    );
}

#[test]
fn record_is_rev_only_when_either_text_is_over_1_mib() {
    let line = "0123456789abcde\n";
    let lines = chain::REV_ONLY_OVER / line.len();
    let old = line.repeat(lines - 1);
    let at_limit = line.repeat(lines);
    assert_eq!(at_limit.len(), chain::REV_ONLY_OVER);
    let chain = build(&[snap(&old)]);
    let recorded = chain.record(&at_limit).unwrap();
    assert_eq!(recorded.patch, Some(hunk::diff(&old, &at_limit)));

    let over = format!("{at_limit}x");
    assert_eq!(
        chain.record(&over),
        Some(Recorded {
            rev: rev(&old),
            to_rev: rev(&over),
            patch: None,
            checkpoint: None,
        })
    );

    // From an oversized text back to a small one, the small text is snapped
    let chain = build(&[snap(&over)]);
    assert_eq!(
        chain.record("a\n"),
        Some(Recorded {
            rev: rev(&over),
            to_rev: rev("a\n"),
            patch: None,
            checkpoint: Some("a\n".to_string()),
        })
    );
}

#[test]
fn record_snaps_when_the_head_text_is_unknown() {
    let chain = build(&[snap(B0), rev_only(B0, B1)]);
    assert_eq!(
        chain.record(T0),
        Some(Recorded {
            rev: rev(B1),
            to_rev: rev(T0),
            patch: None,
            checkpoint: Some(T0.to_string()),
        })
    );
    let chain = build(&[snap(A0), corrupt_edit()]);
    let recorded = chain.record(A2).unwrap();
    assert_eq!(
        (recorded.patch, recorded.checkpoint),
        (None, Some(A2.to_string()))
    );
}

#[test]
fn record_adds_a_checkpoint_to_every_twentieth_patch_since_a_snap() {
    let texts: Vec<String> = (0..=45).map(|i| format!("entry {i}\n")).collect();
    // Baseline, then `patches` writes alternating edit and external, with checkpoints as recorded
    let log = |patches: usize| {
        let mut ops = vec![snap(&texts[0])];
        for i in 1..=patches {
            ops.push(if i % 2 == 0 {
                edit(&texts[i - 1], &texts[i])
            } else {
                external(&texts[i - 1], &texts[i])
            });
            if i % chain::CHECKPOINT_EVERY == 0 {
                ops.push(checkpoint(&texts[i]));
            }
        }
        build(&ops)
    };
    let checkpoint_for = |patches: usize| log(patches).record(&texts[patches + 1]).unwrap();

    assert_eq!(checkpoint_for(18).checkpoint, None);
    let twentieth = checkpoint_for(19);
    assert_eq!(twentieth.patch, Some(hunk::diff(&texts[19], &texts[20])));
    assert_eq!(twentieth.checkpoint, Some(texts[20].clone()));
    assert_eq!(checkpoint_for(20).checkpoint, None);
    assert_eq!(checkpoint_for(38).checkpoint, None);
    assert_eq!(checkpoint_for(39).checkpoint, Some(texts[40].clone()));
}

#[test]
fn checkpoint_counts_patch_ops_not_ops_or_hunks() {
    // Two hunks per write, and a note after each
    let texts: Vec<String> = (0..=21).map(|i| format!("x{i}\nmid\ny{i}\n")).collect();
    let log = |patches: usize| {
        let mut ops = vec![snap(&texts[0])];
        for i in 1..=patches {
            ops.push(edit(&texts[i - 1], &texts[i]));
            ops.push(note());
        }
        build(&ops)
    };
    assert_eq!(hunk::diff(&texts[0], &texts[1]).len(), 2);
    assert_eq!(log(18).record(&texts[19]).unwrap().checkpoint, None);
    assert_eq!(
        log(19).record(&texts[20]).unwrap().checkpoint,
        Some(texts[20].clone())
    );
}

#[test]
fn limits_count_bytes_not_chars() {
    let chain = build(&[snap("a\n")]);
    let wide = format!("{}x", "é".repeat(chain::REV_ONLY_OVER / 2));
    assert_eq!(wide.len(), chain::REV_ONLY_OVER + 1);
    let recorded = chain.record(&wide).unwrap();
    assert_eq!((recorded.patch, recorded.checkpoint), (None, None));

    // A patch whose JSON is one byte over the limit in far fewer chars
    let json_len = |new: &str| serde_json::to_vec(&hunk::diff("a\n", new)).unwrap().len();
    let base = json_len("a\n\n");
    let room = chain::SNAP_OVER + 1 - base;
    let new = format!("a\n{}{}\n", "é".repeat(room / 2), "x".repeat(room % 2));
    assert_eq!(json_len(&new), chain::SNAP_OVER + 1);
    let recorded = chain.record(&new).unwrap();
    assert_eq!((recorded.patch, recorded.checkpoint), (None, Some(new)));

    // An old text of exactly 1 MiB is not over the limit
    let line = "0123456789abcde\n";
    let old = line.repeat(chain::REV_ONLY_OVER / line.len());
    assert_eq!(old.len(), chain::REV_ONLY_OVER);
    let new = old.replacen("0123", "ABCD", 1);
    let chain = build(&[snap(&old)]);
    assert_eq!(
        chain.record(&new).unwrap().patch,
        Some(hunk::diff(&old, &new))
    );
}

#[test]
fn a_recorded_history_rebuilds_every_text() {
    let mut texts = vec!["# Notes\n".to_string()];
    let mut ops = vec![snap(&texts[0])];
    let big = format!("{}\n", "y".repeat(300 * 1024));
    for i in 1..=70 {
        let prev = texts.last().unwrap().clone();
        let next = match i {
            30 => format!("{prev}{big}"),
            31 => prev.replace(&big, ""),
            _ if i % 3 == 0 => prev.replacen("item", "ITEM", 1),
            _ => format!("{prev}- item {i}\n"),
        };
        let recorded = Chain::build(&parse(&ops)).record(&next).unwrap();
        assert_eq!(
            (&recorded.rev, &recorded.to_rev),
            (&rev(&prev), &rev(&next))
        );
        ops.push(json!({
            "by": {"type": "human", "via": "browser"},
            "op": "edit",
            "rev": recorded.rev,
            "to_rev": recorded.to_rev,
            "patch": recorded.patch,
            "act": "edit",
        }));
        if let Some(text) = &recorded.checkpoint {
            assert_eq!(text, &next);
            ops.push(checkpoint(text));
        }
        texts.push(next);
    }

    let chain = Chain::build(&parse(&ops));
    for t in &texts {
        assert_eq!(text(&chain, t).as_deref(), Some(t.as_str()));
    }
    assert_eq!(chain.broken_before(), None);
    let snaps = ops.iter().filter(|op| op["op"] == "snap").count();
    // Baseline, the two oversized writes, and checkpoints on writes 20 and 51
    assert_eq!(snaps, 5);
    let mut run = 0;
    for op in &ops {
        run = if op["op"] == "snap" { 0 } else { run + 1 };
        assert!(
            run <= chain::CHECKPOINT_EVERY,
            "{run} patches without a snap"
        );
    }
}

// --- who wrote an external: claims, releases, approves ---

const W0: &str = "one\ntwo\n";
const W1: &str = "one\ntwo, edited\n";

fn submit(upto: u32) -> Value {
    json!({"by": {"type": "human"}, "op": "submit", "upto": upto, "not_before": 0, "hold_ms": 0})
}

fn claim_by(name: &str, round: u32) -> Value {
    json!({"by": {"type": "agent", "name": name}, "op": "claim", "round": round})
}

fn release(turn: u32) -> Value {
    json!({"by": {"type": "human"}, "op": "yield", "turn": turn, "release": true})
}

/// The cause of the one write, `W0` to `W1`, logged after `ops` (which start on line 3).
fn cause_after(ops: &[Value]) -> Cause {
    let mut all = vec![snap(W0)];
    all.extend_from_slice(ops);
    all.push(external(W0, W1));
    let chain = build(&all);
    chain.steps(&rev(W0), &rev(W1)).unwrap()[0].cause
}

#[test]
fn a_claim_taken_again_after_a_release_is_an_agents_turn() {
    // 3 note, 4 submit, 5 claim, 6 release(5), 7 claim again
    let cause = cause_after(&[
        note(),
        submit(3),
        claim_by("claude", 4),
        release(5),
        claim_by("other", 4),
    ]);
    assert_eq!(cause, Cause::Agent);
}

#[test]
fn a_release_frees_only_the_claim_it_names() {
    // 3 note, 4 submit, 5 claim(4), 6 note, 7 submit, 8 claim(7), 9 release(5)
    let ops = [
        note(),
        submit(3),
        claim_by("claude", 4),
        note(),
        submit(6),
        claim_by("other", 7),
        release(5),
    ];
    assert_eq!(cause_after(&ops), Cause::Agent);
    // A release naming a line that holds no claim frees nothing
    let ops = [note(), submit(3), claim_by("claude", 4), release(9)];
    assert_eq!(cause_after(&ops), Cause::Agent);
    let ops = [note(), submit(3), claim_by("claude", 4), release(5)];
    assert_eq!(cause_after(&ops), Cause::Outside { git: false });
}

#[test]
fn a_claimed_approve_opens_no_turn() {
    let approve = json!({"by": {"type": "human"}, "op": "approve", "rev": rev(W0), "not_before": 0, "hold_ms": 0});
    let cause = cause_after(&[note(), approve, claim_by("claude", 4)]);
    assert_eq!(cause, Cause::Outside { git: false });
}

#[test]
fn two_open_claims_make_the_write_an_agents() {
    // 3 note, 4 submit, 5 claim(4), 6 note, 7 submit, 8 claim(7)
    let cause = cause_after(&[
        note(),
        submit(3),
        claim_by("claude", 4),
        note(),
        submit(6),
        claim_by("other", 7),
    ]);
    assert_eq!(cause, Cause::Agent);
}

#[test]
fn a_git_write_during_a_claim_is_from_outside() {
    // 3 note, 4 submit, 5 claim(4), 6 the checkout
    let mut git = external(W0, W1);
    git["cause"] = json!("git");
    let chain = build(&[snap(W0), note(), submit(3), claim_by("claude", 4), git]);
    let cause = chain.steps(&rev(W0), &rev(W1)).unwrap()[0].cause;
    assert_eq!(cause, Cause::Outside { git: true });
}
