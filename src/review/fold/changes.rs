//! Each hunk of each recorded write as a change: who made it, whether a note asked for
//! it, where it is now, what it removed, and whether `x` can undo it.

use std::collections::{BTreeSet, HashMap};

use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document};
use similar::{DiffOp, TextDiff};

use super::items::Notes;
use super::texts::{Texts, carry, touches};
use super::turns::Turns;
use crate::anchor;
use crate::rev::Rev;
use crate::review::chain::{Cause, Mapped, Step, map_line};
use crate::review::doc::Doc;
use crate::review::ops::{Act, By, ByKind, Hunk, Line, Op};
use crate::review::state::{
    Asked, AskedKind, Attr, Change, ChangeRange, Deletion, DeletionKind, RangeStatus, Reversible,
    ReversibleReason, Rewrite, What,
};

/// Code points of removed text a change keeps for the page to show.
const REMOVED_CHARS: usize = 2_000;
/// A hunk of at most this many lines, changing at most this many words, counts words.
const SMALL: usize = 3;
/// A turn that touches more than this share of the doc's blocks rewrote it.
const REWRITE_SHARE: f64 = 0.40;

pub(super) struct Input<'x> {
    pub lines: &'x [Line],
    pub turns: &'x Turns,
    pub notes: &'x Notes,
    /// The notes each turn holds, by claim line.
    pub held: &'x HashMap<u32, Vec<u32>>,
    /// Stet notes, with the line of the hand-back that sent each.
    pub stets: &'x [(u32, u32)],
    /// The first human `seen` line naming each id.
    pub seen: &'x HashMap<String, u32>,
    /// Lines of hand-backs and approves that stand.
    pub rounds: &'x [u32],
}

pub(super) struct Built {
    pub changes: Vec<Change>,
    pub deletions: Vec<Deletion>,
    pub rewrite: Option<Rewrite>,
}

/// A human edit's new lines, carried forward while no later write replaces them.
struct Yours {
    id: String,
    span: (u32, u32),
    d: Vec<String>,
    i: Vec<String>,
}

/// Blocks a turn touched, for telling a rewrite.
#[derive(Default)]
struct Touched {
    stops: BTreeSet<usize>,
    removed: u32,
}

