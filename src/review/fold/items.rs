//! Notes and what became of them: amends, retracts, replies, and whether the turn that
//! took each one answered it.

use std::collections::HashMap;
use std::sync::Arc;

use super::turns::{RoundRec, Turns};
use super::{RoundKind, TurnIndex};
use crate::rev::Rev;
use crate::review::chain::Chain;
use crate::review::doc::Doc;
use crate::review::ladder::{self, Located};
use crate::review::ops::{
    Anchor, At, By, ByKind, CommentKind, Hunk, Line, Op, ReplyKind, Scope, Verdict,
};
use crate::review::state::ItemStatus;

/// A comment, human or agent, as its amends left it.
pub(super) struct Note {
    pub n: u32,
    pub by: By,
    pub cid: Option<String>,
    pub ts: u64,
    pub at: At,
    pub verdict: Option<Verdict>,
    pub body: Option<String>,
    pub to: Option<u32>,
    pub kind: Option<CommentKind>,
    pub amended: Vec<u32>,
    pub retracted: bool,
    /// A human reply with a verdict decided a `later` note.
    pub superseded: bool,
    pub replies: Vec<ReplyRec>,
}

impl Note {
    pub fn human(&self) -> bool {
        self.by.kind == ByKind::Human
    }

    /// An agent's question for the human.
    pub fn blank(&self) -> bool {
        self.by.kind == ByKind::Agent && self.verdict == Some(Verdict::Ask)
    }

    pub fn later(&self) -> bool {
        self.verdict == Some(Verdict::Later) && !self.superseded
    }

    pub fn anchor(&self) -> Option<&Anchor> {
        match &self.at {
            At::Anchor(anchor) => Some(anchor),
            At::Doc | At::Dir => None,
        }
    }
}

pub(super) struct ReplyRec {
    pub n: u32,
    pub by: By,
    pub ts: u64,
    pub kind: Option<ReplyKind>,
    pub verdict: Option<Verdict>,
    pub body: Option<String>,
}

#[derive(Default)]
pub(super) struct Notes {
    pub notes: Vec<Note>,
    /// Lines of human replies, which go back with the next hand-back.
    pub human_replies: Vec<u32>,
}

impl Notes {
    pub fn get(&self, n: u32) -> Option<&Note> {
        self.notes
            .binary_search_by_key(&n, |note| note.n)
            .ok()
            .map(|i| &self.notes[i])
    }

    fn get_mut(&mut self, n: u32) -> Option<&mut Note> {
        self.notes
            .binary_search_by_key(&n, |note| note.n)
            .ok()
            .map(|i| &mut self.notes[i])
    }

    /// Takes in `line`; `index` has seen only the lines before it.
    pub fn step(&mut self, line: &Line, index: &TurnIndex, warnings: &mut Vec<String>) {
        let ts = line.ts.unwrap_or(0);
        match &line.op {
            Op::Comment {
                at,
                verdict,
                body,
                to,
                kind,
                ..
            } if matches!(line.by.kind, ByKind::Human | ByKind::Agent) => {
                self.notes.push(Note {
                    n: line.n,
                    by: line.by.clone(),
                    cid: line.cid.clone(),
                    ts,
                    at: at.clone(),
                    verdict: *verdict,
                    body: body.clone(),
                    to: *to,
                    kind: *kind,
                    amended: Vec::new(),
                    retracted: false,
                    superseded: false,
                    replies: Vec::new(),
                });
            }
            Op::Amend { to, body, verdict } if line.by.kind == ByKind::Human => {
                match self.draft_mut(*to, index) {
                    Some(note) => {
                        if body.is_some() {
                            note.body.clone_from(body);
                        }
                        if verdict.is_some() {
                            note.verdict = *verdict;
                        }
                        note.amended.push(line.n);
                    }
                    None => warnings.push(unchangeable(line.n, "amend", *to)),
                }
            }
            Op::Retract { to } if line.by.kind == ByKind::Human => {
                match self.draft_mut(*to, index) {
                    Some(note) => note.retracted = true,
                    None => warnings.push(unchangeable(line.n, "retract", *to)),
                }
            }
            Op::Reply {
                to,
                verdict,
                body,
                kind,
                ..
            } => {
                let human = line.by.kind == ByKind::Human;
                let Some(note) = self.get_mut(*to) else {
                    warnings.push(format!(
                        "line {}: reply to {to} ignored: no such note",
                        line.n
                    ));
                    return;
                };
                if human && verdict.is_some() && note.later() {
                    note.superseded = true;
                }
                note.replies.push(ReplyRec {
                    n: line.n,
                    by: line.by.clone(),
                    ts,
                    kind: *kind,
                    verdict: *verdict,
                    body: body.clone(),
                });
                if human {
                    self.human_replies.push(line.n);
                }
            }
            _ => {}
        }
    }

