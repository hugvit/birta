use std::ops::Range;

use birta::rev::Rev;
use birta::review::chain::Chain;
use birta::review::doc::Doc;
use birta::review::hunk;
use birta::review::ladder::{Located, relocate};
use birta::review::ops::{self, Anchor, Scope};
use birta::review::state::AnchorStatus;
use serde_json::{Value, json};

const HEADER: &str = r#"{"birta":"review/1","doc":"pr.md","about":"ops after the last hand-back are drafts; don't act on them"}"#;

const BASE: &str = "Alpha.\n\nRetry the upload twice.\n\nOmega.\n";

fn rev(text: &str) -> Rev {
    Rev::of(text.as_bytes())
}

fn snap(text: &str) -> Value {
    json!({"by": {"type": "birta"}, "op": "snap", "rev": rev(text), "text": text})
}

fn edit(old: &str, new: &str) -> Value {
    json!({"by": {"type": "human", "via": "browser"}, "op": "edit", "act": "edit",
           "rev": rev(old), "to_rev": rev(new), "patch": hunk::diff(old, new)})
}

fn git(old: &str, new: &str) -> Value {
    json!({"by": {"type": "birta"}, "op": "external", "cause": "git",
           "rev": rev(old), "to_rev": rev(new), "patch": hunk::diff(old, new)})
}

/// The log of a doc that went through `texts` in order, each write recorded as an edit.
fn history(texts: &[&str]) -> Chain {
    let mut ops = vec![snap(texts[0])];
    ops.extend(texts.windows(2).map(|w| edit(w[0], w[1])));
    build(&ops)
}

fn build(ops: &[Value]) -> Chain {
    let mut text = format!("{HEADER}\n");
    for op in ops {
        text.push_str(&op.to_string());
        text.push('\n');
    }
    let parsed = ops::parse(&text);
    assert!(
        parsed.bad.is_empty(),
        "test log must parse: {:?}",
        parsed.bad
    );
    Chain::build(&parsed)
}

fn quote(made_on: &str, quote: &str, ctx: (&str, &str), lines: (u32, u32)) -> Anchor {
    Anchor {
        quote: quote.to_string(),
        pre: ctx.0.to_string(),
        suf: ctx.1.to_string(),
        lines,
        rev: rev(made_on),
        scope: Scope::Quote,
        shown: None,
    }
}

fn block(made_on: &str, first_60: &str, lines: (u32, u32), scope: Scope) -> Anchor {
    Anchor {
        quote: first_60.to_string(),
        pre: String::new(),
        suf: String::new(),
        lines,
        rev: rev(made_on),
        scope,
        shown: None,
    }
}

fn at(
    status: AnchorStatus,
    lines: (u32, u32),
    bytes: Range<usize>,
    block: (u32, u32),
    section: Option<&str>,
) -> Located {
    Located {
        status,
        lines: Some(lines),
        bytes: Some(bytes),
        block: Some(block),
        section: section.map(str::to_string),
    }
}

/// Relocates `anchor` on the last of `texts`, after the log recorded every step to it.
fn relocate_on(anchor: &Anchor, texts: &[&str]) -> Located {
    let now = texts.last().unwrap();
    relocate(anchor, &Doc::new(now), &history(texts))
}

fn text_at<'a>(md: &'a str, located: &Located) -> &'a str {
    &md[located.bytes.clone().expect("a found anchor has bytes")]
}

/// `the upload` on line 3 of [`BASE`].
fn the_upload() -> Anchor {
    quote(
        BASE,
        "the upload",
        ("Alpha. Retry ", " twice. Omega. "),
        (3, 3),
    )
}

// L0: the anchor's own rev

#[test]
fn an_anchor_on_its_own_rev_is_ok_where_it_was_made() {
    assert_eq!(
        relocate_on(&the_upload(), &[BASE]),
        at(AnchorStatus::Ok, (3, 3), 14..24, (3, 3), None)
    );
}

#[test]
fn a_twin_on_the_same_line_does_not_take_the_anchor() {
    let md = "Retry the upload, then retry the upload again.\n";
    let second = quote(
        md,
        "the upload",
        ("Retry the upload, then retry ", " again. "),
        (1, 1),
    );
    let got = relocate_on(&second, &[md]);
    assert_eq!(got, at(AnchorStatus::Ok, (1, 1), 29..39, (1, 1), None));
}