pub(super) fn build(input: &Input, texts: &mut Texts) -> Built {
    let chain = texts.chain;
    let head = texts.head;
    let html = Html::new(head);
    let undone = undone(input, texts);
    let mut yours: Vec<Yours> = Vec::new();
    let mut changes = Vec::new();
    let mut deletions = Vec::new();
    let mut touched: HashMap<u32, Touched> = HashMap::new();

    for line in input.lines {
        let step = match &line.op {
            // Your edits before an approve were approved with the text
            Op::Approve { .. } => {
                yours.clear();
                continue;
            }
            Op::Snap { .. } | Op::Edit { .. } | Op::External { .. } => texts.step_at(line.n),
            _ => continue,
        };
        let Some((k, step)) = step else {
            if !matches!(line.op, Op::Snap { .. }) {
                yours.clear();
            }
            continue;
        };
        let Some(hunks) = &step.hunks else {
            yours.clear();
            continue;
        };
        let later = &chain.history()[k + 1..];
        let who = Who::of(line, step, input.turns);
        let reverses = crate::review::hunk::reverse(hunks);
        let mut shift = 0i64;
        for (j, h) in hunks.iter().enumerate() {
            let id = format!("{}.{j}", line.n);
            let new_a = (i64::from(h.a) + shift) as u32;
            shift += h.i.len() as i64 - h.d.len() as i64;
            let (dl, il) = (h.d.len() as u32, h.i.len() as u32);
            let new = (new_a + 1, new_a + il);
            let placed = place(later, new, il == 0, head);
            let gone = placed.status == RangeStatus::Superseded;

            let asked = match who.attr {
                Attr::Human => asked(AskedKind::Human, Vec::new()),
                Attr::Outside => asked(AskedKind::Outside, Vec::new()),
                Attr::Agent => asked_for(input, texts, who.turn, step, h),
            };
            let breaks_stet = if who.attr == Attr::Agent {
                breaks_stet(input, texts, line.n, step, h)
            } else {
                Vec::new()
            };
            let seen_at = input.seen.get(&id).copied();
            let undone_by = undone.get(id.as_str()).copied();
            let settled =
                undone_by.is_some() || seen_at.is_some_and(|s| input.rounds.iter().any(|&r| r > s));
            let (mut changes_your_edit, mut reverts_your_edit) = (None, false);
            if who.attr == Attr::Agent
                && seen_at.is_none()
                && let Some(y) = yours.iter().find(|y| touches(h, y.span))
            {
                changes_your_edit = Some(y.id.clone());
                reverts_your_edit = h.d == y.i && h.i == y.d;
            }

            let (words, gaps) = words(h);
            // An undo is the later write over what it reverted
            let reason = if gone || undone_by.is_some() {
                ReversibleReason::LaterOverlap
            } else if matches!(line.op, Op::Snap { .. }) {
                ReversibleReason::Snap
            } else if who.attr == Attr::Outside {
                ReversibleReason::Outside
            } else if hunks.len() > 1 {
                ReversibleReason::MultiHunk
            } else {
                ReversibleReason::None
            };
            let ok = matches!(reason, ReversibleReason::None | ReversibleReason::MultiHunk);
            let reverse = ok.then(|| Hunk {
                a: placed.lines.0 - 1,
                ..reverses[j].clone()
            });

            if dl > 0 && il == 0 {
                deletions.push(Deletion {
                    change: id.clone(),
                    kind: DeletionKind::Block,
                    at_line: placed.lines.0,
                    at_byte: (!gone).then_some(placed.bytes.0),
                    removed: capped(&h.d.concat()),
                    after_sp: if gone {
                        None
                    } else {
                        stop_above(head, placed.lines.0)
                    },
                });
            }
            for (offset, removed) in gaps {
                let (row, col) = row_col(&h.i, offset);
                let at = new.0 + row;
                let now = mapped(map_line(later, at));
                deletions.push(Deletion {
                    change: id.clone(),
                    kind: DeletionKind::Words,
                    at_line: now.unwrap_or(at),
                    at_byte: now.and_then(|l| head.lines.range(l)).map(|r| r.start + col),
                    removed,
                    after_sp: now.and_then(|l| stop_holding(head, l)),
                });
            }

            if who.attr == Attr::Agent
                && let Some(turn) = who.turn
            {
                let entry = touched.entry(turn).or_default();
                if !gone && il > 0 {
                    entry.stops.extend(stops_in(head, placed.lines));
                }
                if dl > il {
                    let before = texts
                        .with_doc(&step.from, |doc| {
                            stops_in(doc, (h.a + 1, h.a + dl)).count() as u32
                        })
                        .unwrap_or(0);
                    let after = if gone || il == 0 {
                        0
                    } else {
                        stops_in(head, placed.lines).count() as u32
                    };
                    entry.removed += before.saturating_sub(after);
                }
            }

            let touches_html = if gone || il == 0 {
                h.d.iter().chain(&h.i).any(|l| looks_like_html(l))
            } else {
                html.touches(placed.lines)
            };
            changes.push(Change {
                id,
                op: line.n,
                idx: j as u32,
                turn: who.turn,
                attr: who.attr,
                by: who.by.clone(),
                cause: who.cause.clone(),
                old: (h.a + 1, h.a + dl),
                new,
                range: ChangeRange {
                    status: placed.status,
                    lines: placed.lines,
                    bytes: placed.bytes,
                    sp: placed.sp,
                },
                asked,
                changes_your_edit,
                reverts_your_edit,
                breaks_stet,
                seen: seen_at.is_some(),
                settled,
                undone_by,
                what: What {
                    old_lines: dl,
                    new_lines: il,
                    words,
                    touches_html,
                },
                removed: (dl > 0 && il < dl).then(|| capped(&h.d.concat())),
                is_rewrite: false,
                reverse,
                reversible: Reversible { ok, reason },
                op_hunks: hunks.len() as u32,
            });
        }

        yours.retain_mut(|y| match carry(std::slice::from_ref(step), y.span) {
            Some(span) => {
                y.span = span;
                true
            }
            None => false,
        });
        if who.attr == Attr::Human {
            let mut shift = 0i64;
            for (j, h) in hunks.iter().enumerate() {
                let new_a = (i64::from(h.a) + shift) as u32;
                shift += h.i.len() as i64 - h.d.len() as i64;
                if !h.i.is_empty() {
                    yours.push(Yours {
                        id: format!("{}.{j}", line.n),
                        span: (new_a + 1, new_a + h.i.len() as u32),
                        d: h.d.clone(),
                        i: h.i.clone(),
                    });
                }
            }
        }
    }

    let total = head.blocks.stops.len() as u32;
    let rewrite = touched
        .iter()
        .filter_map(|(&turn, t)| {
            let changed = t.stops.len() as u32 + t.removed;
            let all = total + t.removed;
            let pct = f64::from(changed) / f64::from(all.max(1));
            let mine = || changes.iter().filter(|c: &&Change| c.turn == Some(turn));
            let unseen = mine().any(|c| !c.seen);
            (pct > REWRITE_SHARE && unseen).then_some(Rewrite {
                turn,
                pct,
                blocks_changed: changed,
                blocks_total: all,
            })
        })
        .max_by_key(|r| r.turn);
    if let Some(r) = &rewrite {
        for c in &mut changes {
            c.is_rewrite = c.turn == Some(r.turn);
        }
    }
    let order: HashMap<&str, usize> = changes
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    deletions.sort_by_key(|d| order.get(d.change.as_str()).copied().unwrap_or(usize::MAX));
    Built {
        changes,
        deletions,
        rewrite,
    }
}

