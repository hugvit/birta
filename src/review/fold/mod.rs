//! Reading a review log into state. [`scan`] is the cheap pass the write guards and
//! `--wait` use: rounds, claims and withdrawals, with no chain or doc. [`fold`] builds
//! the whole [`ReviewState`].

mod changes;
mod items;
mod texts;
mod turns;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use super::Reject;
use super::chain::Chain;
use super::doc::Doc;
use super::ladder;
use super::ops::{At, ByKind, Line, Op, Parsed, Scope, Verdict};
use super::state::{
    AnchorStatus, ApproveState, AskedKind, AtKind, AtState, Attr, Change, Counts, Hold, HoldKind,
    Item, ItemOutcome, ItemStatus, Loc, LogInfo, OwnerState, Phrase, PickedUp, Replied, ReplyState,
    ReviewState, Round as RoundState, Sent, Silence, Stage,
};
use crate::anchor::{self, StopKind};
use crate::rev::Rev;
use items::{Notes, Outcome, cover};
use texts::{Texts, span_of};
use turns::{RoundRec, Turns};

/// Whether a round view lists `item`: the claim `claim` holds it, or with no claim yet
/// it was sent in one of `rounds`; and, but for a read-only view, a note an earlier turn
/// left open, which the fold carries to the next claim.
pub fn listed(item: &Item, claim: Option<u32>, rounds: &[u32], read_only: bool) -> bool {
    if item.by.kind != ByKind::Human || item.status == ItemStatus::Retracted {
        return false;
    }
    let held = match claim {
        Some(c) => item.turn == Some(c),
        None => item.sent_in.is_some_and(|s| rounds.contains(&s)),
    };
    held || (!read_only && items::carries(item.status))
}

/// Minutes of agent silence after which the human may release a claim, in ms.
const SILENCE_MS: u64 = 900_000;
/// Creation writes the doc's owner on this line; later owner lines count only from the human.
const OWNER_LINE: u32 = 3;
/// Lines of a turn note the phrase keeps.
const NOTE_LINES: usize = 2;

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
    /// The live claim; later claims of the same round lost the race.
    pub claim: Option<u32>,
    pub claimed_by: Option<String>,
    /// The agent's yield that ended the live claim's turn.
    pub yielded: Option<u32>,
    /// A claim of it was released, so the human took it back from an agent once.
    pub released: bool,
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
        index.step(line);
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
    /// Takes in the next line of the log.
    pub(crate) fn step(&mut self, line: &Line) {
        let round = |kind, not_before, upto| Round {
            n: line.n,
            kind,
            not_before: due_at(line.ts, not_before),
            upto,
            claim: None,
            claimed_by: None,
            yielded: None,
            released: false,
            withdrawn: None,
        };
        match &line.op {
            Op::Submit {
                upto, not_before, ..
            } => self
                .rounds
                .push(round(RoundKind::Submit, *not_before, *upto)),
            Op::Approve { not_before, .. } => {
                self.rounds
                    .push(round(RoundKind::Approve, *not_before, line.n));
            }
            Op::Claim { round } => {
                if let Some(r) = self.open_round(*round) {
                    r.claim = Some(line.n);
                    r.claimed_by.clone_from(&line.by.name);
                }
            }
            Op::Withdraw { to } => {
                if let Some(r) = self.open_round(*to).filter(|r| !r.released) {
                    r.withdrawn = Some(line.n);
                }
            }
            Op::Yield { turn, release } => match (line.by.kind, *release, *turn) {
                (ByKind::Human, true, Some(turn)) => {
                    if let Some(r) = self.live_turn(turn) {
                        r.claim = None;
                        r.claimed_by = None;
                        r.released = true;
                    }
                }
                (ByKind::Human, ..) => {}
                (_, _, turn) => {
                    let name = &line.by.name;
                    let ended = self.rounds.iter_mut().rev().find(|r| {
                        r.claim.is_some()
                            && r.yielded.is_none()
                            && r.claimed_by == *name
                            && turn.is_none_or(|turn| r.claim == Some(turn))
                    });
                    if let Some(r) = ended {
                        r.yielded = Some(line.n);
                    }
                }
            },
            Op::Comment { .. } | Op::Reply { .. } | Op::Amend { .. } | Op::Retract { .. }
                if line.by.kind == ByKind::Human =>
            {
                self.notes.push(line.n);
            }
            _ => {}
        }
    }

    /// Whether claim line `n` of `round`, just stepped, won a hand-back: that opens an
    /// agent's turn. An approve's claim asks nothing of the agent.
    pub(crate) fn opens_turn(&self, round: u32, n: u32) -> bool {
        self.rounds
            .binary_search_by_key(&round, |r| r.n)
            .is_ok_and(|i| {
                self.rounds[i].claim == Some(n) && self.rounds[i].kind == RoundKind::Submit
            })
    }

    /// The round whose live claim is `turn`, while the agent hasn't yielded it.
    fn live_turn(&mut self, turn: u32) -> Option<&mut Round> {
        self.rounds
            .iter_mut()
            .find(|r| r.claim == Some(turn) && r.yielded.is_none())
    }

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

    /// A round can be withdrawn until a claim names it, even one released since.
    pub fn check_withdraw(&self, to: u32) -> Result<(), Reject> {
        match self.rounds.iter().find(|r| r.n == to) {
            Some(r) if r.claim.is_none() && !r.released && r.withdrawn.is_none() => Ok(()),
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

    /// Rounds neither withdrawn nor taken by a claim of theirs or of a later round.
    fn pending(&self) -> impl DoubleEndedIterator<Item = (usize, &Round)> {
        let after = self
            .rounds
            .iter()
            .rposition(|r| r.claim.is_some())
            .map_or(0, |i| i + 1);
        self.rounds
            .iter()
            .enumerate()
            .skip(after)
            .filter(|(_, r)| r.withdrawn.is_none())
    }

    /// A round nothing has claimed or withdrawn yet, so the first such op decides it.
    fn open_round(&mut self, n: u32) -> Option<&mut Round> {
        // Rounds are in log order
        let i = self.rounds.binary_search_by_key(&n, |r| r.n).ok()?;
        Some(&mut self.rounds[i]).filter(|r| r.claim.is_none() && r.withdrawn.is_none())
    }
}

/// A folded review log, and what the CLI and server ask of it beyond the state.
pub struct Folded<'a> {
    pub state: ReviewState,
    index: TurnIndex,
    chain: &'a Chain,
    doc: &'a Doc,
    /// Items each hand-back or approve covers, by its line.
    covered: HashMap<u32, Vec<usize>>,
}

