//! Rounds and turns: what each hand-back sent, which claim took it, and which agent
//! ops and writes belong to which turn.

use std::collections::HashMap;

use super::{RoundKind, TurnIndex};
use crate::rev::Rev;
use crate::review::ops::{ByKind, Line, Op, To};

/// A submit or approve as the fold tracks it.
pub(super) struct RoundRec {
    pub n: u32,
    pub kind: RoundKind,
    pub ts: Option<u64>,
    /// When it counts as handed back, after the far-future clamp.
    pub due: u64,
    pub by: Option<String>,
    pub to: Option<To>,
    pub batch: Option<u64>,
    pub upto: u32,
    /// The approved rev.
    pub rev: Option<Rev>,
    pub withdrawn: bool,
    /// The claim holding a submit's notes, open or ended by a yield.
    pub taker: Option<u32>,
    /// Claims that held it and were released.
    pub released: Vec<u32>,
    /// The claim of an approve, which opens no turn.
    pub approve_claim: Option<u32>,
}

impl RoundRec {
    /// Nobody ever claimed it, so it can still be withdrawn.
    pub fn unclaimed(&self) -> bool {
        self.taker.is_none() && self.released.is_empty() && self.approve_claim.is_none()
    }
}

/// An agent's turn, opened by its claim of a submit.
pub(super) struct Turn {
    pub n: u32,
    pub ts: u64,
    pub agent: Option<String>,
    /// The agent's yield.
    pub end: Option<u32>,
    /// The human's release, after which only late replies and `done`s count.
    pub released: Option<u32>,
    /// The agent's yield after a release.
    pub late_end: Option<u32>,
}

impl Turn {
    pub fn open(&self) -> bool {
        self.end.is_none() && self.released.is_none()
    }

    /// Still taking a released claim's late respond.
    fn late(&self) -> bool {
        self.end.is_none() && self.released.is_some() && self.late_end.is_none()
    }

    /// The line after which nothing more counts for the turn.
    pub fn close(&self) -> u32 {
        self.end.or(self.late_end).unwrap_or(u32::MAX)
    }
}

#[derive(Default)]
pub(super) struct Turns {
    pub rounds: Vec<RoundRec>,
    /// By claim line.
    pub turns: Vec<Turn>,
    /// Turns that took external writes, by the write's line.
    pub writes: HashMap<u32, u32>,
    /// Turns that agent replies belong to, by the reply's line.
    pub replies: HashMap<u32, u32>,
    /// Turns that `done` ops count for, by the note they name.
    pub dones: HashMap<u32, Vec<u32>>,
    /// Turns that agent comments were written in, by the comment's line.
    pub comments: HashMap<u32, u32>,
    pub notes: HashMap<u32, String>,
    /// Text the agent flagged as changed unasked, by turn.
    pub unasked: HashMap<u32, Vec<String>>,
    /// The notes the agent cited for text it wrote, by turn: `(note, quote)`.
    pub covers: HashMap<u32, Vec<(u32, String)>>,
}

impl Turns {
    pub fn turn(&self, n: u32) -> Option<&Turn> {
        self.turns
            .binary_search_by_key(&n, |t| t.n)
            .ok()
            .map(|i| &self.turns[i])
    }

    /// Takes in `line`, which `index` has already stepped over.
    pub fn step(&mut self, line: &Line, index: &TurnIndex, warnings: &mut Vec<String>) {
        let name = &line.by.name;
        match &line.op {
            Op::Submit {
                upto, to, batch, ..
            } => self.push_round(line, index, *upto, to.clone(), *batch, None),
            Op::Approve { rev, .. } => {
                self.push_round(line, index, line.n, None, None, Some(rev.clone()));
            }
            Op::Claim { round, .. } => {
                let won = index
                    .rounds
                    .iter()
                    .any(|r| r.n == *round && r.claim == Some(line.n));
                if won {
                    self.claim(line, *round);
                }
            }
            Op::Withdraw { to } => {
                let taken = index
                    .rounds
                    .iter()
                    .any(|r| r.n == *to && r.withdrawn == Some(line.n));
                match self.rounds.iter_mut().find(|r| r.n == *to) {
                    Some(r) if taken => r.withdrawn = true,
                    _ => warnings.push(format!(
                        "line {}: withdraw of {to} ignored: a claim named it, or it is no open round",
                        line.n
                    )),
                }
            }
            Op::Yield { turn, release } => match (line.by.kind, *release, *turn) {
                (ByKind::Human, true, Some(turn)) => self.release(line.n, turn),
                (ByKind::Human, ..) => {}
                (_, _, turn) => self.end(line.n, turn, name),
            },
            // A checkout or pull is nobody's turn
            Op::External { cause, .. } if cause.as_deref() != Some("git") => {
                if let Some(t) = self.turns.iter().rev().find(|t| t.open()) {
                    self.writes.insert(line.n, t.n);
                }
            }
            Op::Reply { late, .. } if line.by.kind == ByKind::Agent => {
                if let Some(t) = self.author_turn(name, *late) {
                    self.replies.insert(line.n, t);
                }
            }
            Op::Done { to, turn, late } if line.by.kind == ByKind::Agent => {
                if self.counts_for(*turn, name, *late) {
                    self.dones.entry(*to).or_default().push(*turn);
                }
            }
            Op::Comment { turn, .. } if line.by.kind == ByKind::Agent => {
                let turn = turn
                    .filter(|t| self.counts_for(*t, name, false))
                    .or_else(|| self.author_turn(name, false));
                if let Some(t) = turn {
                    self.comments.insert(line.n, t);
                }
            }
            Op::TurnNote { body, turn, late } if self.counts_for(*turn, name, *late) => {
                self.notes.insert(*turn, body.clone());
            }
            Op::Unasked {
                turn, quote, late, ..
            } if self.counts_for(*turn, name, *late) => {
                self.unasked.entry(*turn).or_default().push(quote.clone());
            }
            Op::Covers {
                to,
                quote,
                turn,
                late,
            } if self.counts_for(*turn, name, *late) => {
                self.covers
                    .entry(*turn)
                    .or_default()
                    .push((*to, quote.clone()));
            }
            _ => {}
        }
    }

