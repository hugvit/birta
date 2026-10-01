//! The doc's history as its review log records it: each rev, its text, and the hunks
//! between revs. Texts are rebuilt by replaying patches from the log's snaps.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::cut;
use super::fold::TurnIndex;
use super::hunk;
use super::ops::{ByKind, Hunk, Op, Parsed};
use crate::rev::Rev;

/// A write whose patch serializes to more bytes than this is stored as a snap.
pub const SNAP_OVER: usize = 256 * 1024;
/// A write where either text has more bytes than this is recorded by rev only.
pub const REV_ONLY_OVER: usize = 1024 * 1024;
/// Every this many patches since the last snap, the write also snaps its text.
pub const CHECKPOINT_EVERY: usize = 20;

/// One recorded change of rev, at log line `n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub n: u32,
    pub from: Rev,
    pub to: Rev,
    /// `None` when the log has no patch and no text to derive one from.
    pub hunks: Option<Vec<Hunk>>,
    pub cause: Cause,
}

/// Who made a step, as far as the log alone can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// birta's own write: an `edit`.
    Edit,
    /// An `external` while an agent held an open claim, unless git made it.
    Agent,
    /// Any other write, including a snap standing in for one.
    Outside { git: bool },
}

/// Where a line of an older text is after some steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapped {
    /// Untouched, at the same line number.
    Same(u32),
    /// Untouched, at a new line number.
    Shifted(u32),
    /// A step changed or removed it.
    Overlapped,
}

/// What to append for a write: the `edit` or `external` fields, and the text of a
/// checkpoint `snap` to follow it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub rev: Rev,
    pub to_rev: Rev,
    pub patch: Option<Vec<Hunk>>,
    pub checkpoint: Option<String>,
}

/// A log op that moves or restores the doc.
enum Link {
    Snap {
        n: u32,
        rev: Rev,
        text: Arc<str>,
    },
    Patch {
        n: u32,
        from: Rev,
        to: Rev,
        hunks: Option<Vec<Hunk>>,
        cause: Cause,
    },
}

pub struct Chain {
    links: Vec<Link>,
    /// Revs whose text some link can rebuild.
    known: HashSet<Rev>,
    walked: Walk,
}

impl Chain {
    /// Never fails: a patch that doesn't apply or verify breaks the chain until a snap.
    pub fn build(parsed: &Parsed) -> Self {
        let mut claims = Claims::default();
        let mut index = TurnIndex::default();
        let links: Vec<Link> = parsed
            .lines
            .iter()
            .filter_map(|line| {
                index.step(line);
                let patch = |rev: &Rev, to: &Rev, hunks: &Option<Vec<Hunk>>, cause| {
                    Some(Link::Patch {
                        n: line.n,
                        from: rev.clone(),
                        to: to.clone(),
                        hunks: hunks.clone(),
                        cause,
                    })
                };
                match &line.op {
                    Op::Snap { rev, text, .. } => Some(Link::Snap {
                        n: line.n,
                        rev: rev.clone(),
                        text: Arc::from(text.as_str()),
                    }),
                    Op::Edit {
                        rev,
                        to_rev,
                        patch: hunks,
                        ..
                    } => patch(rev, to_rev, hunks, Cause::Edit),
                    Op::External {
                        rev,
                        to_rev,
                        patch: hunks,
                        cause,
                        ..
                    } => {
                        // A checkout or pull is never the claimer's, whoever holds a turn
                        let git = cause.as_deref() == Some("git");
                        let cause = if !git && !claims.open.is_empty() {
                            Cause::Agent
                        } else {
                            Cause::Outside { git }
                        };
                        patch(rev, to_rev, hunks, cause)
                    }
                    // Only a claim the fold lets win opens a turn
                    Op::Claim { round } if index.opens_turn(*round, line.n) => {
                        claims.open.push(OpenClaim {
                            n: line.n,
                            name: line.by.name.clone(),
                        });
                        None
                    }
                    Op::Yield { release, turn } => {
                        match (line.by.kind, *release, *turn) {
                            (ByKind::Human, true, Some(turn)) => claims.release(turn),
                            (ByKind::Human, ..) => {}
                            _ => claims.end(*turn, &line.by.name),
                        }
                        None
                    }
                    _ => None,
                }
            })
            .collect();
        let mut known = HashSet::new();
        let walked = Walk::run(&links, true, &mut |rev, _| {
            known.insert(rev.clone());
            false
        });
        Self {
            links,
            known,
            walked,
        }
    }