/// How the current text ends its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
    Cr,
    Mixed,
    /// A single line with no terminator.
    None,
}

/// The current text's rev and the byte habits a writer must keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadInfo {
    pub rev: Rev,
    pub bom: bool,
    pub eol: Eol,
}

impl Folded<'_> {
    /// The latest round that is due at `now_ms` and still unclaimed, as [`scan`] says.
    pub fn claimable_round(&self, now_ms: u64) -> Option<u32> {
        self.index.claimable_round(now_ms)
    }

    /// The hand-back or approve on line `n`.
    pub fn round(&self, n: u32) -> Option<&Round> {
        self.index.rounds.iter().find(|r| r.n == n)
    }

    /// The human notes round `round` covers, retracted ones aside.
    pub fn notes_covered_by(&self, round: u32) -> Vec<&Item> {
        self.covered
            .get(&round)
            .into_iter()
            .flatten()
            .map(|&i| &self.state.items[i])
            .collect()
    }

    pub fn text_at(&self, rev: &Rev) -> Option<Arc<str>> {
        if *rev == self.doc.rev {
            return Some(self.doc.text.clone());
        }
        self.chain.text(rev)
    }

    /// The bytes of `rev` as they were on disk, a BOM included.
    pub fn text_bytes_at(&self, rev: &Rev) -> Option<Vec<u8>> {
        self.text_at(rev).map(|text| text.as_bytes().to_vec())
    }

    pub fn head_info(&self) -> HeadInfo {
        let text = &self.doc.text;
        let bytes = text.as_bytes();
        let (mut lf, mut crlf, mut cr) = (false, false, false);
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                    crlf = true;
                    i += 1;
                }
                b'\r' => cr = true,
                b'\n' => lf = true,
                _ => {}
            }
            i += 1;
        }
        let eol = match (lf, crlf, cr) {
            (false, false, false) => Eol::None,
            (true, false, false) => Eol::Lf,
            (false, true, false) => Eol::Crlf,
            (false, false, true) => Eol::Cr,
            _ => Eol::Mixed,
        };
        HeadInfo {
            rev: self.doc.rev.clone(),
            bom: text.starts_with('\u{feff}'),
            eol,
        }
    }
}