    fn push_round(
        &mut self,
        line: &Line,
        index: &TurnIndex,
        upto: u32,
        to: Option<To>,
        batch: Option<u64>,
        rev: Option<Rev>,
    ) {
        let Some(r) = index.rounds.iter().rev().find(|r| r.n == line.n) else {
            return;
        };
        self.rounds.push(RoundRec {
            n: line.n,
            kind: r.kind,
            ts: line.ts,
            due: r.not_before,
            by: line.by.name.clone(),
            to,
            batch,
            upto,
            rev,
            withdrawn: false,
            taker: None,
            released: Vec::new(),
            approve_claim: None,
        });
    }

    /// A claim that won its round. A claim of a submit takes every earlier round nobody holds.
    fn claim(&mut self, line: &Line, round: u32) {
        let Some(named) = self.rounds.iter_mut().find(|r| r.n == round) else {
            return;
        };
        if named.kind == RoundKind::Approve {
            named.approve_claim = Some(line.n);
            return;
        }
        for r in &mut self.rounds {
            if r.kind == RoundKind::Submit && r.n <= round && !r.withdrawn && r.taker.is_none() {
                r.taker = Some(line.n);
            }
        }
        self.turns.push(Turn {
            n: line.n,
            ts: line.ts.unwrap_or(0),
            agent: line.by.name.clone(),
            end: None,
            released: None,
            late_end: None,
        });
    }

    /// The human takes back what the open claim `turn` holds.
    fn release(&mut self, n: u32, turn: u32) {
        if let Some(r) = self
            .rounds
            .iter_mut()
            .find(|r| r.approve_claim == Some(turn))
        {
            r.approve_claim = None;
            r.released.push(turn);
            return;
        }
        let Some(t) = self.turns.iter_mut().find(|t| t.n == turn && t.open()) else {
            return;
        };
        t.released = Some(n);
        for r in &mut self.rounds {
            if r.taker == Some(turn) {
                r.taker = None;
                r.released.push(turn);
            }
        }
    }

    /// An agent's yield of `turn`, or of its latest open turn when it names none.
    fn end(&mut self, n: u32, turn: Option<u32>, name: &Option<String>) {
        let found = self.turns.iter_mut().rev().find(|t| {
            t.agent == *name
                && turn.is_none_or(|turn| t.n == turn)
                && (t.open() || (turn.is_some() && t.late()))
        });
        if let Some(t) = found {
            if t.open() {
                t.end = Some(n);
            } else {
                t.late_end = Some(n);
            }
        }
    }

    /// Turn `turn` is `name`'s and still takes its ops. Once the agent yielded, only
    /// ops flagged `late` count, and they never re-open it.
    fn counts_for(&self, turn: u32, name: &Option<String>, late: bool) -> bool {
        self.turn(turn).is_some_and(|t| {
            t.agent == *name && (t.open() || t.late() || (late && t.end.is_some()))
        })
    }

    /// The turn an op by `name` belongs to: its latest open one, or with `late` its
    /// latest released or yielded one.
    fn author_turn(&self, name: &Option<String>, late: bool) -> Option<u32> {
        let mine = || self.turns.iter().rev().filter(|t| t.agent == *name);
        mine()
            .find(|t| t.open())
            .or_else(|| mine().find(|t| late && (t.late() || t.end.is_some())))
            .map(|t| t.n)
    }
}