    pub fn head(&self) -> Option<&Rev> {
        self.walked.revs.last().map(|(_, rev)| rev)
    }

    pub fn text(&self, rev: &Rev) -> Option<Arc<str>> {
        if !self.known.contains(rev) {
            return None;
        }
        if self.head() == Some(rev)
            && let Some(text) = &self.walked.text
        {
            return Some(text.clone());
        }
        let mut found = None;
        Walk::run(&self.links, false, &mut |at, text| {
            if at == rev {
                found = Some(text.clone());
            }
            found.is_some()
        });
        found
    }

    /// The texts of `needed` that the chain has, in one pass over the log.
    pub fn texts(&self, needed: &HashSet<Rev>) -> HashMap<Rev, Arc<str>> {
        let wanted = needed
            .iter()
            .filter(|rev| self.known.contains(*rev))
            .count();
        let mut texts = HashMap::new();
        if wanted > 0 {
            Walk::run(&self.links, false, &mut |rev, text| {
                if needed.contains(rev) {
                    texts.entry(rev.clone()).or_insert_with(|| text.clone());
                }
                texts.len() == wanted
            });
        }
        texts
    }

    /// The doc's rev once every chain op up to log line `n` has happened.
    pub fn rev_at_line(&self, n: u32) -> Option<&Rev> {
        let revs = &self.walked.revs;
        let i = revs.partition_point(|(line, _)| *line <= n);
        i.checked_sub(1).map(|i| &revs[i].1)
    }

    /// The steps from the latest `from` before the latest `to`, if they are unbroken.
    pub fn steps(&self, from: &Rev, to: &Rev) -> Option<&[Step]> {
        let (revs, steps) = (&self.walked.revs, &self.walked.steps);
        let k = revs.iter().rposition(|(_, rev)| rev == to)?;
        let j = revs[..=k].iter().rposition(|(_, rev)| rev == from)?;
        // Every rev but a leading baseline came from a step
        let offset = revs.len() - steps.len();
        let range = j + 1 - offset..k + 1 - offset;
        let unbroken = range.clone().all(|i| !self.walked.broken[i])
            && steps[range.clone()]
                .iter()
                .zip(&revs[j..k])
                .all(|(step, (_, before))| step.from == *before);
        unbroken.then(|| &steps[range])
    }

    /// Every step in log order. Step `i` is trustworthy unless [`Chain::broke`] says so.
    pub fn history(&self) -> &[Step] {
        &self.walked.steps
    }

    /// Step `i` of [`Chain::history`] didn't apply, didn't verify, or skipped a rev.
    pub fn broke(&self, i: usize) -> bool {
        self.walked.broken.get(i).copied().unwrap_or(true)
    }

    /// `Some(n)`: a break left revs recorded before log line `n` without text.
    pub fn broken_before(&self) -> Option<u32> {
        if self.walked.unrecovered {
            let last = self.links.last().map_or(0, |link| link.n());
            Some(last + 1)
        } else {
            self.walked.recovered_at
        }
    }

    /// What to log for a write that leaves the doc as `new`; `None` when there is no
    /// baseline to step from or nothing changed.
    pub fn record(&self, new: &str) -> Option<Recorded> {
        let rev = self.head()?.clone();
        let to_rev = Rev::of(new.as_bytes());
        if to_rev == rev {
            return None;
        }
        let recorded = |patch, checkpoint| {
            Some(Recorded {
                rev,
                to_rev,
                patch,
                checkpoint,
            })
        };
        if new.len() > REV_ONLY_OVER {
            return recorded(None, None);
        }
        let old = self.walked.text.as_deref();
        let Some(old) = old.filter(|old| old.len() <= REV_ONLY_OVER) else {
            return recorded(None, Some(new.to_string()));
        };
        let hunks = cut::diff(old, new);
        let size = serde_json::to_vec(&hunks).map_or(usize::MAX, |json| json.len());
        if size > SNAP_OVER {
            return recorded(None, Some(new.to_string()));
        }
        let checkpoint = self.walked.since_snap + 1 >= CHECKPOINT_EVERY;
        recorded(Some(hunks), checkpoint.then(|| new.to_string()))
    }
}