/// The line the human copies to point an agent at the notes on `doc`.
pub fn hand_off(doc: &Path) -> String {
    format!("Take a look at my notes: `birta review {}`", doc.display())
}

/// Folds `parsed` into the state of `doc`, its current text. Pure and clock-free: the
/// caller fills `server_now`, `writable`, `log_writable` and `hand_off`.
pub fn fold<'a>(parsed: &Parsed, chain: &'a Chain, doc: &'a Doc) -> Folded<'a> {
    let mut warnings: Vec<String> = parsed
        .unknown
        .iter()
        .map(|u| format!("line {}: unknown op {:?}", u.n, u.op))
        .collect();
    let mut index = TurnIndex::default();
    let mut turns = Turns::default();
    let mut notes = Notes::default();
    let mut drafted_by = None;
    let mut seen = HashSet::new();
    let mut seen_at: HashMap<String, u32> = HashMap::new();
    let mut seen_last: HashMap<String, u32> = HashMap::new();
    // The last agent line about each note, which a note's own seen must come after
    let mut news_at: HashMap<u32, u32> = HashMap::new();
    for line in &parsed.lines {
        notes.step(line, &index, &mut warnings);
        index.step(line);
        turns.step(line, &index, &mut warnings);
        match &line.op {
            Op::Owner { drafted_by: owner } => {
                if line.n == OWNER_LINE || line.by.kind == ByKind::Human {
                    drafted_by.clone_from(owner);
                } else {
                    warnings.push(format!(
                        "line {}: owner ignored: only the human changes it",
                        line.n
                    ));
                }
            }
            Op::Seen { ids } if line.by.kind == ByKind::Human => {
                for id in ids {
                    seen.insert(id.clone());
                    seen_at.entry(id.clone()).or_insert(line.n);
                    seen_last.insert(id.clone(), line.n);
                }
            }
            Op::Done { to, .. } | Op::Covers { to, .. } | Op::Reply { to, .. }
                if line.by.kind == ByKind::Agent =>
            {
                news_at.insert(*to, line.n);
            }
            _ => {}
        }
    }

    let lines: HashMap<u32, &Line> = parsed.lines.iter().map(|l| (l.n, l)).collect();
    let mut texts = Texts::new(chain, doc);
    let froms: Vec<Rev> = turns
        .writes
        .keys()
        .filter_map(|&n| texts.step_at(n))
        .map(|(_, step)| step.from.clone())
        .collect();
    texts.will_ask(froms);
    texts.prefetch_bases(
        notes
            .notes
            .iter()
            .filter_map(|note| Some((note.n, note.anchor()?))),
    );
    // A bound `covers` settles its note, so the carry below must already see it
    let bound = changes::covers_bound(&parsed.lines, &turns, &texts);
    let mut facts: Vec<Facts> = notes
        .notes
        .iter()
        .map(|note| {
            let covered = |held_by: &[u32]| {
                bound
                    .get(&note.n)
                    .is_some_and(|turns| turns.iter().any(|t| held_by.contains(t)))
            };
            let round = (note.human() && !note.retracted)
                .then(|| cover(&turns.rounds, note.n, None))
                .flatten();
            let submit = round.filter(|r| r.kind == RoundKind::Submit);
            let mut held_by: Vec<u32> = submit
                .map(|r| {
                    r.taker
                        .into_iter()
                        .chain(r.released.iter().copied())
                        .collect()
                })
                .unwrap_or_default();
            let mut holder = submit.and_then(|r| r.taker);
            let mut out = if held_by.is_empty() {
                Outcome::default()
            } else {
                items::outcome(note, &held_by, &turns, &mut texts)
            };
            out.covered = covered(&held_by);
            let mut status = items::status(note, round, holder, &turns, &out);
            while let Some(next) = items::carried_to(&turns, holder, status) {
                held_by.push(next);
                holder = Some(next);
                out = items::outcome(note, &held_by, &turns, &mut texts);
                out.covered = covered(&held_by);
                status = items::status(note, round, holder, &turns, &out);
            }
            Facts {
                status,
                sent_in: submit.map(|r| r.n),
                turn: holder,
                sent_by: round.map(|r| r.n),
                held_by,
                out,
            }
        })
        .collect();
    // Texts are read only where the turn wrote near a note it marked done
    texts.prefetch(
        facts
            .iter()
            .filter_map(|f| f.out.compare.clone())
            .flat_map(|(from, to)| [from, to]),
    );
    for (note, f) in notes.notes.iter().zip(&mut facts) {
        if let Some(pair) = f.out.compare.take() {
            f.out.no_change = items::same_text(note, &pair, &mut texts);
        }
    }

    let mut held: HashMap<u32, Vec<u32>> = HashMap::new();
    for (note, f) in notes.notes.iter().zip(&facts) {
        for &turn in &f.held_by {
            held.entry(turn).or_default().push(note.n);
        }
    }
    let stets: Vec<(u32, u32)> = notes
        .notes
        .iter()
        .zip(&facts)
        .filter(|(note, _)| note.human() && !note.retracted && note.verdict == Some(Verdict::Stet))
        .filter_map(|(note, f)| Some((note.n, f.sent_by?)))
        .collect();
    let standing: Vec<u32> = turns
        .rounds
        .iter()
        .filter(|r| !r.withdrawn)
        .map(|r| r.n)
        .collect();
    let built = changes::build(
        &changes::Input {
            lines: &parsed.lines,
            turns: &turns,
            notes: &notes,
            held: &held,
            stets: &stets,
            seen: &seen_at,
            rounds: &standing,
        },
        &mut texts,
    );
    let locator = anchor::Locator::new(&doc.text);
    let items: Vec<Item> = notes
        .notes
        .iter()
        .zip(&facts)
        .map(|(note, f)| {
            let mut it = item(note, f, &seen, at_state(note, &mut texts, &locator));
            it.seen = seen_last
                .get(&format!("#{}", note.n))
                .is_some_and(|&s| s > news_at.get(&note.n).copied().unwrap_or(0));
            it
        })
        .collect();
    let fold = Fold {
        turns: &turns,
        notes: &notes,
        facts: &facts,
        lines: &lines,
        changes: &built.changes,
    };
    let count = |status| facts.iter().filter(|f| f.status == status).count() as u32;
    let later: Vec<u32> = items
        .iter()
        .filter(|i| i.status == ItemStatus::Later)
        .map(|i| i.n)
        .collect();
    let orphaned = |note: &items::Note, i: &Item| {
        !note.retracted && i.at.status == Some(AnchorStatus::Orphaned)
    };
    // The texts at the end of the turns that may have taken them, in one pass
    texts.prefetch(
        notes
            .notes
            .iter()
            .zip(&items)
            .filter(|(note, i)| orphaned(note, i) && i.status == ItemStatus::Handled)
            .filter_map(|(_, i)| chain.rev_at_line(turns.turn(i.turn?)?.close()).cloned()),
    );
    let lost: Vec<u32> = notes
        .notes
        .iter()
        .zip(&items)
        .filter(|(note, i)| {
            orphaned(note, i)
                && !(i.status == ItemStatus::Handled && lost_in_turn(note, i, &turns, &mut texts))
        })
        .map(|(note, _)| note.n)
        .collect();
    let unseen = built
        .changes
        .iter()
        .filter(|c| c.attr != Attr::Human && !c.seen && !c.settled)
        .count() as u32;
    let attention = items
        .iter()
        .map(|i| {
            let replies = i
                .replies
                .iter()
                .filter(|r| r.by.kind == ByKind::Agent && !r.seen)
                .count();
            let open = i.status == ItemStatus::Open && i.verdict == Some(Verdict::Ask);
            let unseen_nh = i.id.as_ref().is_some_and(|id| !seen.contains(id));
            replies + usize::from(open) + usize::from(unseen_nh)
        })
        .sum::<usize>() as u32
        + unseen;
    let counts = Counts {
        drafts: fold.drafts(None),
        sent_unclaimed: count(ItemStatus::Sent),
        unseen,
        unasked: built
            .changes
            .iter()
            .filter(|c| c.asked.kind == AskedKind::Unasked && !c.settled)
            .count() as u32,
        not_handled: count(ItemStatus::NotHandled),
        open_asks: items
            .iter()
            .filter(|i| i.status == ItemStatus::Open && i.verdict == Some(Verdict::Ask))
            .count() as u32,
        later: later.len() as u32,
        lost: lost.len() as u32,
        items: item_stops(doc).count() as u32,
        decided: decided(&notes, doc, &mut texts),
        attention,
    };

    let latest = turns.rounds.iter().rev().find(|r| !r.withdrawn);
    let pending = latest.filter(|r| r.unclaimed() && r.ts.is_some_and(|ts| r.due > ts));
    let hold = pending.map(|r| Hold {
        n: r.n,
        kind: match r.kind {
            RoundKind::Submit => HoldKind::HandBack,
            RoundKind::Approve => HoldKind::Approve,
        },
        count: fold.sent_by(r.n, None),
        not_before: r.due,
    });
    let approve = latest
        .filter(|r| r.kind == RoundKind::Approve)
        .and_then(|r| {
            let rev = r.rev.as_ref()?;
            let current_matches = *rev == doc.rev;
            Some(ApproveState {
                n: r.n,
                ts: r.due,
                rev: rev.as_str().to_string(),
                current_matches,
                changed_since: !current_matches,
                later: later.len() as u32,
                can_withdraw: r.unclaimed(),
            })
        });
    let rounds = turns
        .rounds
        .iter()
        .filter(|r| r.kind == RoundKind::Submit && !r.withdrawn)
        .map(|r| {
            let taker = r.taker.and_then(|t| turns.turn(t));
            RoundState {
                submit: r.n,
                ts: r.due,
                by: r.by.clone().unwrap_or_default(),
                to: r.to.clone(),
                batch: r.batch,
                items: fold.sent_items(r.n, None).collect(),
                claim: r.taker,
                claimed_by: taker.and_then(|t| t.agent.clone()),
                r#yield: taker.and_then(|t| t.end),
                released: !r.released.is_empty(),
            }
        })
        .collect();

    let mut covered: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, f) in facts.iter().enumerate() {
        if let Some(round) = f.sent_by {
            covered.entry(round).or_default().push(i);
        }
    }
    let state = ReviewState {
        schema: super::SCHEMA_STATE.to_string(),
        doc: parsed
            .header
            .as_ref()
            .map(|h| h.doc.clone())
            .unwrap_or_default(),
        rev: doc.rev.as_str().to_string(),
        server_now: 0,
        writable: true,
        log_writable: true,
        hand_off: String::new(),
        log: log_info(parsed, chain),
        owner: OwnerState { drafted_by },
        phrase: fold.phrase(None),
        phrase_before_hold: pending.map(|r| fold.phrase(Some(r.n))),
        hold,
        counts,
        items,
        changes: built.changes,
        deletions: built.deletions,
        rewrite: built.rewrite,
        later,
        lost,
        approve,
        rounds,
        warnings,
    };
    Folded {
        state,
        index,
        chain,
        doc,
        covered,
    }
}

