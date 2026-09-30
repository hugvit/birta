//! The fold's time budget on a large review: a 100 KB doc and 400 ops.

use std::time::{Duration, Instant};

use birta::review::anchor_from_req;
use birta::review::chain::Chain;
use birta::review::doc::Doc;
use birta::review::fold;
use birta::review::ops::{AnchorReq, BlockScope};
use birta::review::state::AtKind;
use serde_json::Value;

mod support {
    pub mod scenario;
}
use support::scenario::Scenario;

const CLAUDE: &str = "claude";
const ROUNDS: usize = 31;
/// Tries at the bound; a busy machine gets another go before the test fails.
const RUNS: usize = 3;

/// About 100 KB of sections, each a two-line paragraph and twelve list items.
fn big_doc() -> String {
    let mut text = String::from("# Big\n\n");
    let mut i = 0;
    while text.len() < 100_000 {
        text += &format!(
            "## Section {i}\n\nParagraph {i} explains the section in a sentence long enough to be real.\nIt goes on for a second line, as prose does.\n\n"
        );
        for j in 0..12 {
            text += &format!("- item {i}.{j} with a few words after it\n");
        }
        text.push('\n');
        i += 1;
    }
    text
}

fn item_line(r: usize, j: usize) -> String {
    format!("- item {r}.{j} with a few words after it")
}

/// An anchor built on `doc` by the one constructor.
fn at(doc: &Doc, req: AnchorReq) -> Value {
    serde_json::to_value(anchor_from_req(&req, doc, &doc.rev).unwrap()).unwrap()
}

/// Each round: three notes, a hand-back, a claim, three writes, a `done`, an answer, a
/// turn note, a yield, and the human marking the writes seen. 13 ops, 31 rounds.
fn big_review() -> Scenario {
    let mut s = Scenario::new(&big_doc());
    for r in 0..ROUNDS {
        let doc = Doc::new(s.text());
        let first = item_line(r, 0);
        let stop = doc
            .blocks
            .stops
            .iter()
            .find(|st| {
                doc.lines
                    .range(st.lines.0)
                    .is_some_and(|b| doc.text[b] == *first)
            })
            .unwrap();
        let block = at(
            &doc,
            AnchorReq::Block {
                sp: stop.sp.clone(),
                scope: BlockScope::Block,
            },
        );
        let quote = at(
            &doc,
            AnchorReq::Quote {
                sp: None,
                q: format!("Paragraph {r} explains"),
                pre: String::new(),
                suf: String::new(),
                off: None,
                tl: None,
            },
        );
        s.comment(block, Some("no"), Some("reword"));
        let q = s.comment(quote, None, Some("wordy"));
        let d = s.note(&format!("round {r}: anything else?"));
        s.submit();
        let turn = s.claim(CLAUDE);
        let mut writes = Vec::new();
        for (old, new) in [
            (first.clone(), format!("{first}, reworded")),
            (item_line(r, 5), format!("{}, edited", item_line(r, 5))),
            (
                item_line(r, 11),
                format!("{}\n- item {r}.12 added", item_line(r, 11)),
            ),
        ] {
            let text = s.replaced(&old, &new);
            writes.push(s.external_diffed(&text));
        }
        s.done(CLAUDE, q, turn);
        s.agent_reply(CLAUDE, d, "answer", "no");
        s.turn_note(CLAUDE, "Reworded and added one.", turn);
        s.yield_turn(CLAUDE, turn);
        let ids: Vec<String> = writes.iter().map(|n| format!("{n}.0")).collect();
        s.seen(&ids.iter().map(String::as_str).collect::<Vec<_>>());
    }
    s
}

#[test]
fn a_100_kb_doc_with_400_ops_folds_in_time() {
    let s = big_review();
    let parsed = s.parsed();
    assert!(s.text().len() >= 100_000);
    assert!(parsed.lines.len() >= 400, "{}", parsed.lines.len());

    let bound = if cfg!(debug_assertions) {
        Duration::from_millis(650)
    } else {
        Duration::from_millis(50)
    };
    let mut best = Duration::MAX;
    let mut state = None;
    for _ in 0..RUNS {
        let start = Instant::now();
        let chain = Chain::build(&parsed);
        let doc = Doc::new(s.text());
        let folded = fold::fold(&parsed, &chain, &doc);
        best = best.min(start.elapsed());
        state = Some(folded.state);
        if best < bound {
            break;
        }
    }
    let state = state.unwrap();
    eprintln!(
        "fold of {} ops on {} bytes: {best:?}",
        parsed.lines.len(),
        s.text().len()
    );

    // The work under the clock is the whole fold, changes included
    assert_eq!(state.changes.len(), 3 * ROUNDS);
    assert_eq!(state.items.len(), 3 * ROUNDS);
    assert_eq!(state.counts.unseen, 0);
    assert_eq!(state.counts.not_handled, 0);
    assert!(
        state
            .items
            .iter()
            .all(|i| i.at.kind == AtKind::Doc || i.at.loc.is_some())
    );
    assert!(best < bound, "{best:?} over {bound:?}");
}