#[test]
fn line_endings_and_a_bom_keep_bytes_in_the_docs_own_coordinates() {
    let crlf = "Alpha.\r\n\r\nRetry the upload twice.\r\n";
    let anchor = quote(crlf, "the upload", ("Alpha. Retry ", " twice. "), (3, 3));
    assert_eq!(
        relocate_on(&anchor, &[crlf]),
        at(AnchorStatus::Ok, (3, 3), 16..26, (3, 3), None)
    );
    let moved = format!("Intro.\r\n\r\n{crlf}");
    assert_eq!(
        relocate_on(&anchor, &[crlf, &moved]),
        at(AnchorStatus::Moved, (5, 5), 26..36, (5, 5), None)
    );

    let bom = "\u{feff}Alpha.\n\nRetry the upload twice.\n";
    let anchor = quote(bom, "the upload", ("Alpha. Retry ", " twice. "), (3, 3));
    assert_eq!(
        relocate_on(&anchor, &[bom]),
        at(AnchorStatus::Ok, (3, 3), 17..27, (3, 3), None)
    );
    let moved = "\u{feff}Intro.\n\nAlpha.\n\nRetry the upload twice.\n";
    assert_eq!(
        relocate_on(&anchor, &[bom, moved]),
        at(AnchorStatus::Moved, (5, 5), 25..35, (5, 5), None)
    );
}

#[test]
fn block_is_the_innermost_stop_and_section_the_innermost_heading() {
    let md = "# Top\n\n## Mid\n\n- outer item\n  - inner words here\n";
    let anchor = quote(
        md,
        "inner words",
        ("# Top ## Mid - outer item - ", " here "),
        (6, 6),
    );
    let got = relocate_on(&anchor, &[md]);
    assert_eq!(
        got,
        at(AnchorStatus::Ok, (6, 6), 32..43, (6, 6), Some("Mid"))
    );
}

// L1: mapped through the chain

#[test]
fn an_edit_below_leaves_the_anchor_ok() {
    let below = "Alpha.\n\nRetry the upload twice.\n\nOmega changed.\n";
    assert_eq!(
        relocate_on(&the_upload(), &[BASE, below]),
        at(AnchorStatus::Ok, (3, 3), 14..24, (3, 3), None)
    );
}

#[test]
fn an_insert_above_moves_the_anchor_by_the_chain() {
    let above = format!("Intro.\n\n{BASE}");
    let got = relocate_on(&the_upload(), &[BASE, &above]);
    assert_eq!(got, at(AnchorStatus::Moved, (5, 5), 22..32, (5, 5), None));
    assert_eq!(text_at(&above, &got), "the upload");
}

#[test]
fn several_steps_compose() {
    let one = format!("Intro.\n\n{BASE}");
    let two = format!("{one}\nCoda.\n");
    let three = format!("Title.\n\n{two}");
    let got = relocate_on(&the_upload(), &[BASE, &one, &two, &three]);
    assert_eq!(got, at(AnchorStatus::Moved, (7, 7), 30..40, (7, 7), None));
}

#[test]
fn the_chain_tells_twins_with_the_same_context_apart() {
    // Both twins have the same 32 chars of context on each side, so only the chain
    // knows which one the anchor was on; the wrong one lands on the old line number
    let p = "Filler paragraph that is long enough to fill all context.";
    let q = "Retry the upload.";
    let md0 = format!("{p}\n\n{q}\n\n{p}\n\n{q}\n\n{p}\n");
    let md1 = format!("New intro.\n\nSome new words.\n\n{md0}");
    let ctx = (
        "ong enough to fill all context. ",
        " Filler paragraph that is long e",
    );
    let second = quote(&md0, q, ctx, (7, 7));
    let got = relocate_on(&second, &[&md0, &md1]);
    assert_eq!(
        got,
        at(AnchorStatus::Moved, (11, 11), 166..183, (11, 11), None)
    );
}

#[test]
fn a_multi_line_quote_with_its_middle_line_replaced_is_not_found_whole() {
    let md0 = "alpha line\nbeta line\ngamma line\n";
    let md1 = "alpha line\nBETA CHANGED\ngamma line\n";
    let anchor = quote(md0, "alpha line\nbeta line\ngamma line", ("", " "), (1, 3));
    let got = relocate_on(&anchor, &[md0, md1]);
    assert!(
        !matches!(got.status, AnchorStatus::Ok | AnchorStatus::Moved),
        "{got:?}"
    );

    // The same quote, untouched but pushed down, is still found whole
    let above = format!("Intro.\n\n{md0}");
    assert_eq!(
        relocate_on(&anchor, &[md0, &above]),
        at(AnchorStatus::Moved, (3, 5), 8..39, (3, 5), None)
    );
}