fn log_info(parsed: &Parsed, chain: &Chain) -> LogInfo {
    let last_line = parsed
        .lines
        .iter()
        .map(|l| l.n)
        .chain(parsed.bad.iter().map(|b| b.n))
        .chain(parsed.unknown.iter().map(|u| u.n))
        .chain(parsed.header.as_ref().map(|_| 1))
        .max()
        .unwrap_or(0);
    LogInfo {
        lines: last_line,
        bad: parsed.bad.len() as u32,
        unknown_ops: parsed.unknown.len() as u32,
        read_only: parsed.read_only,
        baseline_available: chain.head().is_some(),
        baseline_from: None,
    }
}

/// Folds a folder's review file, whose notes are on the folder: the same walk with no
/// doc, so no changes and no items to decide.
pub fn fold_dir(parsed: &Parsed) -> ReviewState {
    let mut notes_only = parsed.clone();
    notes_only.lines.retain(|l| {
        !matches!(
            l.op,
            Op::Snap { .. } | Op::Edit { .. } | Op::External { .. }
        )
    });
    let chain = Chain::build(&notes_only);
    let doc = Doc::new("");
    let mut state = fold(&notes_only, &chain, &doc).state;
    state.rev = String::new();
    state.log = log_info(parsed, &chain);
    state
}

