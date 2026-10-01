//! Notes and what became of them: amends, retracts, replies, and whether the turn that
//! took each one answered it.

use super::texts::{Texts, touches, untouched};
use super::turns::{RoundRec, Turns};
use super::{RoundKind, TurnIndex};
use crate::rev::Rev;
use crate::review::ops::{Anchor, At, By, ByKind, CommentKind, Line, Op, ReplyKind, Verdict};
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
            Op::Amend {
                to,
                body,
                verdict,
                drop_verdict,
            } if line.by.kind == ByKind::Human => match self.draft_mut(*to, index) {
                Some(note) => {
                    if body.is_some() {
                        note.body.clone_from(body);
                    }
                    if verdict.is_some() || *drop_verdict {
                        note.verdict = *verdict;
                    }
                    note.amended.push(line.n);
                }
                None => warnings.push(unchangeable(line.n, "amend", *to)),
            },
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
    /// A `covers` of the note bound to text a holding turn wrote.
    pub covered: bool,
    /// Ids of changes on the note's block.
    pub changes: Vec<String>,
    pub no_change: bool,
    /// The turn's first and last revs, when only comparing their texts can tell `no_change`.
    pub compare: Option<(Rev, Rev)>,
}

pub(super) fn outcome(note: &Note, held_by: &[u32], turns: &Turns, texts: &mut Texts) -> Outcome {
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
    let mut writes: Vec<u32> = turns
        .writes
        .iter()
        .filter(|(_, t)| held_by.contains(t))
        .map(|(&n, _)| n)
        .collect();
    writes.sort_unstable();
    for n in writes {
        // The chain has the hunks even of a write recorded by rev only
        let Some((_, step)) = texts.step_at(n) else {
            continue;
        };
        let Some(hunks) = &step.hunks else {
            continue;
        };
        let Some(span) = texts.span_at(note.n, anchor, &step.from) else {
            continue;
        };
        for (i, h) in hunks.iter().enumerate() {
            if touches(h, span) {
                out.changes.push(format!("{n}.{i}"));
            }
        }
    }
    if let Some(turn) = out.done_in.and_then(|t| turns.turn(t)) {
        let chain = texts.chain;
        let (from, to) = (
            chain.rev_at_line(turn.n).cloned(),
            chain.rev_at_line(turn.close()).cloned(),
        );
        if let (Some(from), Some(to)) = (from, to) {
            let span = texts.span_at(note.n, anchor, &from);
            let steps = chain.steps(&from, &to);
            out.no_change = from == to
                || matches!((span, steps), (Some(span), Some(steps)) if untouched(steps, span));
            if !out.no_change {
                out.compare = Some((from, to));
            }
        }
    }
    out
}

/// Whether the note's span reads the same at both ends of the turn, for a `done` whose
/// turn wrote on or near it. The texts are expected prefetched.
pub(super) fn same_text(note: &Note, (from, to): &(Rev, Rev), texts: &mut Texts) -> bool {
    let Some(anchor) = note.anchor() else {
        return false;
    };
    let mut read = |rev: &Rev| {
        let span = texts.span_at(note.n, anchor, rev)?;
        let (text, lines) = (texts.text(rev)?, texts.lines(rev)?);
        let bytes = lines.range(span.0)?.start..lines.range(span.1)?.end;
        text.get(bytes).map(str::to_string)
    };
    let before = read(from);
    before.is_some() && before == read(to)
}

/// Where a human note stands, given the round that sent it, the claim holding it now,
/// and what was done about it.
pub(super) fn status(
    note: &Note,
    round: Option<&RoundRec>,
    holder: Option<u32>,
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
    match holder.and_then(|t| turns.turn(t)) {
        Some(turn) if turn.open() => ItemStatus::PickedUp,
        Some(_) => settled(),
        // Taken back from a claim: only a late respond settles it
        None if !note.later() && satisfied(note, out) && has_outcome(out) => ItemStatus::Handled,
        None => ItemStatus::Sent,
    }
}

fn has_outcome(out: &Outcome) -> bool {
    out.done || out.answered || out.didnt || out.noted || out.covered || !out.changes.is_empty()
}

/// Which verdicts need something from the agent, and what counts.
fn satisfied(note: &Note, out: &Outcome) -> bool {
    match note.verdict {
        Some(Verdict::Yes | Verdict::Stet | Verdict::Later) => true,
        Some(Verdict::Ask) => out.answered,
        _ => has_outcome(out),
    }
}

/// The claim a note its holder left open is carried to: the first turn opened after the
/// holder's ended. The round view lists such a note for that claim, by [`carries`].
pub(super) fn carried_to(turns: &Turns, holder: Option<u32>, status: ItemStatus) -> Option<u32> {
    if !carries(status) {
        return None;
    }
    let held = turns.turn(holder?)?;
    let ended = held.end.or(held.released)?;
    turns.turns.iter().find(|t| t.n > ended).map(|t| t.n)
}

/// Whether a note is still owed an outcome by whichever claim comes next.
pub fn carries(status: ItemStatus) -> bool {
    status == ItemStatus::NotHandled
}