// L2: searched on the current text

#[test]
fn a_reflowed_quote_is_found_by_its_collapsed_text() {
    let md0 = "The upload retries three times\nbefore it gives up.\n";
    let md1 = "The upload\nretries three\ntimes before it gives up.\n";
    let anchor = quote(
        md0,
        "retries three times\nbefore",
        ("The upload ", " it gives up. "),
        (1, 2),
    );
    let got = relocate_on(&anchor, &[md0, md1]);
    assert_eq!(got, at(AnchorStatus::Moved, (2, 3), 11..37, (1, 3), None));
    assert_eq!(text_at(md1, &got), "retries three\ntimes before");
}

#[test]
fn a_paragraph_moved_to_another_section_takes_its_new_section() {
    let md0 = "## One\n\nA para about retries and backoff.\n\n## Two\n\nOther text.\n";
    let md1 = "## One\n\n## Two\n\nOther text.\n\nA para about retries and backoff.\n";
    let ctx = ("## One A para about ", ". ## Two Other text. ");
    let anchor = quote(md0, "retries and backoff", ctx, (3, 3));
    assert_eq!(
        relocate_on(&anchor, &[md0, md1]),
        at(AnchorStatus::Moved, (7, 7), 42..61, (7, 7), Some("Two"))
    );
}

#[test]
fn a_single_hit_needs_context_or_its_old_section_to_count_as_moved() {
    let md0 = "## One\n\nWe retry now.\n\n## Two\n\nNothing.\n";
    let anchor = quote(
        md0,
        "retry",
        ("## One We ", " now. ## Two Nothing. "),
        (3, 3),
    );

    let elsewhere = "## One\n\nGone.\n\n## Two\n\nThey retry later.\n";
    assert_eq!(
        relocate_on(&anchor, &[md0, elsewhere]),
        at(AnchorStatus::Fuzzy, (7, 7), 28..33, (7, 7), Some("Two"))
    );

    let same_section = "## One\n\nThey retry later.\n\n## Two\n\nNothing.\n";
    assert_eq!(
        relocate_on(&anchor, &[md0, same_section]),
        at(AnchorStatus::Moved, (3, 3), 13..18, (3, 3), Some("One"))
    );
}

#[test]
fn a_quote_is_searched_as_source_not_as_rendered_text() {
    // The decoy renders the same (`–`, `…`) but its source differs
    let md0 =
        "The retry uses backoff -- because it must.\n\nThe retry uses backoff – because it must.\n";
    let md1 =
        "A retry uses backoff -- because it must.\n\nThe retry uses backoff – because it must.\n";
    let ctx = ("The retry uses ", " it must. The retry uses backoff");
    let anchor = quote(md0, "backoff -- because", ctx, (1, 1));
    assert_eq!(
        relocate_on(&anchor, &[md0, md1]),
        at(AnchorStatus::Moved, (1, 1), 13..31, (1, 1), None)
    );

    let md0 = "We wait... then retry.\n\nWe wait… then retry.\n";
    let md1 = "So we wait... then retry.\n\nWe wait… then retry.\n";
    let anchor = quote(
        md0,
        "wait... then",
        ("We ", " retry. We wait… then retry. "),
        (1, 1),
    );
    assert_eq!(
        relocate_on(&anchor, &[md0, md1]),
        at(AnchorStatus::Moved, (1, 1), 6..18, (1, 1), None)
    );
}

#[test]
fn equal_hits_take_the_nearest_to_the_changed_line_as_fuzzy() {
    let md0 = format!(
        "x retry now y\n\nkeep retry now please\n\n{}x retry now y\n",
        "f\n\n".repeat(10)
    );
    let md1 = md0.replace("keep retry now please", "gone");
    let ctx = ("x retry now y keep ", " please f f f f f f f f f f x re");
    let anchor = quote(&md0, "retry now", ctx, (3, 3));
    assert_eq!(
        relocate_on(&anchor, &[&md0, &md1]),
        at(AnchorStatus::Fuzzy, (1, 1), 2..11, (1, 1), None)
    );
}

#[test]
fn an_anchor_on_a_rev_the_log_never_saw_is_found_by_search() {
    let now = format!("Intro.\n\n{BASE}");
    let anchor = Anchor {
        rev: rev("a text the log never recorded\n"),
        ..the_upload()
    };
    let got = relocate(&anchor, &Doc::new(&now), &history(&[&now]));
    assert_eq!(got, at(AnchorStatus::Moved, (5, 5), 22..32, (5, 5), None));
}