/// Who made a write, as the chain and the turns tell it.
struct Who {
    attr: Attr,
    turn: Option<u32>,
    by: By,
    cause: Option<String>,
}

impl Who {
    fn of(line: &Line, step: &Step, turns: &Turns) -> Self {
        match step.cause {
            Cause::Edit => Self {
                attr: Attr::Human,
                turn: None,
                by: line.by.clone(),
                cause: None,
            },
            Cause::Agent => {
                let turn = turns.writes.get(&line.n).copied();
                Self {
                    attr: Attr::Agent,
                    turn,
                    by: By {
                        kind: ByKind::Agent,
                        name: turn
                            .and_then(|t| turns.turn(t))
                            .and_then(|t| t.agent.clone()),
                        via: None,
                    },
                    cause: None,
                }
            }
            Cause::Outside { git } => Self {
                attr: Attr::Outside,
                turn: None,
                by: line.by.clone(),
                cause: git.then(|| "git".to_string()),
            },
        }
    }
}

fn asked(kind: AskedKind, notes: Vec<u32>) -> Asked {
    Asked { kind, notes }
}

/// Whether a note of the agent's turn asked for hunk `h`: by where it is, or because the
/// agent cited the note for text the hunk wrote.
fn asked_for(input: &Input, texts: &mut Texts, turn: Option<u32>, step: &Step, h: &Hunk) -> Asked {
    let unasked = || asked(AskedKind::Unasked, Vec::new());
    let Some(turn) = turn else {
        return unasked();
    };
    let wrote = collapse(&h.i.concat());
    let quoted = |q: &str| {
        let q = collapse(q);
        !q.is_empty() && wrote.contains(&q)
    };
    let flagged = input
        .turns
        .unasked
        .get(&turn)
        .is_some_and(|quotes| quotes.iter().any(|q| quoted(q)));
    if flagged {
        return unasked();
    }
    let held: &[u32] = input.held.get(&turn).map_or(&[], Vec::as_slice);
    let location: Vec<u32> = held
        .iter()
        .copied()
        .filter(|&n| {
            let Some(anchor) = input.notes.get(n).and_then(|note| note.anchor()) else {
                return false;
            };
            texts
                .span_at(n, anchor, &step.from)
                .is_some_and(|span| touches(h, span))
        })
        .collect();
    if !location.is_empty() {
        return asked(AskedKind::Location, location);
    }
    let cited: BTreeSet<u32> = input
        .turns
        .covers
        .get(&turn)
        .into_iter()
        .flatten()
        .filter(|(to, q)| held.contains(to) && quoted(q))
        .map(|(to, _)| *to)
        .collect();
    if cited.is_empty() {
        unasked()
    } else {
        asked(AskedKind::Cited, cited.into_iter().collect())
    }
}