/// Whether a handled note was already gone when its own turn ended, so the change that
/// handled it took its text.
fn lost_in_turn(note: &items::Note, item: &Item, turns: &Turns, texts: &mut Texts) -> bool {
    let (Some(anchor), Some(turn)) = (note.anchor(), item.turn.and_then(|t| turns.turn(t))) else {
        return false;
    };
    let chain = texts.chain;
    let Some(rev) = chain.rev_at_line(turn.close()).cloned() else {
        return false;
    };
    texts
        .with_doc(&rev, |doc| {
            ladder::relocate(anchor, doc, chain).status == AnchorStatus::Orphaned
        })
        .unwrap_or(false)
}

/// Where a note is on the current text, and the place the page paints for it.
fn at_state(note: &items::Note, texts: &mut Texts, locator: &anchor::Locator) -> AtState {
    let anchor = match &note.at {
        At::Doc | At::Dir => {
            return AtState {
                kind: if matches!(note.at, At::Doc) {
                    AtKind::Doc
                } else {
                    AtKind::Dir
                },
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
            };
        }
        At::Anchor(anchor) => anchor,
    };
    let head = texts.head;
    let located = texts.located(note.n, anchor).clone();
    let loc = match (anchor.scope, &located.bytes, located.lines) {
        (Scope::Quote, Some(bytes), _) => locator.locate(bytes.clone()).map(|l| Loc {
            sp: l.sp,
            off: Some(l.off),
            tl: Some(l.tl),
        }),
        (Scope::Block | Scope::Section, _, Some((first, _))) => head
            .blocks
            .stops
            .iter()
            .find(|s| s.lines.0 == first)
            .map(|s| Loc {
                sp: s.sp.clone(),
                off: None,
                tl: None,
            }),
        _ => None,
    };
    AtState {
        kind: AtKind::Anchor,
        status: Some(located.status),
        scope: Some(anchor.scope),
        quote: Some(anchor.quote.clone()),
        shown: anchor.shown.clone(),
        lines: located.lines,
        range: located.bytes.map(|b| (b.start, b.end)),
        block: located.block,
        section: located.section,
        rev: Some(anchor.rev.as_str().to_string()),
        loc,
    }
}