// L3 and L4: fuzzy, orphaned

const SENTENCE: &str = "Intro.\n\nthe upload retries three times before giving up\n\nOutro.\n";

fn sentence() -> Anchor {
    let ctx = ("Intro. ", " Outro. ");
    quote(
        SENTENCE,
        "the upload retries three times before giving up",
        ctx,
        (3, 3),
    )
}

#[test]
fn a_reworded_quote_is_fuzzy_on_its_best_line() {
    // similar's char ratio against the original quote is 0.796
    let md1 = "Intro.\n\nthe upload tries three times and then gives up\n\nOutro.\n";
    assert_eq!(
        relocate_on(&sentence(), &[SENTENCE, md1]),
        at(AnchorStatus::Fuzzy, (3, 3), 8..54, (3, 3), None)
    );
}

#[test]
fn a_quote_reworded_past_three_quarters_is_orphaned() {
    // similar's char ratio against the original quote is 0.716
    let md1 = "Intro.\n\nthe upload retries a few times until it gives up\n\nOutro.\n";
    let got = relocate_on(&sentence(), &[SENTENCE, md1]);
    assert_eq!(got.status, AnchorStatus::Orphaned, "{got:?}");

    // Just over the threshold, at 0.774, it is still found
    let md1 = "Intro.\n\nthe upload tries three times until it gives up\n\nOutro.\n";
    assert_eq!(
        relocate_on(&sentence(), &[SENTENCE, md1]),
        at(AnchorStatus::Fuzzy, (3, 3), 8..54, (3, 3), None)
    );
}

#[test]
fn a_deleted_quote_is_orphaned_with_nowhere_to_point() {
    let got = relocate_on(&the_upload(), &[BASE, "Alpha.\n\nOmega.\n"]);
    assert_eq!(
        got,
        Located {
            status: AnchorStatus::Orphaned,
            lines: None,
            bytes: None,
            block: None,
            section: None,
        }
    );
}

// Suspended: removed by a write from outside that may be undone

const REWRITE: &str = "Completely new text.\n";

/// Ten lines, with `the upload` on line 3.
const TEN: &str = "Alpha.\n\nRetry the upload twice.\n\nOmega.\n\nSix.\n\nNine.\nTen.\n";

fn nowhere(status: AnchorStatus) -> Located {
    Located {
        status,
        lines: None,
        bytes: None,
        block: None,
        section: None,
    }
}

fn external(old: &str, new: &str) -> Value {
    json!({"by": {"type": "birta"}, "op": "external",
           "rev": rev(old), "to_rev": rev(new), "patch": hunk::diff(old, new)})
}

/// A human note, its hand-back and an agent's claim, on log lines 3 to 5.
fn open_turn(text: &str) -> Vec<Value> {
    vec![
        snap(text),
        json!({"by": {"type": "human"}, "op": "comment", "at": "doc", "body": "fix it"}),
        json!({"by": {"type": "human"}, "op": "submit", "upto": 3, "not_before": 0, "hold_ms": 0}),
        json!({"by": {"type": "agent", "name": "claude"}, "op": "claim", "round": 4}),
    ]
}

fn agent_yield() -> Value {
    json!({"by": {"type": "agent", "name": "claude"}, "op": "yield"})
}

#[test]
fn an_outside_rewrite_suspends_the_anchor_until_its_text_comes_back() {
    for write in [git(BASE, REWRITE), external(BASE, REWRITE)] {
        let chain = build(&[snap(BASE), write.clone()]);
        assert_eq!(
            relocate(&the_upload(), &Doc::new(REWRITE), &chain),
            nowhere(AnchorStatus::Suspended),
            "{write}"
        );
    }

    let chain = build(&[snap(BASE), git(BASE, REWRITE), git(REWRITE, BASE)]);
    let got = relocate(&the_upload(), &Doc::new(BASE), &chain);
    assert_eq!(got, at(AnchorStatus::Ok, (3, 3), 14..24, (3, 3), None));
}

