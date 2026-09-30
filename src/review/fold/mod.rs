//! Reading a review log into state. [`scan`] is the cheap pass the write guards and
//! `--wait` use: rounds, claims and withdrawals, with no chain or doc.

use super::Reject;
use super::ops::{ByKind, Op, Parsed};

/// A `not_before` further than this past its line's `ts` is a bad clock or a hand edit.
const FAR_FUTURE_MS: u64 = 60_000;
/// What such a `not_before` is clamped to, after its `ts`.
const CLAMPED_HOLD_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundKind {
    Submit,
    Approve,
}

/// A hand-back or approve, and what became of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Round {
    pub n: u32,
    pub kind: RoundKind,
    /// When it may be claimed, after the far-future clamp.
    pub not_before: u64,
    /// The last human op line it covers.
    pub upto: u32,
    /// The claim that took it; later claims of the same round lost the race.
    pub claim: Option<u32>,
    /// The withdraw that took it back before any claim.
    pub withdrawn: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct TurnIndex {
    pub rounds: Vec<Round>,
    /// Lines of human notes, which a hand-back sends.
    notes: Vec<u32>,
}

/// Indexes the rounds of `parsed`. A round's due time never depends on the reader's
/// clock, so time passing can't make a due round wait again.
pub fn scan(parsed: &Parsed, _now_ms: u64) -> TurnIndex {
    let mut index = TurnIndex::default();
    for line in &parsed.lines {
        let round = |kind, not_before, upto| Round {
            n: line.n,
            kind,
            not_before: due_at(line.ts, not_before),
            upto,
            claim: None,
            withdrawn: None,
        };
        match &line.op {
            Op::Submit {
                upto, not_before, ..
            } => index
                .rounds
                .push(round(RoundKind::Submit, *not_before, *upto)),
            Op::Approve { not_before, .. } => {
                index
                    .rounds
                    .push(round(RoundKind::Approve, *not_before, line.n));
            }
            Op::Claim { round } => {
                if let Some(r) = index.open_round(*round) {
                    r.claim = Some(line.n);
                }
            }
            Op::Withdraw { to } => {
                if let Some(r) = index.open_round(*to) {
                    r.withdrawn = Some(line.n);
                }
            }
            Op::Comment { .. } | Op::Reply { .. } | Op::Amend { .. } | Op::Retract { .. }
                if line.by.kind == ByKind::Human =>
            {
                index.notes.push(line.n);
            }
            _ => {}
        }
    }
    index
}

/// `not_before`, clamped when it is too far past `ts`. A hand-written line with no `ts`
/// has no hold.
fn due_at(ts: Option<u64>, not_before: u64) -> u64 {
    match ts {
        Some(ts) if not_before > ts.saturating_add(FAR_FUTURE_MS) => {
            ts.saturating_add(CLAMPED_HOLD_MS)
        }
        Some(_) => not_before,
        None => 0,
    }
}

impl TurnIndex {
    /// The latest round that is due and still unclaimed and unwithdrawn.
    pub fn claimable_round(&self, now_ms: u64) -> Option<u32> {
        self.pending()
            .filter(|(_, r)| now_ms >= r.not_before)
            .map(|(_, r)| r.n)
            .next_back()
    }

    /// The earliest time an unclaimed, unwithdrawn round becomes claimable.
    pub fn pending_deadline(&self) -> Option<u64> {
        self.pending().map(|(_, r)| r.not_before).min()
    }

    /// The claim CAS: `round` must be a live, due round nobody has claimed.
    pub fn check_claim(&self, round: u32, now_ms: u64) -> Result<(), Reject> {
        let Some(i) = self.rounds.iter().position(|r| r.n == round) else {
            return Err(Reject::Invalid(format!("line {round} is not a hand-back")));
        };
        let r = &self.rounds[i];
        if r.withdrawn.is_some() {
            return Err(Reject::Invalid(format!("round {round} was withdrawn")));
        }
        if self.claimed(i) {
            return Err(Reject::AlreadyClaimed);
        }
        if now_ms < r.not_before {
            return Err(Reject::NotYet);
        }
        Ok(())
    }

    /// A round can be withdrawn until a claim names it.
    pub fn check_withdraw(&self, to: u32) -> Result<(), Reject> {
        match self.rounds.iter().find(|r| r.n == to) {
            Some(r) if r.claim.is_none() && r.withdrawn.is_none() => Ok(()),
            _ => Err(Reject::NotWithdrawable),
        }
    }

    /// A hand-back of notes up to line `upto`: refused while another is in its hold,
    /// or when every note up to `upto` was already sent.
    pub fn check_submit(&self, upto: u32, now_ms: u64) -> Result<(), Reject> {
        let holding = self
            .pending()
            .any(|(_, r)| r.kind == RoundKind::Submit && now_ms < r.not_before);
        if holding {
            return Err(Reject::HoldPending);
        }
        let sent = self
            .rounds
            .iter()
            .filter(|r| r.withdrawn.is_none())
            .map(|r| r.upto)
            .max()
            .unwrap_or(0);
        if self.notes.iter().any(|&n| n > sent && n <= upto) {
            Ok(())
        } else {
            Err(Reject::NothingToSend)
        }
    }

    /// A claim of a later round takes the notes of every round before it.
    fn claimed(&self, i: usize) -> bool {
        self.rounds[i..].iter().any(|r| r.claim.is_some())
    }

    fn pending(&self) -> impl DoubleEndedIterator<Item = (usize, &Round)> {
        self.rounds
            .iter()
            .enumerate()
            .filter(|(i, r)| r.withdrawn.is_none() && !self.claimed(*i))
    }

    /// A round nothing has claimed or withdrawn yet, so the first such op decides it.
    fn open_round(&mut self, n: u32) -> Option<&mut Round> {
        self.rounds
            .iter_mut()
            .find(|r| r.n == n && r.claim.is_none() && r.withdrawn.is_none())
    }
}