/// Agents holding a turn, for telling their writes from outside ones.
#[derive(Default)]
struct Claims {
    /// Open turns, oldest first.
    open: Vec<OpenClaim>,
}

struct OpenClaim {
    n: u32,
    name: Option<String>,
}

impl Claims {
    /// The human takes back the round of claim `turn`, which may then be claimed again.
    fn release(&mut self, turn: u32) {
        self.open.retain(|c| c.n != turn);
    }

    /// An agent's yield ends its turn `turn`, or all its turns when it names none.
    fn end(&mut self, turn: Option<u32>, name: &Option<String>) {
        self.open
            .retain(|c| c.name != *name || turn.is_some_and(|turn| turn != c.n));
    }
}

impl Link {
    fn n(&self) -> u32 {
        match self {
            Self::Snap { n, .. } | Self::Patch { n, .. } => *n,
        }
    }
}

/// Where `line` (1-based, in the text before `steps`) is after them.
pub fn map_line(steps: &[Step], line: u32) -> Mapped {
    let mut at = line;
    for step in steps {
        let Some(hunks) = &step.hunks else {
            return Mapped::Overlapped;
        };
        let mut shift = 0i64;
        for hunk in hunks {
            // Hunks are sorted, and `a` is the 0-based line the hunk starts before
            if at <= hunk.a {
                break;
            }
            if at <= hunk.a + hunk.d.len() as u32 {
                return Mapped::Overlapped;
            }
            shift += hunk.i.len() as i64 - hunk.d.len() as i64;
        }
        at = (i64::from(at) + shift) as u32;
    }
    if at == line {
        Mapped::Same(at)
    } else {
        Mapped::Shifted(at)
    }
}

/// One pass over the links, rebuilding each text as the doc reaches it.
#[derive(Default)]
struct Walk {
    /// Each rev the doc reached, with the line of the link that moved it there.
    revs: Vec<(u32, Rev)>,
    steps: Vec<Step>,
    /// Parallel to `steps`: its patch didn't apply or verify, or it skipped a rev.
    broken: Vec<bool>,
    /// The head's text, when known.
    text: Option<Arc<str>>,
    /// Patches since the last snap.
    since_snap: usize,
    /// A break lost the head's text and nothing has supplied it since.
    unrecovered: bool,
    /// The line where the head's text came back after the last break.
    recovered_at: Option<u32>,
    /// Texts of revs the doc reaches more than once, so a broken step to one recovers.
    kept: HashMap<Rev, Arc<str>>,
    recurring: HashSet<Rev>,
    /// A rev-only step the next link may supply the text for, with its old text.
    pending: Option<(usize, Option<Arc<str>>)>,
}

/// Called with each text the walk rebuilds; returning true stops the walk.
type OnText<'a> = dyn FnMut(&Rev, &Arc<str>) -> bool + 'a;

impl Walk {
    /// `derive` fills in the hunks of steps that only have texts.
    fn run(links: &[Link], derive: bool, on_text: &mut OnText) -> Self {
        let mut seen = HashSet::new();
        let mut walk = Self::default();
        for link in links {
            let rev = match link {
                Link::Snap { rev, .. } | Link::Patch { to: rev, .. } => rev,
            };
            if !seen.insert(rev) {
                walk.recurring.insert(rev.clone());
            }
        }
        for link in links {
            let pending = walk.pending.take();
            let stop = match link {
                Link::Snap { n, rev, text } => walk.snap(*n, rev, text, pending, derive, on_text),
                Link::Patch {
                    n,
                    from,
                    to,
                    hunks,
                    cause,
                } => {
                    // The rev-only step before this one never got its text
                    walk.unrecovered |= pending.is_some();
                    walk.patch(*n, (from, to), hunks.as_deref(), *cause, derive, on_text)
                }
            };
            if stop {
                return walk;
            }
        }
        if walk.pending.take().is_some() {
            walk.unrecovered = true;
        }
        walk
    }