#[test]
fn a_rewrite_by_birta_or_by_the_agent_holding_the_turn_orphans() {
    let chain = build(&[snap(BASE), edit(BASE, REWRITE)]);
    assert_eq!(
        relocate(&the_upload(), &Doc::new(REWRITE), &chain),
        nowhere(AnchorStatus::Orphaned)
    );

    // Inside the claim, the write is the agent's
    let mut ops = open_turn(BASE);
    ops.push(external(BASE, REWRITE));
    assert_eq!(
        relocate(&the_upload(), &Doc::new(REWRITE), &build(&ops)),
        nowhere(AnchorStatus::Orphaned)
    );

    // After the agent yields, the same write is from outside
    let mut ops = open_turn(BASE);
    ops.extend([agent_yield(), external(BASE, REWRITE)]);
    assert_eq!(
        relocate(&the_upload(), &Doc::new(REWRITE), &build(&ops)),
        nowhere(AnchorStatus::Suspended)
    );
}

#[test]
fn a_small_outside_change_suspends_only_when_git_made_it() {
    let anchor = Anchor {
        suf: " twice. Omega. Six. Nine. Ten. ".to_string(),
        rev: rev(TEN),
        ..the_upload()
    };
    // One line of ten
    let small = TEN.replace("Retry the upload twice.", "Gone.");
    let chain = build(&[snap(TEN), external(TEN, &small)]);
    assert_eq!(
        relocate(&anchor, &Doc::new(&small), &chain),
        nowhere(AnchorStatus::Orphaned)
    );
    let chain = build(&[snap(TEN), git(TEN, &small)]);
    assert_eq!(
        relocate(&anchor, &Doc::new(&small), &chain),
        nowhere(AnchorStatus::Suspended)
    );
}

#[test]
fn a_fuzzy_match_after_an_outside_rewrite_is_suspended() {
    let md1 = "Intro.\n\nthe upload tries three times and then gives up\n\nOutro.\n";
    let chain = build(&[snap(SENTENCE), git(SENTENCE, md1)]);
    let got = relocate(&sentence(), &Doc::new(md1), &chain);
    assert_eq!(got.status, AnchorStatus::Suspended, "{got:?}");
}

// Block and section scope

#[test]
fn a_block_anchor_moves_with_its_block() {
    let anchor = block(BASE, "Retry the upload twice.", (3, 3), Scope::Block);
    assert_eq!(
        relocate_on(&anchor, &[BASE]),
        at(AnchorStatus::Ok, (3, 3), 8..31, (3, 3), None)
    );
    let above = format!("Intro.\n\n{BASE}");
    assert_eq!(
        relocate_on(&anchor, &[BASE, &above]),
        at(AnchorStatus::Moved, (5, 5), 16..39, (5, 5), None)
    );
}

#[test]
fn a_block_whose_text_changed_is_fuzzy_on_that_block() {
    let anchor = block(BASE, "Retry the upload twice.", (3, 3), Scope::Block);
    let md1 = "Alpha.\n\nRetry the upload three times.\n\nOmega.\n";
    assert_eq!(
        relocate_on(&anchor, &[BASE, md1]),
        at(AnchorStatus::Fuzzy, (3, 3), 8..37, (3, 3), None)
    );
}

#[test]
fn a_section_anchor_runs_to_the_next_heading_of_its_level_or_above() {
    let md0 = "## One\n\nText one.\n\n### Sub\n\nSub text.\n\n## Two\n\nText two.\n";
    let md1 = "## One\n\nText one.\n\nMore.\n\n### Sub\n\nSub text.\n\n## Two\n\nText two.\n";
    let anchor = block(md0, "## One", (1, 8), Scope::Section);

    let got = relocate_on(&anchor, &[md0]);
    assert_eq!(
        (got.status, got.lines, got.block, got.section.as_deref()),
        (AnchorStatus::Ok, Some((1, 8)), Some((1, 1)), Some("One"))
    );
    assert!(text_at(md0, &got).starts_with("## One"), "{got:?}");

    // The heading stayed put and its section grew
    let got = relocate_on(&anchor, &[md0, md1]);
    assert_eq!(
        (got.status, got.lines, got.block, got.section.as_deref()),
        (AnchorStatus::Ok, Some((1, 10)), Some((1, 1)), Some("One"))
    );

    let moved = format!("Preface.\n\n{md1}");
    let got = relocate_on(&anchor, &[md0, md1, &moved]);
    assert_eq!(
        (got.status, got.lines, got.block, got.section.as_deref()),
        (
            AnchorStatus::Moved,
            Some((3, 12)),
            Some((3, 3)),
            Some("One")
        )
    );
}

// Adversary killers

const TWIN_P: &str = "Filler paragraph that is long enough to fill all context.";
const TWIN_Q: &str = "Retry the upload.";
const TWIN_CTX: (&str, &str) = (
    "ong enough to fill all context. ",
    " Filler paragraph that is long e",
);

