//! Claude's news: every change, removal, reply, question, not-handled row and turn note
//! of its, in walk order, until the reviewer has checked it and then handed back or
//! approved. `to_check` counts the stops still holding an unchecked item.

use std::collections::{HashMap, HashSet};

use super::items::{Note, Notes};
use super::turns::Turns;
use crate::review::doc::Doc;
use crate::review::ops::{ByKind, Line, Op, Verdict};
use crate::review::state::{
    AtKind, Attr, Change, Check, Deletion, DeletionKind, Item, ItemStatus, News, NewsKind,
    RangeStatus,
};

pub(super) struct Input<'x> {
    pub doc: &'x Doc,
    pub lines: &'x [Line],
    pub turns: &'x Turns,
    pub notes: &'x Notes,
    pub items: &'x [Item],
    pub lost: &'x [u32],
    pub changes: &'x [Change],
    pub deletions: &'x [Deletion],
    /// Lines of hand-backs and approves that stand.
    pub rounds: &'x [u32],
    /// Lines of approves that stand: each settles all news before it, checked or not.
    pub approves: &'x [u32],
}

pub(super) struct Built {
    pub news: Vec<News>,
    pub to_check: u32,
    /// Each change's check, by id.
    pub checks: HashMap<String, Check>,
    /// The change each note written on it replies to, by note.
    pub replied_to: HashMap<u32, String>,
}

/// Where an item sits in the walk.
#[derive(Clone)]
enum Place {
    Doc,
    Block(String),
    Removal {
        change: String,
        after: Option<String>,
    },
    Question {
        id: String,
        sp: Option<String>,
    },
}

impl Place {
    fn stop(&self) -> String {
        match self {
            Place::Doc => "doc".to_string(),
            Place::Block(sp) => format!("b:{sp}"),
            Place::Removal { change, .. } => format!("r:{change}"),
            Place::Question { id, .. } => format!("q:{id}"),
        }
    }

    /// Walk order: `whole doc` first, then doc order; in one stop's place, its change,
    /// its replies, its questions, its not-handled rows, then the removals in its gap.
    fn order(&self, rank: u8, group: u64, within: u8, line: u32) -> Order {
        let start = |sp: &str| {
            sp.split(':')
                .next()
                .and_then(|l| l.parse().ok())
                .unwrap_or(0)
        };
        match self {
            Place::Doc => (0, 0, rank, group, within, line),
            Place::Block(sp) => (1, start(sp), rank, group, within, line),
            Place::Question { sp: None, .. } => (0, 0, 1, 0, 0, line),
            Place::Question { sp: Some(sp), .. } => (1, start(sp), 2, 0, 0, line),
            Place::Removal { after, .. } => {
                (1, after.as_deref().map_or(0, start), 4, group, within, line)
            }
        }
    }
}

type Order = (u8, u32, u8, u64, u8, u32);

struct Raw {
    id: String,
    /// The line that made it.
    born: u32,
    kind: NewsKind,
    place: Place,
    order: Order,
    /// The lines that checked it, and how.
    events: Vec<(u32, Check)>,
}

impl Raw {
    fn check(&self) -> Option<Check> {
        self.events.iter().map(|(_, c)| *c).max_by_key(|c| rank(*c))
    }

    /// Checked, then handed back or approved.
    fn settled(&self, rounds: &[u32]) -> bool {
        settled(&self.events, rounds)
    }
}

fn settled(events: &[(u32, Check)], rounds: &[u32]) -> bool {
    events
        .iter()
        .map(|(n, _)| *n)
        .min()
        .is_some_and(|first| rounds.iter().any(|&r| r > first))
}

/// A revert outranks a reply, which outranks a keep; a reply never clears a revert.
fn rank(c: Check) -> u8 {
    match c {
        Check::Reverted | Check::Restored => 3,
        Check::Replied => 2,
        Check::Answered => 1,
        Check::Kept | Check::Ok => 0,
    }
}

/// Whether two spans of the current text's lines share a line.
/// Who wrote over a change a later write overlaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Over {
    /// Only Claude, later in the same turn: the later change stands for both.
    Own,
    /// Someone else, or a later turn.
    Other,
}

/// Who wrote over change `c`, when a later write overlaps it.
pub fn overwritten(c: &Change, changes: &[Change]) -> Option<Over> {
    if c.range.status != RangeStatus::Superseded || c.undone_by.is_some() {
        return None;
    }
    let mut writes = changes
        .iter()
        .filter(|x| x.op > c.op && touches(x.span, c.span))
        .peekable();
    writes.peek()?;
    let own = c.attr == Attr::Agent
        && writes.all(|x| x.attr == Attr::Agent && x.turn.is_some() && x.turn == c.turn);
    Some(if own { Over::Own } else { Over::Other })
}

fn touches(a: Option<(u32, u32)>, b: Option<(u32, u32)>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.0 <= b.1 && b.0 <= a.1)
}

fn group(c: &Change) -> u64 {
    (u64::from(c.op) << 16) | u64::from(c.idx)
}