/// What the fold worked out for one note.
struct Facts {
    status: ItemStatus,
    sent_in: Option<u32>,
    turn: Option<u32>,
    /// The hand-back or approve that covers it.
    sent_by: Option<u32>,
    /// Claims that held it: the live one, and any released.
    held_by: Vec<u32>,
    out: Outcome,
}

struct Fold<'a> {
    turns: &'a Turns,
    notes: &'a Notes,
    facts: &'a [Facts],
    lines: &'a HashMap<u32, &'a Line>,
    changes: &'a [Change],
}

impl Fold<'_> {
    /// Human notes and replies no hand-back covers, as if round `ignore` were absent.
    fn drafts(&self, ignore: Option<u32>) -> u32 {
        let rounds = &self.turns.rounds;
        let notes = self
            .notes
            .notes
            .iter()
            .filter(|n| n.human() && !n.retracted && cover(rounds, n.n, ignore).is_none())
            .count();
        let replies = self
            .notes
            .human_replies
            .iter()
            .filter(|&&n| cover(rounds, n, ignore).is_none())
            .count();
        (notes + replies) as u32
    }

    /// The human notes round `round` handed back.
    fn sent_items(&self, round: u32, ignore: Option<u32>) -> impl Iterator<Item = u32> {
        self.notes
            .notes
            .iter()
            .filter(move |n| {
                n.human()
                    && !n.retracted
                    && cover(&self.turns.rounds, n.n, ignore).is_some_and(|r| r.n == round)
            })
            .map(|n| n.n)
    }

    fn sent_by(&self, round: u32, ignore: Option<u32>) -> u32 {
        self.sent_items(round, ignore).count() as u32
    }

    fn ts(&self, n: u32) -> u64 {
        self.lines.get(&n).and_then(|l| l.ts).unwrap_or(0)
    }

    /// The phrase facts, as if round `ignore` were absent.
    fn phrase(&self, ignore: Option<u32>) -> Phrase {
        let live = |r: &&RoundRec| !r.withdrawn && Some(r.n) != ignore;
        let rounds = &self.turns.rounds;
        let latest = rounds.iter().rev().find(live);
        let submit = rounds
            .iter()
            .rev()
            .filter(live)
            .find(|r| r.kind == RoundKind::Submit);
        let turn = submit
            .and_then(|r| r.taker)
            .and_then(|t| self.turns.turn(t));
        let stage = match latest {
            None if self.drafts(ignore) > 0 => Stage::Drafts,
            None => Stage::Idle,
            Some(r) if r.kind == RoundKind::Approve => Stage::Approved,
            Some(_) => match turn {
                None => Stage::Sent,
                Some(t) if t.open() => Stage::PickedUp,
                Some(_) => Stage::Replied,
            },
        };
        Phrase {
            stage,
            sent: submit.map(|r| Sent {
                n: r.n,
                ts: r.due,
                items: self.sent_by(r.n, ignore),
                can_withdraw: r.unclaimed(),
            }),
            picked_up: turn.map(|t| PickedUp {
                n: t.n,
                ts: t.ts,
                by: t.agent.clone().unwrap_or_default(),
                silence: Silence {
                    since_ts: t.ts,
                    threshold_ms: SILENCE_MS,
                },
            }),
            replied: turn.and_then(|t| {
                let end = t.end?;
                let mine = || {
                    self.notes
                        .notes
                        .iter()
                        .zip(self.facts)
                        .filter(|(_, f)| f.turn == Some(t.n))
                };
                let tally =
                    |pick: fn(&Facts) -> bool| mine().filter(|(_, f)| pick(f)).count() as u32;
                Some(Replied {
                    turn: t.n,
                    ts: self.ts(end),
                    by: t.agent.clone().unwrap_or_default(),
                    done: tally(|f| f.out.done),
                    answered: tally(|f| f.out.answered),
                    didnt: tally(|f| f.out.didnt),
                    asked: self
                        .turns
                        .comments
                        .iter()
                        .filter(|(n, turn)| {
                            **turn == t.n && self.notes.get(**n).is_some_and(|n| n.blank())
                        })
                        .count() as u32,
                    unasked: self
                        .changes
                        .iter()
                        .filter(|c| c.turn == Some(t.n) && c.asked.kind == AskedKind::Unasked)
                        .count() as u32,
                    not_handled: tally(|f| f.status == ItemStatus::NotHandled),
                    note: self
                        .turns
                        .notes
                        .get(&t.n)
                        .map(|body| body.lines().take(NOTE_LINES).collect::<Vec<_>>().join("\n")),
                })
            }),
        }
    }
}