fn twin_texts() -> (String, String) {
    let md0 = format!("{TWIN_P}\n\n{TWIN_Q}\n\n{TWIN_P}\n\n{TWIN_Q}\n\n{TWIN_P}\n");
    let md1 = format!("New intro.\n\nSome new words.\n\n{md0}");
    (md0, md1)
}

#[test]
fn twins_with_the_same_context_stay_put_on_their_own_rev() {
    let (md0, _) = twin_texts();
    let second = quote(&md0, TWIN_Q, TWIN_CTX, (7, 7));
    assert_eq!(
        relocate_on(&second, &[&md0]),
        at(AnchorStatus::Ok, (7, 7), 137..154, (7, 7), None)
    );
}

#[test]
fn the_chain_moves_the_first_twin_too() {
    let (md0, md1) = twin_texts();
    let first = quote(&md0, TWIN_Q, TWIN_CTX, (3, 3));
    assert_eq!(
        relocate_on(&first, &[&md0, &md1]),
        at(AnchorStatus::Moved, (7, 7), 88..105, (7, 7), None)
    );
}

#[test]
fn a_multi_line_quote_shifts_both_ends() {
    let md0 = "Alpha.\n\nretries three times\nbefore giving up.\n";
    let md1 = format!("Intro.\n\n{md0}");
    let anchor = quote(
        md0,
        "three times\nbefore",
        ("Alpha. retries ", " giving up. "),
        (3, 4),
    );
    assert_eq!(
        relocate_on(&anchor, &[md0, &md1]),
        at(AnchorStatus::Moved, (5, 6), 24..42, (5, 6), None)
    );
}

#[test]
fn whitespace_runs_and_crlf_map_back_to_every_byte() {
    let md = "Retry  the upload.\n";
    let anchor = quote(md, "Retry  the", ("", " upload. "), (1, 1));
    assert_eq!(
        relocate_on(&anchor, &[md]),
        at(AnchorStatus::Ok, (1, 1), 0..10, (1, 1), None)
    );

    let crlf = "line one\r\nline two\r\n";
    let anchor = quote(crlf, "one\r\nline", ("line ", " two "), (1, 2));
    assert_eq!(
        relocate_on(&anchor, &[crlf]),
        at(AnchorStatus::Ok, (1, 2), 5..14, (1, 2), None)
    );
    let above = format!("Intro.\r\n\r\n{crlf}");
    assert_eq!(
        relocate_on(&anchor, &[crlf, &above]),
        at(AnchorStatus::Moved, (3, 4), 15..24, (3, 4), None)
    );
}

#[test]
fn a_split_block_is_not_ok() {
    let md0 = "Alpha.\n\nRetry the upload\ntwice now.\n\nOmega.\n";
    let md1 = "Alpha.\n\nRetry the upload\n\ntwice now.\n\nOmega.\n";
    let anchor = block(md0, "Retry the upload\ntwice now.", (3, 4), Scope::Block);
    let got = relocate_on(&anchor, &[md0, md1]);
    assert!(
        !matches!(got.status, AnchorStatus::Ok | AnchorStatus::Moved),
        "{got:?}"
    );
    assert_eq!(got.block, Some((3, 3)), "{got:?}");
}

#[test]
fn l2_takes_the_best_scored_hit() {
    let md0 = "## One\n\nThen we said retry now please do.\n\n## Two\n\nOther.\n";
    let md1 = "## One\n\nGone.\n\n## Two\n\nOther. Ok we said retry now.\n\nThen we said retry now please do.\n";
    let ctx = ("## One Then we said ", " please do. ## Two Other. ");
    let anchor = quote(md0, "retry now", ctx, (3, 3));
    assert_eq!(
        relocate_on(&anchor, &[md0, md1]),
        at(AnchorStatus::Moved, (9, 9), 66..75, (9, 9), Some("Two"))
    );
}

#[test]
fn evidence_is_8_chars_on_one_side() {
    let md0 = "## One\n\nabcdefgh retry.\n\n## Two\n\nNothing.\n";
    let anchor = quote(
        md0,
        "retry",
        ("## One abcdefgh ", ". ## Two Nothing. "),
        (3, 3),
    );
    for (s, status) in [
        ("xbcdefgh", AnchorStatus::Moved),
        ("yycdefgh", AnchorStatus::Fuzzy),
    ] {
        let md1 = format!("## One\n\nGone.\n\n## Two\n\n{s} retry.\n");
        assert_eq!(
            relocate_on(&anchor, &[md0, &md1]),
            at(status, (7, 7), 32..37, (7, 7), Some("Two")),
            "{s}"
        );
    }
}