pub(super) fn build(input: &Input) -> Built {
    let mut seen: HashMap<&str, Vec<u32>> = HashMap::new();
    let mut dones: HashMap<u32, (u32, u32)> = HashMap::new();
    for line in input.lines {
        match &line.op {
            Op::Seen { ids } if line.by.kind == ByKind::Human => {
                for id in ids {
                    seen.entry(id.as_str()).or_default().push(line.n);
                }
            }
            Op::Done { to, turn, .. } if line.by.kind == ByKind::Agent => {
                let counted = input.turns.dones.get(to).is_some_and(|t| t.contains(turn));
                if counted {
                    dones.insert(*to, (line.n, *turn));
                }
            }
            _ => {}
        }
    }
    let seen_after = |id: &str, born: u32, check: Check| -> Vec<(u32, Check)> {
        seen.get(id)
            .into_iter()
            .flatten()
            .filter(|&&n| n > born)
            .map(|&n| (n, check))
            .collect()
    };
    let human_replies = |note: &Note, after: u32, check: Check| -> Vec<(u32, Check)> {
        note.replies
            .iter()
            .filter(|r| r.by.kind == ByKind::Human && !r.retracted && r.n > after)
            .map(|r| (r.n, check))
            .collect()
    };
    let lost: HashSet<u32> = input.lost.iter().copied().collect();
    let removals: HashMap<&str, &Deletion> = input
        .deletions
        .iter()
        .filter(|d| d.kind == DeletionKind::Block)
        .map(|d| (d.change.as_str(), d))
        .collect();

    // Claude's and outside changes, at their block, the block a revert put back, or their gap
    let mut raws: Vec<Raw> = Vec::new();
    let mut places: HashMap<&str, Place> = HashMap::new();
    for c in input.changes {
        if c.attr == Attr::Human {
            continue;
        }
        let overlapped = c.range.status == RangeStatus::Superseded && c.undone_by.is_none();
        // A later write over it keeps it news until settled, on that write's block
        let over = if overlapped {
            // Claude's own later hunk in the same turn stands for both
            if overwritten(c, input.changes) == Some(Over::Own) {
                continue;
            }
            let Some(stop) = c.span.and_then(|s| super::span_stop(input.doc, s)) else {
                continue;
            };
            Some(stop.sp.clone())
        } else {
            None
        };
        // A whole removal sits in its gap until you revert it; then it is back on its block
        let removal = removals
            .get(c.id.as_str())
            .filter(|_| c.range.sp.is_none() && c.undone_by.is_none());
        let block = over.or_else(|| {
            super::change_stop(input.changes, input.doc, c)
                .map(|s| s.sp.clone())
                .or_else(|| c.range.sp.clone().filter(|_| c.undone_by.is_none()))
        });
        let (kind, place) = match removal {
            Some(d) => (
                NewsKind::Removal,
                Place::Removal {
                    change: c.id.clone(),
                    after: d.after_sp.clone(),
                },
            ),
            None => (NewsKind::Change, block.map_or(Place::Doc, Place::Block)),
        };
        let mut events = seen_after(&c.id, c.op, Check::Kept);
        if let Some(undo) = c.undone_by {
            events.push((undo, Check::Reverted));
        } else if let Some(undo) = c.restored_by {
            events.push((undo, Check::Restored));
        }
        places.insert(&c.id, place.clone());
        raws.push(Raw {
            id: c.id.clone(),
            born: c.op,
            kind,
            order: place.order(0, group(c), 0, 0),
            place,
            events,
        });
    }

    // A note you wrote on a change's block while it was still news replies to it
    let mut replied_to: HashMap<u32, String> = HashMap::new();
    let settle_before: Vec<u32> = raws
        .iter()
        .map(|r| {
            let first = r.events.iter().map(|(n, _)| *n).min();
            first
                .and_then(|f| input.rounds.iter().copied().find(|&rd| rd > f))
                .unwrap_or(u32::MAX)
        })
        .collect();
    for i in input.items {
        let live = !matches!(i.status, ItemStatus::Retracted | ItemStatus::Withdrawn);
        let Some(sp) = i.at.loc.as_ref().map(|l| &l.sp) else {
            continue;
        };
        if i.by.kind != ByKind::Human || !live || lost.contains(&i.n) {
            continue;
        }
        let on = raws
            .iter_mut()
            .zip(&settle_before)
            .rev()
            .find(|(r, until)| {
                matches!(&r.place, Place::Block(b) if b == sp) && r.born < i.n && i.n < **until
            });
        if let Some((r, _)) = on {
            r.events.push((i.n, Check::Replied));
            replied_to.insert(i.n, r.id.clone());
        }
    }

    let checks: HashMap<String, Check> = raws
        .iter()
        .filter_map(|r| Some((r.id.clone(), r.check()?)))
        .collect();

    // Where a note sits: its block, the change that took its text, or `whole doc`
    let place_of = |i: &Item| -> Place {
        if matches!(i.at.kind, AtKind::Doc | AtKind::Dir) || lost.contains(&i.n) {
            return Place::Doc;
        }
        if let Some(loc) = &i.at.loc {
            return Place::Block(loc.sp.clone());
        }
        i.at.change
            .as_deref()
            .and_then(|c| places.get(c).cloned())
            .unwrap_or(Place::Doc)
    };
    for i in input.items {
        if matches!(i.status, ItemStatus::Retracted | ItemStatus::Withdrawn) {
            continue;
        }
        let Some(note) = input.notes.get(i.n) else {
            continue;
        };
        let place = place_of(i);
        let at_removal = |line: u32| match &place {
            Place::Removal { change, .. } => {
                let c = input.changes.iter().find(|c| c.id == *change);
                let g = c.map_or(0, group);
                place.order(4, g, 1, line)
            }
            _ => place.order(1, u64::from(i.n), 0, line),
        };
        if i.by.kind == ByKind::Agent {
            if i.verdict == Some(Verdict::Ask) {
                let id = format!("#{}", i.n);
                let place = Place::Question {
                    id: id.clone(),
                    sp: i.at.loc.as_ref().map(|l| l.sp.clone()),
                };
                raws.push(Raw {
                    id,
                    born: i.n,
                    kind: NewsKind::Question,
                    order: place.order(2, 0, 0, i.n),
                    place,
                    events: human_replies(note, i.n, Check::Answered),
                });
            }
            continue;
        }
        let kind = if i.verdict == Some(Verdict::Ask) {
            NewsKind::Answer
        } else {
            NewsKind::Reply
        };
        let doc_order = |line: u32| place.order(0, u64::from(i.n), 0, line);
        let order = |line: u32| match place {
            Place::Doc => doc_order(line),
            _ => at_removal(line),
        };
        let agent: Vec<&_> = note
            .replies
            .iter()
            .filter(|r| r.by.kind == ByKind::Agent && !r.retracted)
            .collect();
        for r in &agent {
            let id = format!("#{}", r.n);
            let mut events = seen_after(&id, r.n, Check::Ok);
            events.extend(human_replies(note, r.n, Check::Replied));
            raws.push(Raw {
                id,
                born: r.n,
                kind,
                place: place.clone(),
                order: order(r.n),
                events,
            });
        }
        // A bare `done` is Claude's reply too, unless a reply line of its turn says more,
        // on time or late
        if let Some(&(line, turn)) = dones.get(&i.n) {
            let close = input.turns.turn(turn).map_or(u32::MAX, |t| t.close());
            let of_turn =
                |n: u32| (n > turn && n <= close) || input.turns.replies.get(&n) == Some(&turn);
            if !agent.iter().any(|r| of_turn(r.n)) {
                let id = format!("#{}", i.n);
                let mut events = seen_after(&id, line, Check::Ok);
                events.extend(human_replies(note, line, Check::Replied));
                raws.push(Raw {
                    id,
                    born: line,
                    kind,
                    place: place.clone(),
                    order: order(line),
                    events,
                });
            }
        }
        if i.status == ItemStatus::NotHandled {
            let born = i
                .turn
                .and_then(|t| input.turns.turn(t))
                .map_or(0, |t| t.close());
            let id = format!("nh:{}", i.n);
            let mut events = seen_after(&id, born, Check::Ok);
            events.extend(human_replies(note, born, Check::Replied));
            let order = match place {
                Place::Doc => doc_order(born),
                _ => place.order(3, 0, 0, i.n),
            };
            raws.push(Raw {
                id,
                born,
                kind: NewsKind::NotHandled,
                place: place.clone(),
                order,
                events,
            });
        }
    }
    for note in &input.notes.turn_notes {
        let id = format!("#{}", note.n);
        let mut events = seen_after(&id, note.n, Check::Ok);
        events.extend(human_replies(note, note.n, Check::Replied));
        raws.push(Raw {
            id,
            born: note.n,
            kind: NewsKind::TurnNote,
            order: Place::Doc.order(3, 0, 0, note.n),
            place: Place::Doc,
            events,
        });
    }

    // An approve keeps every change you didn't revert, so it settles what you never checked too
    raws.retain(|r| !r.settled(input.rounds) && !input.approves.iter().any(|&a| a > r.born));
    raws.sort_by_key(|r| r.order);
    let unchecked: HashSet<String> = raws
        .iter()
        .filter(|r| r.check().is_none())
        .map(|r| r.place.stop())
        .collect();
    let news = raws
        .into_iter()
        .map(|r| {
            let check = r.check();
            let (sp, after_sp) = match r.place {
                Place::Doc => (None, None),
                Place::Block(sp) => (Some(sp), None),
                Place::Question { sp, .. } => (sp, None),
                Place::Removal { after, .. } => (None, Some(after)),
            };
            // Only a removal names its gap; a reply there joins it by its note's `at.change`
            let after_sp = if r.kind == NewsKind::Removal {
                after_sp
            } else {
                None
            };
            News {
                id: r.id,
                kind: r.kind,
                sp,
                after_sp,
                check,
            }
        })
        .collect();
    Built {
        news,
        to_check: unchecked.len() as u32,
        checks,
        replied_to,
    }
}
