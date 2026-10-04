//! One writer at a time: from a hand-back until its reply, the reviewer sees the doc as
//! it was handed back, folded without what the agent has done in the turn so far.

use std::sync::Arc;

use super::chain::Chain;
use super::fold::{self, RoundKind};
use super::ops::{ByKind, Op, Parsed};
use crate::rev::Rev;

/// The hand-back a doc is held at: the submit's line and the doc's rev on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldAt {
    pub rev: Rev,
    pub n: u32,
}

/// The held doc: where it is held, the log without the turn's agent lines, and the text.
pub struct HeldView {
    pub at: HeldAt,
    pub log: Parsed,
    pub text: Arc<str>,
}

/// The submit line of a standing hand-back: the latest one not withdrawn, until its
/// turn's yield or a release.
fn standing(parsed: &Parsed) -> Option<u32> {
    let index = fold::scan(parsed, 0);
    let round = index
        .rounds
        .iter()
        .rev()
        .find(|r| r.kind == RoundKind::Submit && r.withdrawn.is_none())?;
    (round.yielded.is_none() && !round.released).then_some(round.n)
}

/// Where the doc is held: a standing hand-back whose text the log can rebuild. The one
/// test of "held" that every check uses.
pub fn at(parsed: &Parsed) -> Option<HeldAt> {
    let n = standing(parsed)?;
    let chain = Chain::build(parsed);
    let rev = chain.rev_at_line(n)?.clone();
    chain.knows(&rev).then_some(HeldAt { rev, n })
}

/// The held view of `parsed`, or `None` when nothing is held.
pub fn view(parsed: &Parsed) -> Option<HeldView> {
    let n = standing(parsed)?;
    let chain = Chain::build(parsed);
    let rev = chain.rev_at_line(n)?.clone();
    let text = chain.text(&rev)?;
    Some(HeldView {
        at: HeldAt { rev, n },
        log: held_log(parsed, n),
        text,
    })
}

/// The log as the reviewer sees it while held: after the submit, only human lines and
/// the claim that took it.
fn held_log(parsed: &Parsed, n: u32) -> Parsed {
    let mut log = parsed.clone();
    log.lines
        .retain(|l| l.n <= n || l.by.kind == ByKind::Human || matches!(l.op, Op::Claim { .. }));
    log
}