fn item(note: &items::Note, f: &Facts, seen: &HashSet<String>, at: AtState) -> Item {
    let out = &f.out;
    Item {
        n: note.n,
        id: (f.status == ItemStatus::NotHandled).then(|| format!("nh:{}", note.n)),
        op: "comment".to_string(),
        by: note.by.clone(),
        cid: note.cid.clone(),
        ts: note.ts,
        at,
        verdict: note.verdict,
        body: note.body.clone(),
        to: note.to,
        kind: note.kind,
        status: f.status,
        sent_in: f.sent_in,
        turn: f.turn,
        amended: note.amended.clone(),
        replies: note
            .replies
            .iter()
            .map(|r| {
                let id = format!("#{}", r.n);
                ReplyState {
                    n: r.n,
                    seen: seen.contains(&id),
                    id,
                    by: r.by.clone(),
                    kind: r.kind,
                    verdict: r.verdict,
                    body: r.body.clone(),
                    ts: r.ts,
                }
            })
            .collect(),
        outcome: (out.done || !out.changes.is_empty()).then(|| ItemOutcome {
            done: out.done,
            no_change: out.done && out.no_change,
            changes: out.changes.clone(),
        }),
        seen: false,
    }
}

fn item_stops(doc: &Doc) -> impl Iterator<Item = &crate::anchor::Stop> {
    doc.blocks
        .stops
        .iter()
        .filter(|s| matches!(s.kind, StopKind::ListItem | StopKind::TableRow))
}

/// Item stops a live human verdict covers: on the item, or on an item or heading
/// section holding it. `ask` is a question, not a decision.
fn decided(notes: &Notes, doc: &Doc, texts: &mut Texts) -> u32 {
    let spans: Vec<(u32, u32)> = notes
        .notes
        .iter()
        .filter(|n| n.human() && !n.retracted)
        .filter(|n| n.verdict.is_some_and(|v| v != Verdict::Ask))
        .filter_map(|n| {
            let anchor = n.anchor()?;
            span_of(anchor, texts.located(n.n, anchor))
        })
        .collect();
    item_stops(doc)
        .filter(|s| {
            spans
                .iter()
                .any(|&(first, last)| first <= s.lines.0 && s.lines.1 <= last)
        })
        .count() as u32
}