/// The turns in which a `covers` of each note bound: its quote is in text a write of that
/// turn put in, whatever the hunk is asked for.
pub(super) fn covers_bound(
    lines: &[Line],
    turns: &Turns,
    texts: &Texts,
) -> HashMap<u32, BTreeSet<u32>> {
    let mut bound: HashMap<u32, BTreeSet<u32>> = HashMap::new();
    for line in lines {
        if !matches!(
            line.op,
            Op::Snap { .. } | Op::Edit { .. } | Op::External { .. }
        ) {
            continue;
        }
        let Some((_, step)) = texts.step_at(line.n) else {
            continue;
        };
        let Some(hunks) = &step.hunks else {
            continue;
        };
        let who = Who::of(line, step, turns);
        let (Attr::Agent, Some(turn)) = (who.attr, who.turn) else {
            continue;
        };
        let Some(covers) = turns.covers.get(&turn) else {
            continue;
        };
        for h in hunks {
            let wrote = collapse(&h.i.concat());
            for (to, q) in covers {
                let q = collapse(q);
                if !q.is_empty() && wrote.contains(&q) {
                    bound.entry(*to).or_default().insert(turn);
                }
            }
        }
    }
    bound
}

/// Sent stet notes whose block hunk `h`, written on line `n`, changes.
fn breaks_stet(input: &Input, texts: &mut Texts, n: u32, step: &Step, h: &Hunk) -> Vec<u32> {
    input
        .stets
        .iter()
        .filter(|(_, sent)| *sent < n)
        .filter(|(note, _)| {
            let Some(anchor) = input.notes.get(*note).and_then(|note| note.anchor()) else {
                return false;
            };
            texts
                .span_at(*note, anchor, &step.from)
                .is_some_and(|span| touches(h, span))
        })
        .map(|(note, _)| *note)
        .collect()
}

/// Where a change is in the current text.
struct Placed {
    status: RangeStatus,
    /// For a pure deletion, `(at, at - 1)` with `at` the line after the gap.
    lines: (u32, u32),
    bytes: (usize, usize),
    sp: Option<String>,
}

/// Lines `new` of a write's text carried through the writes after it. With `gap`, the
/// hunk removed lines and `new.0` is the line after them.
/// The undo that reverted each change id. An undo names one change; the rest of its
/// write counts as undone too when the undo's patch reverted every hunk of it.
fn undone(input: &Input, texts: &mut Texts) -> HashMap<String, u32> {
    let mut undone = HashMap::new();
    for line in input.lines {
        let Op::Edit {
            act: Act::Undo,
            of: Some(of),
            ..
        } = &line.op
        else {
            continue;
        };
        undone.insert(of.clone(), line.n);
        let Some((op, _)) = of.split_once('.') else {
            continue;
        };
        let Some(op) = op.parse::<u32>().ok().filter(|n| n.to_string() == op) else {
            continue;
        };
        if let Some(hunks) = reverted_whole(texts, op, line.n) {
            for j in 0..hunks {
                undone.insert(format!("{op}.{j}"), line.n);
            }
        }
    }
    undone
}