#[test]
fn a_reword_moved_past_its_old_place_is_still_found() {
    // One hunk deletes it, another inserts the reworded line below the next paragraph
    let md1 = "Intro.\n\nMid.\n\nthe upload tries three times and then gives up\n\nOutro.\n";
    let md0 = "Intro.\n\nthe upload retries three times before giving up\n\nMid.\n\nOutro.\n";
    let anchor = quote(
        md0,
        "the upload retries three times before giving up",
        ("Intro. ", " Mid. Outro. "),
        (3, 3),
    );
    assert_eq!(
        relocate_on(&anchor, &[md0, md1]),
        at(AnchorStatus::Fuzzy, (5, 5), 14..60, (5, 5), None)
    );
}

#[test]
fn the_ratio_boundary_is_three_quarters() {
    // 0.7586
    let md1 = "Intro.\n\nthe upload retries twice before it quits\n\nOutro.\n";
    assert_eq!(
        relocate_on(&sentence(), &[SENTENCE, md1]),
        at(AnchorStatus::Fuzzy, (3, 3), 8..48, (3, 3), None)
    );
    // 0.7447
    let md1 = "Intro.\n\nthe upload retries four times and then gives up\n\nOutro.\n";
    let got = relocate_on(&sentence(), &[SENTENCE, md1]);
    assert_eq!(got.status, AnchorStatus::Orphaned, "{got:?}");
}

#[test]
fn a_reword_far_below_an_insert_is_fuzzy_at_its_new_line() {
    let md1 = format!(
        "Intro.\n\n{}the upload tries three times and then gives up\n\nOutro.\n",
        "f\n\n".repeat(10)
    );
    assert_eq!(
        relocate_on(&sentence(), &[SENTENCE, &md1]),
        at(AnchorStatus::Fuzzy, (23, 23), 38..84, (23, 23), None)
    );
}

#[test]
fn a_rev_only_step_falls_to_search() {
    let above = format!("Intro.\n\n{BASE}");
    let rev_only = json!({"by": {"type": "birta"}, "op": "external",
                          "rev": rev(BASE), "to_rev": rev(&above), "patch": null});
    let chain = build(&[snap(BASE), rev_only]);
    assert_eq!(
        relocate(&the_upload(), &Doc::new(&above), &chain),
        at(AnchorStatus::Moved, (5, 5), 22..32, (5, 5), None)
    );

    // The old line number now holds a twin; with no hunks, nothing says the anchor stayed
    let twinned =
        "Alpha.\n\nRetry the upload twice.\n\nAlpha.\n\nRetry the upload twice.\n\nOmega.\n";
    let rev_only = json!({"by": {"type": "birta"}, "op": "external",
                          "rev": rev(BASE), "to_rev": rev(twinned), "patch": null});
    let chain = build(&[snap(BASE), rev_only]);
    assert_eq!(
        relocate(&the_upload(), &Doc::new(twinned), &chain),
        at(AnchorStatus::Moved, (7, 7), 47..57, (7, 7), None)
    );
}

#[test]
fn equal_hits_take_the_nearest_to_the_mapped_line_not_the_old_one() {
    // Twenty lines land between the first twin and the anchor's line, which is replaced
    let md0 = "x retry now y\n\nkeep retry now please\n\nf\n\nf\n\nx retry now y\n";
    let md1 = format!(
        "x retry now y\n{}\ngone\n\nf\n\nf\n\nx retry now y\n",
        "g\n".repeat(20)
    );
    let ctx = ("x retry now y keep ", " please f f x retry now y ");
    let anchor = quote(md0, "retry now", ctx, (3, 3));
    assert_eq!(
        relocate_on(&anchor, &[md0, &md1]),
        at(AnchorStatus::Fuzzy, (29, 29), 69..78, (29, 29), None)
    );
}