    fn snap(
        &mut self,
        n: u32,
        rev: &Rev,
        text: &Arc<str>,
        pending: Option<(usize, Option<Arc<str>>)>,
        derive: bool,
        on_text: &mut OnText,
    ) -> bool {
        self.since_snap = 0;
        let good = (Rev::of(text.as_bytes()) == *rev).then(|| text.clone());
        let Some(head) = self.head().cloned() else {
            self.revs.push((n, rev.clone()));
            if good.is_none() {
                self.unrecovered = true;
            }
            return self.reach(n, good, on_text);
        };
        if head == *rev {
            if self.text.is_some() || good.is_none() {
                self.unrecovered |= pending.is_some();
                return false;
            }
            if let Some((i, Some(old))) = pending.filter(|_| derive) {
                self.steps[i].hunks = Some(hunk::diff(&old, text));
            }
            return self.reach(n, good, on_text);
        }
        // A snap to another rev stands in for the patch between them
        let hunks = match (&self.text, &good) {
            (Some(old), Some(_)) if derive => Some(hunk::diff(old, text)),
            _ => None,
        };
        self.unrecovered |= pending.is_some() || good.is_none();
        let cause = Cause::Outside { git: false };
        self.push_step(n, (head, rev.clone()), hunks, cause, false);
        self.text = None;
        self.reach(n, good, on_text)
    }

    fn patch(
        &mut self,
        n: u32,
        (from, to): (&Rev, &Rev),
        hunks: Option<&[Hunk]>,
        cause: Cause,
        derive: bool,
        on_text: &mut OnText,
    ) -> bool {
        self.since_snap += 1;
        let gap = self.head() != Some(from);
        let (next, broken) = match (hunks, &self.text) {
            _ if gap => (None, true),
            (Some(hunks), Some(old)) => match hunk::apply(old, hunks) {
                Ok(new) if Rev::of(new.as_bytes()) == *to => (Some(Arc::from(new)), false),
                _ => (None, true),
            },
            _ => (None, false),
        };
        self.unrecovered |= broken;
        let next = next.or_else(|| self.kept.get(to).cloned());
        let mut hunks = hunks.map(<[Hunk]>::to_vec);
        if hunks.is_none() && !gap {
            match (&self.text, &next) {
                (Some(old), Some(new)) if derive => hunks = Some(hunk::diff(old, new)),
                (old, None) => self.pending = Some((self.steps.len(), old.clone())),
                _ => {}
            }
        }
        self.push_step(n, (from.clone(), to.clone()), hunks, cause, broken);
        self.text = None;
        self.reach(n, next, on_text)
    }

    fn head(&self) -> Option<&Rev> {
        self.revs.last().map(|(_, rev)| rev)
    }

    fn push_step(
        &mut self,
        n: u32,
        (from, to): (Rev, Rev),
        hunks: Option<Vec<Hunk>>,
        cause: Cause,
        broken: bool,
    ) {
        self.revs.push((n, to.clone()));
        self.steps.push(Step {
            n,
            from,
            to,
            hunks,
            cause,
        });
        self.broken.push(broken);
    }

    /// The head's text is now `text`, if known.
    fn reach(&mut self, n: u32, text: Option<Arc<str>>, on_text: &mut OnText) -> bool {
        let Some(text) = text else {
            return false;
        };
        let Some(head) = self.head().cloned() else {
            return false;
        };
        if self.unrecovered {
            self.unrecovered = false;
            self.recovered_at = Some(n);
        }
        if self.recurring.contains(&head) {
            self.kept.insert(head.clone(), text.clone());
        }
        self.text = Some(text.clone());
        on_text(&head, &text)
    }
}