/// How many hunks the write on line `op` has, when the undo on line `undo` reverted
/// all of them at once.
fn reverted_whole(texts: &mut Texts, op: u32, undo: u32) -> Option<usize> {
    let (k, step) = texts.step_at(op)?;
    let hunks = step.hunks.as_ref().filter(|h| h.len() > 1)?;
    let (u, undo) = texts.step_at(undo)?;
    let between = texts.chain.history().get(k + 1..u)?;
    let mut reverses = Vec::with_capacity(hunks.len());
    let mut shift = 0i64;
    for h in hunks {
        let new_a = (i64::from(h.a) + shift) as u32;
        shift += h.i.len() as i64 - h.d.len() as i64;
        let at = mapped(map_line(between, new_a + 1))?;
        // Every line the hunk wrote must still be there, in one piece
        let il = h.i.len() as u32;
        if il > 0 && mapped(map_line(between, new_a + il))? != at + il - 1 {
            return None;
        }
        reverses.push(Hunk {
            a: at - 1,
            d: h.i.clone(),
            i: h.d.clone(),
        });
    }
    let from = texts.text(&undo.from)?;
    let reverted = crate::review::hunk::apply(&from, &reverses).ok()?;
    (Rev::of(reverted.as_bytes()) == undo.to).then_some(hunks.len())
}

fn place(later: &[Step], new: (u32, u32), gap: bool, head: &Doc) -> Placed {
    let gone = Placed {
        status: RangeStatus::Superseded,
        lines: new,
        bytes: (0, 0),
        sp: None,
    };
    let status = |lines: (u32, u32)| {
        if lines == new {
            RangeStatus::Ok
        } else {
            RangeStatus::Moved
        }
    };
    if gap {
        let Some(at) = mapped(map_line(later, new.0)) else {
            return gone;
        };
        let byte = head.lines.range(at).map_or(head.text.len(), |r| r.start);
        return Placed {
            status: status((at, at - 1)),
            lines: (at, at - 1),
            bytes: (byte, byte),
            sp: stop_holding(head, at),
        };
    }
    // Any later hunk over any of its lines writes over it, not only one at an end
    let mut now = Vec::with_capacity((new.1 - new.0 + 1) as usize);
    for line in new.0..=new.1 {
        match mapped(map_line(later, line)) {
            Some(l) => now.push(l),
            None => return gone,
        }
    }
    let lines = (now[0], now[now.len() - 1]);
    let (Some(first), Some(last)) = (head.lines.range(lines.0), head.lines.range(lines.1)) else {
        return gone;
    };
    Placed {
        status: status(lines),
        lines,
        bytes: (first.start, last.end),
        sp: stop_holding(head, lines.0),
    }
}

fn mapped(m: Mapped) -> Option<u32> {
    match m {
        Mapped::Same(line) | Mapped::Shifted(line) => Some(line),
        Mapped::Overlapped => None,
    }
}

/// The innermost stop holding `line`.
fn stop_holding(doc: &Doc, line: u32) -> Option<String> {
    doc.blocks
        .stops
        .iter()
        .rfind(|s| s.lines.0 <= line && line <= s.lines.1)
        .map(|s| s.sp.clone())
}

/// The stop that ends last above `line`; of stops ending together, the innermost.
fn stop_above(doc: &Doc, line: u32) -> Option<String> {
    doc.blocks
        .stops
        .iter()
        .filter(|s| s.lines.1 < line)
        .max_by_key(|s| (s.lines.1, s.lines.0))
        .map(|s| s.sp.clone())
}

/// Indexes of the stops with a line in `span`.
fn stops_in(doc: &Doc, (first, last): (u32, u32)) -> impl Iterator<Item = usize> + '_ {
    doc.blocks
        .stops
        .iter()
        .enumerate()
        .filter(move |(_, s)| s.lines.0 <= last && first <= s.lines.1)
        .map(|(i, _)| i)
}