    /// Human note `to`, while nothing has handed it back yet.
    fn draft_mut(&mut self, to: u32, index: &TurnIndex) -> Option<&mut Note> {
        let sent = index
            .rounds
            .iter()
            .any(|r| r.withdrawn.is_none() && r.n > to && r.upto >= to);
        self.get_mut(to)
            .filter(|note| note.human() && !note.retracted && !sent)
    }
}

fn unchangeable(n: u32, op: &str, to: u32) -> String {
    format!("line {n}: {op} of {to} ignored: not a human note that is still a draft")
}

/// The round that handed line `n` back: the first one covering it that stands.
pub(super) fn cover(rounds: &[RoundRec], n: u32, ignore: Option<u32>) -> Option<&RoundRec> {
    rounds
        .iter()
        .find(|r| !r.withdrawn && Some(r.n) != ignore && r.n > n && r.upto >= n)
}

/// What the turns that held a note did about it.
#[derive(Default)]
pub(super) struct Outcome {
    pub done: bool,
    /// The turn whose `done` counted.
    pub done_in: Option<u32>,
    pub answered: bool,
    pub didnt: bool,
    pub noted: bool,
    /// Ids of changes on the note's block.
    pub changes: Vec<String>,
    pub no_change: bool,
}

/// Texts and docs of the revs the fold looks at, each built once.
pub(super) struct Texts<'a> {
    chain: &'a Chain,
    docs: HashMap<Rev, Option<Arc<Doc>>>,
}

impl<'a> Texts<'a> {
    pub fn new(chain: &'a Chain) -> Self {
        Self {
            chain,
            docs: HashMap::new(),
        }
    }

    fn doc(&mut self, rev: &Rev) -> Option<Arc<Doc>> {
        let chain = self.chain;
        self.docs
            .entry(rev.clone())
            .or_insert_with(|| chain.text(rev).map(|text| Arc::new(Doc::new(&text))))
            .clone()
    }

    /// The lines a note's verdict or change covers in the text of `rev`.
    fn span(&mut self, anchor: &Anchor, rev: &Rev) -> Option<(Arc<Doc>, (u32, u32))> {
        let doc = self.doc(rev)?;
        let span = span_of(anchor, &ladder::relocate(anchor, &doc, self.chain))?;
        Some((doc, span))
    }
}

/// The lines a located anchor covers: a quote's block, or a block's or section's lines.
pub(super) fn span_of(anchor: &Anchor, located: &Located) -> Option<(u32, u32)> {
    match anchor.scope {
        Scope::Quote => located.block,
        Scope::Block | Scope::Section => located.lines,
    }
}

/// Whether hunk `h`, in its old text, touches lines `span`.
fn touches(h: &Hunk, (first, last): (u32, u32)) -> bool {
    let a = h.a;
    if h.d.is_empty() {
        // Inserted after line `a`
        return first <= a && a <= last;
    }
    let end = a + h.d.len() as u32;
    a < last && first <= end
}

/// The source of `span` in `doc`.
fn span_text(doc: &Doc, (first, last): (u32, u32)) -> Option<&str> {
    let start = doc.lines.range(first)?.start;
    let end = doc.lines.range(last)?.end;
    doc.text.get(start..end)
}