#[test]
#[ignore = "release bench: cargo test --release --test review_ladder_test -- --ignored"]
fn a_hundred_overlapped_anchors_relocate_in_under_10_ms() {
    let para = |i: usize, word: &str| {
        format!("Paragraph {i} says {word} about item {i} and nothing else.\n\n")
    };
    let mut text: String = (0..1700).map(|i| para(i, "something")).collect();
    assert!(text.len() > 100_000);
    let md0 = text.clone();
    let mut ops = vec![snap(&md0)];
    for k in 0..400 {
        let i = k * 4;
        let new = text.replacen(&para(i, "something"), &para(i, &format!("changed{k}")), 1);
        ops.push(edit(&text, &new));
        text = new;
    }
    let chain = build(&ops);
    let doc = Doc::new(&text);
    let anchors: Vec<Anchor> = (0..100)
        .map(|k| {
            let i = k * 4;
            let line = (2 * i + 1) as u32;
            quote(
                &md0,
                &format!("says something about item {i}"),
                ("", ""),
                (line, line),
            )
        })
        .collect();
    let start = std::time::Instant::now();
    for anchor in &anchors {
        let got = relocate(anchor, &doc, &chain);
        assert_ne!(got.status, AnchorStatus::Ok, "{got:?}");
    }
    let took = start.elapsed();
    assert!(took.as_millis() < 10, "{took:?}");
}

// Lead rulings

#[test]
fn a_block_that_grows_is_pointed_at_whole() {
    let md0 = "Alpha.\n\nRetry the upload\ntwice now.\n\nOmega.\n";
    let md1 = "Alpha.\n\nRetry the upload\ntwice now.\nThen stop.\n\nOmega.\n";
    let anchor = block(md0, "Retry the upload\ntwice now.", (3, 4), Scope::Block);
    assert_eq!(
        relocate_on(&anchor, &[md0, md1]),
        at(AnchorStatus::Ok, (3, 5), 8..46, (3, 5), None)
    );
}

#[test]
fn whitespace_in_code_is_significant() {
    let code0 = "Intro.\n\n```\nlet  total = 1;\n```\n";
    let code1 = "Intro.\n\n```\nlet total = 1;\n```\n";
    let anchor = quote(code0, "let  total", ("Intro. ``` ", " = 1; ``` "), (4, 4));
    assert_eq!(
        relocate_on(&anchor, &[code0]),
        at(AnchorStatus::Ok, (4, 4), 12..22, (3, 5), None)
    );
    let got = relocate_on(&anchor, &[code0, code1]);
    assert_eq!(
        (got.status, got.lines),
        (AnchorStatus::Fuzzy, Some((4, 4))),
        "{got:?}"
    );

    // Line endings are the one thing code matching folds
    let crlf = "```\r\nlet a = 1;\r\nlet b = 2;\r\n```\r\n";
    let anchor = quote(crlf, "1;\r\nlet b", ("``` let a = ", " = 2; ``` "), (2, 3));
    assert_eq!(
        relocate_on(&anchor, &[crlf]),
        at(AnchorStatus::Ok, (2, 3), 13..22, (1, 4), None)
    );

    // The same edit in prose only reflows whitespace
    let prose0 = "Intro.\n\nlet  total be one.\n";
    let prose1 = "Intro.\n\nlet total be one.\n";
    let anchor = quote(prose0, "let  total", ("Intro. ", " be one. "), (3, 3));
    assert_eq!(
        relocate_on(&anchor, &[prose0, prose1]),
        at(AnchorStatus::Moved, (3, 3), 8..17, (3, 3), None)
    );
}

#[test]
fn a_heading_that_changes_level_keeps_its_anchors() {
    let md0 = "## One\n\nText one.\n\n### Sub\n\nSub text.\n\n## Two\n\nText two.\n";
    let md1 = "### One\n\nText one.\n\n### Sub\n\nSub text.\n\n## Two\n\nText two.\n";

    // Its section now ends at the next heading of level 3 or above
    let section = block(md0, "## One", (1, 8), Scope::Section);
    let got = relocate_on(&section, &[md0, md1]);
    assert_eq!(
        (got.status, got.lines, got.block, got.section.as_deref()),
        (AnchorStatus::Moved, Some((1, 4)), Some((1, 1)), Some("One"))
    );
    assert!(text_at(md1, &got).starts_with("### One"), "{got:?}");

    let heading = block(md0, "## One", (1, 1), Scope::Block);
    assert_eq!(
        relocate_on(&heading, &[md0, md1]),
        at(AnchorStatus::Moved, (1, 1), 0..7, (1, 1), Some("One"))
    );

    let ctx = ("## ", " Text one. ### Sub Sub text. ## ");
    let word = quote(md0, "One", ctx, (1, 1));
    assert_eq!(
        relocate_on(&word, &[md0, md1]),
        at(AnchorStatus::Moved, (1, 1), 4..7, (1, 1), Some("One"))
    );
}