/// How many words a small hunk changed, and the places in its new text where it only
/// removed words, with what it removed.
fn words(h: &Hunk) -> (Option<u32>, Vec<(usize, String)>) {
    if h.d.len() > SMALL || h.i.len() > SMALL {
        return (None, Vec::new());
    }
    let (old, new) = (h.d.concat(), h.i.concat());
    let diff = TextDiff::from_words(old.as_str(), new.as_str());
    let old_words: Vec<&str> = diff.iter_old_slices().collect();
    let new_words: Vec<&str> = diff.iter_new_slices().collect();
    let count = |ws: &[&str]| ws.iter().filter(|w| !w.trim().is_empty()).count();
    let starts: Vec<usize> = std::iter::once(0)
        .chain(new_words.iter().scan(0, |at, w| {
            *at += w.len();
            Some(*at)
        }))
        .collect();
    let (mut removed, mut added) = (0, 0);
    let mut gaps = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => {
                let gone = &old_words[old_index..old_index + old_len];
                removed += count(gone);
                // Only a replace hunk has a caret; a pure delete leaves a block tick
                if count(gone) > 0 && !h.i.is_empty() {
                    gaps.push((starts[new_index], gone.concat()));
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => added += count(&new_words[new_index..new_index + new_len]),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                removed += count(&old_words[old_index..old_index + old_len]);
                added += count(&new_words[new_index..new_index + new_len]);
            }
        }
    }
    let changed = removed.max(added);
    ((changed <= SMALL).then_some(changed as u32), gaps)
}

/// The line of `lines` holding byte `offset` of their concatenation, and the column.
fn row_col(lines: &[String], offset: usize) -> (u32, usize) {
    let mut start = 0;
    for (row, line) in lines.iter().enumerate() {
        if offset < start + line.len() || row + 1 == lines.len() {
            return (row as u32, offset - start);
        }
        start += line.len();
    }
    (0, offset)
}

fn capped(text: &str) -> String {
    text.chars().take(REMOVED_CHARS).collect()
}

/// `s` with whitespace runs as one space and none at either end.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A tag or comment in source that can no longer be looked up on the page.
fn looks_like_html(line: &str) -> bool {
    line.as_bytes()
        .windows(2)
        .any(|w| w[0] == b'<' && (w[1].is_ascii_alphabetic() || matches!(w[1], b'/' | b'!' | b'?')))
}

/// Lines of the current text the page shows as raw HTML or not at all.
struct Html {
    /// By 0-based line: raw HTML, or text no block renders (a link definition).
    hidden: Vec<bool>,
}

impl Html {
    fn new(doc: &Doc) -> Self {
        let count = doc.lines.count() as usize;
        let mut html = vec![false; count];
        let mut covered = vec![false; count];
        let arena = Arena::new();
        let root = parse_document(&arena, &doc.text, &anchor::parse_options());
        for block in root.children() {
            let sp = block.data.borrow().sourcepos;
            mark(&mut covered, sp.start.line, sp.end.line);
        }
        for node in root.descendants() {
            let data = node.data.borrow();
            if matches!(
                data.value,
                NodeValue::HtmlBlock(_) | NodeValue::HtmlInline(_)
            ) {
                mark(
                    &mut html,
                    data.sourcepos.start.line,
                    data.sourcepos.end.line,
                );
            }
        }
        let hidden = (0..count)
            .map(|i| {
                let blank = doc
                    .lines
                    .range(i as u32 + 1)
                    .is_none_or(|r| doc.text[r].trim().is_empty());
                html[i] || (!covered[i] && !blank)
            })
            .collect();
        Self { hidden }
    }

    fn touches(&self, (first, last): (u32, u32)) -> bool {
        (first..=last).any(|l| {
            l.checked_sub(1)
                .and_then(|i| self.hidden.get(i as usize))
                .copied()
                .unwrap_or(false)
        })
    }
}

/// Sets `flags` for 1-based lines `first..=last`.
fn mark(flags: &mut [bool], first: usize, last: usize) {
    for flag in flags.iter_mut().take(last).skip(first.saturating_sub(1)) {
        *flag = true;
    }
}