pub(super) fn outcome(
    note: &Note,
    held_by: &[u32],
    turns: &Turns,
    lines: &HashMap<u32, &Line>,
    texts: &mut Texts,
) -> Outcome {
    let mut out = Outcome::default();
    if let Some(dones) = turns.dones.get(&note.n) {
        out.done_in = dones.iter().copied().find(|t| held_by.contains(t));
        out.done = out.done_in.is_some();
    }
    for reply in &note.replies {
        if turns
            .replies
            .get(&reply.n)
            .is_some_and(|t| held_by.contains(t))
        {
            match reply.kind {
                Some(ReplyKind::Answer) => out.answered = true,
                Some(ReplyKind::Didnt) => out.didnt = true,
                Some(ReplyKind::Note) | None => out.noted = true,
            }
        }
    }
    let Some(anchor) = note.anchor() else {
        return out;
    };
    let mut writes: Vec<(u32, u32)> = turns
        .writes
        .iter()
        .filter(|(_, t)| held_by.contains(t))
        .map(|(&n, &t)| (n, t))
        .collect();
    writes.sort_unstable();
    for (n, _) in writes {
        let Some(Op::External {
            rev,
            patch: Some(hunks),
            ..
        }) = lines.get(&n).map(|line| &line.op)
        else {
            continue;
        };
        let Some((_, span)) = texts.span(anchor, rev) else {
            continue;
        };
        for (i, h) in hunks.iter().enumerate() {
            if touches(h, span) {
                out.changes.push(format!("{n}.{i}"));
            }
        }
    }
    if let Some(turn) = out.done_in.and_then(|t| turns.turn(t)) {
        let (from, to) = (
            texts.chain.rev_at_line(turn.n).cloned(),
            texts.chain.rev_at_line(turn.close()).cloned(),
        );
        out.no_change = match (from, to) {
            (Some(from), Some(to)) if from == to => true,
            (Some(from), Some(to)) => {
                let before = texts.span(anchor, &from);
                let after = texts.span(anchor, &to);
                match (before, after) {
                    (Some((a, sa)), Some((b, sb))) => span_text(&a, sa) == span_text(&b, sb),
                    _ => false,
                }
            }
            _ => false,
        };
    }
    out
}

/// Where a human note stands, given the round that sent it and what was done about it.
pub(super) fn status(
    note: &Note,
    round: Option<&RoundRec>,
    turns: &Turns,
    out: &Outcome,
) -> ItemStatus {
    if !note.human() {
        let answered = note.replies.iter().any(|r| r.by.kind == ByKind::Human);
        return match (note.blank(), answered) {
            (true, true) => ItemStatus::Answered,
            (true, false) => ItemStatus::Open,
            (false, _) => ItemStatus::Handled,
        };
    }
    if note.retracted {
        return ItemStatus::Retracted;
    }
    let Some(round) = round else {
        return ItemStatus::Draft;
    };
    let settled = || {
        if note.later() {
            ItemStatus::Later
        } else if satisfied(note, out) {
            ItemStatus::Handled
        } else {
            ItemStatus::NotHandled
        }
    };
    if round.kind == RoundKind::Approve {
        return if note.later() {
            ItemStatus::Later
        } else {
            ItemStatus::Handled
        };
    }
    match round.taker.and_then(|t| turns.turn(t)) {
        Some(turn) if turn.open() => ItemStatus::PickedUp,
        Some(_) => settled(),
        // Taken back from a claim: only a late respond settles it
        None if !note.later() && satisfied(note, out) && has_outcome(out) => ItemStatus::Handled,
        None => ItemStatus::Sent,
    }
}

fn has_outcome(out: &Outcome) -> bool {
    out.done || out.answered || out.didnt || out.noted || !out.changes.is_empty()
}

/// Which verdicts need something from the agent, and what counts.
fn satisfied(note: &Note, out: &Outcome) -> bool {
    match note.verdict {
        Some(Verdict::Yes | Verdict::Stet | Verdict::Later) => true,
        Some(Verdict::Ask) => out.answered,
        _ => has_outcome(out),
    }
}
