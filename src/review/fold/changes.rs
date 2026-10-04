//! Each hunk of each recorded write as a change: who made it, whether a note asked for
//! it, where it is now, what it removed, and whether `x` can undo it.

use std::collections::{BTreeSet, HashMap, HashSet};

use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document};
use similar::{DiffOp, TextDiff};

use super::items::Notes;
use super::label::Write;
use super::texts::{Texts, carry, touches};
use super::turns::Turns;
use crate::anchor;
use crate::rev::Rev;
use crate::review::chain::{Cause, Mapped, Step, map_line};
use crate::review::doc::Doc;
use crate::review::ops::{Act, By, ByKind, Hunk, Line, Op};
use crate::review::state::{
    Asked, AskedKind, Attr, Change, ChangeRange, Deletion, DeletionKind, RangeStatus, Reversible,
    ReversibleReason, What,
};

/// Code points of removed text a change keeps for the page to show.
const REMOVED_CHARS: usize = 2_000;
/// A hunk of at most this many lines, changing at most this many words, counts words.
const SMALL: usize = 3;

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
    /// Ids of hunks that render the same, which make no change.
    pub hidden: &'x HashSet<String>,
    /// Lines of the reviewer's replies to the agent's notes, asks and turn notes.
    pub answers: &'x HashSet<u32>,
}

pub(super) struct Built {
    pub changes: Vec<Change>,
    pub deletions: Vec<Deletion>,
}

/// A human edit's new lines, carried forward while no later write replaces them.
struct Yours {
    id: String,
    span: (u32, u32),
    d: Vec<String>,
    i: Vec<String>,
}

pub(super) fn build(input: &Input, texts: &mut Texts) -> Built {
    let chain = texts.chain;
    let head = texts.head;
    let html = Html::new(head);
    let undone = undone(input, texts);
    let mut yours: Vec<Yours> = Vec::new();
    let mut changes = Vec::new();
    let mut deletions = Vec::new();

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
        let quoted = if who.attr == Attr::Agent {
            Quoted::of(input, texts, who.turn, step)
        } else {
            Quoted::default()
        };
        let (from, to) = (texts.text(&step.from), texts.text(&step.to));
        let write = from
            .as_deref()
            .zip(to.as_deref())
            .map(|(f, t)| Write::new(f, t));
        let mut shift = 0i64;
        for (j, h) in hunks.iter().enumerate() {
            let id = format!("{}.{j}", line.n);
            let new_a = (i64::from(h.a) + shift) as u32;
            shift += h.i.len() as i64 - h.d.len() as i64;
            if input.hidden.contains(&id) {
                continue;
            }
            let (dl, il) = (h.d.len() as u32, h.i.len() as u32);
            let new = (new_a + 1, new_a + il);
            let kind = write.as_ref().and_then(|w| w.label(hunks, j));
            let whole = kind.as_ref().is_some_and(|k| k.whole_removal());
            let followed = follow(later, input.hidden, new_a, &reverses[j]);
            let span = span_now(later, input.hidden, new_a, &reverses[j]);
            let placed = place(followed.as_ref(), new, whole, head);
            let gone = placed.status == RangeStatus::Superseded;

            let asked = match who.attr {
                Attr::Human => asked(AskedKind::Human, Vec::new()),
                Attr::Outside => asked(AskedKind::Outside, Vec::new()),
                Attr::Agent => asked_for(input, texts, who.turn, step, h, j, &quoted),
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
            } else {
                ReversibleReason::None
            };
            let ok = reason == ReversibleReason::None;
            let reverse = followed.filter(|f| ok && f.exact).map(|f| f.reverse);

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
                    first_line: first_line(&h.d),
                    label: kind.as_ref().and_then(|k| k.removed.clone()),
                    head: kind.as_ref().and_then(|k| k.head.clone()),
                    more: kind.as_ref().and_then(|k| k.more),
                });
            }
            for (offset, removed) in gaps {
                let (row, col) = row_col(&h.i, offset);
                let at = new.0 + row;
                let line = Hunk {
                    a: at - 1,
                    d: vec![h.i[row as usize].clone()],
                    i: Vec::new(),
                };
                let now = follow(later, input.hidden, at - 1, &line).map(|f| f.reverse.a + 1);
                deletions.push(Deletion {
                    change: id.clone(),
                    kind: DeletionKind::Words,
                    at_line: now.unwrap_or(at),
                    at_byte: now
                        .and_then(|l| head.lines.range(l))
                        .map(|r| (r.start + col).min(r.end)),
                    removed,
                    after_sp: now.and_then(|l| stop_holding(head, l)),
                    first_line: None,
                    label: None,
                    head: None,
                    more: None,
                });
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
                label: kind.map(|k| k.text),
                removed: (dl > 0 && il < dl).then(|| capped(&h.d.concat())),
                is_rewrite: false,
                old_text: h.d.concat(),
                new_text: h.i.concat(),
                check: None,
                unasked_why: quoted.unasked.get(&j).cloned().flatten(),
                flagged: quoted.unasked.contains_key(&j),
                span,
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
                if !h.i.is_empty() && !input.hidden.contains(&format!("{}.{j}", line.n)) {
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

    let order: HashMap<&str, usize> = changes
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    deletions.sort_by_key(|d| order.get(d.change.as_str()).copied().unwrap_or(usize::MAX));
    Built { changes, deletions }
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
    Asked {
        kind,
        notes,
        answers: Vec::new(),
    }
}

/// Where in one agent write the turn's `unasked` and `covers` quotes read.
#[derive(Default)]
struct Quoted {
    /// Hunks the agent flagged as changed unasked, with the first reason given.
    unasked: HashMap<usize, Option<String>>,
    /// Each covered note, with the hunks its quotes read in.
    covers: Vec<(u32, Vec<usize>)>,
}

impl Quoted {
    fn of(input: &Input, texts: &mut Texts, turn: Option<u32>, step: &Step) -> Self {
        let Some(turn) = turn else {
            return Self::default();
        };
        let removals: Vec<(usize, String)> = step
            .hunks
            .iter()
            .flatten()
            .enumerate()
            .filter(|(_, h)| h.i.is_empty())
            .map(|(j, h)| (j, h.d.concat()))
            .collect();
        let mut unasked: HashMap<usize, Option<String>> = HashMap::new();
        for (q, why) in input.turns.unasked.get(&turn).into_iter().flatten() {
            let gone = removals.iter().filter(|(_, d)| quotes_removed(q, d));
            for j in cited(q, step, texts)
                .into_iter()
                .chain(gone.map(|(j, _)| *j))
            {
                let slot = unasked.entry(j).or_default();
                if slot.is_none() {
                    slot.clone_from(why);
                }
            }
        }
        let covers = input
            .turns
            .covers
            .get(&turn)
            .into_iter()
            .flatten()
            .map(|(to, q)| (*to, cited(q, step, texts)))
            .collect();
        Self { unasked, covers }
    }
}

/// Whether a note of the agent's turn asked for hunk `h`, index `j` of its write: by
/// where it is, or because the agent cited the note for text the hunk wrote.
fn asked_for(
    input: &Input,
    texts: &mut Texts,
    turn: Option<u32>,
    step: &Step,
    h: &Hunk,
    j: usize,
    quoted: &Quoted,
) -> Asked {
    let unasked = || asked(AskedKind::Unasked, Vec::new());
    let Some(turn) = turn else {
        return unasked();
    };
    if quoted.unasked.contains_key(&j) {
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
    let cited = |to: &dyn Fn(u32) -> bool| -> Vec<u32> {
        let quoted = quoted
            .covers
            .iter()
            .filter(|(n, hunks)| to(*n) && hunks.contains(&j));
        quoted
            .map(|(n, _)| *n)
            .collect::<BTreeSet<u32>>()
            .into_iter()
            .collect()
    };
    let notes = cited(&|n| held.contains(&n));
    // An answer written before the turn began reached it
    let answers = cited(&|n| n < turn && input.answers.contains(&n));
    if notes.is_empty() && answers.is_empty() {
        unasked()
    } else {
        Asked {
            kind: AskedKind::Cited,
            notes,
            answers,
        }
    }
}

/// Indexes of the hunks of `step` whose written lines hold `quote`, whitespace aside,
/// wherever it reads in the text the step left.
pub(super) fn cited(quote: &str, step: &Step, texts: &mut Texts) -> Vec<usize> {
    let q = collapse(quote);
    let Some(hunks) = step.hunks.as_deref().filter(|_| !q.is_empty()) else {
        return Vec::new();
    };
    let (Some(text), Some(lines)) = (texts.text(&step.to), texts.lines(&step.to)) else {
        return Vec::new();
    };
    let (flat, from) = flatten(&text);
    let mut out = Vec::new();
    for (at, _) in flat.match_indices(q.as_str()) {
        let first = lines.line_of(from[at]);
        let last = lines.line_of(from[at + q.len() - 1]);
        let mut shift = 0i64;
        for (j, h) in hunks.iter().enumerate() {
            let new_a = (i64::from(h.a) + shift) as u32;
            shift += h.i.len() as i64 - h.d.len() as i64;
            let il = h.i.len() as u32;
            if il > 0 && new_a < last && first <= new_a + il && !out.contains(&j) {
                out.push(j);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Whether an `unasked` quote names text a removal took: any run of it, whitespace aside.
pub fn quotes_removed(quote: &str, removed: &str) -> bool {
    let q = collapse(quote);
    !q.is_empty() && collapse(removed).contains(&q)
}

/// `text` with each whitespace run as one space, and where in `text` each byte is from.
fn flatten(text: &str) -> (String, Vec<usize>) {
    let mut flat = String::with_capacity(text.len());
    let mut from = Vec::with_capacity(text.len());
    let mut space = false;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if !space {
                flat.push(' ');
                from.push(i);
            }
            space = true;
            continue;
        }
        space = false;
        flat.push(c);
        from.extend((0..c.len_utf8()).map(|k| i + k));
    }
    (flat, from)
}

/// Ids of the changes each `covers` of doc note `note` bound, in the turns that held it.
pub(super) fn covers_credit(
    note: u32,
    held_by: &[u32],
    turns: &Turns,
    texts: &mut Texts,
    hidden: &HashSet<String>,
) -> Vec<String> {
    let mut writes: Vec<(u32, u32)> = turns
        .writes
        .iter()
        .filter(|(_, t)| held_by.contains(t))
        .map(|(&n, &t)| (n, t))
        .collect();
    writes.sort_unstable();
    let mut out = Vec::new();
    for (n, turn) in writes {
        let Some((_, step)) = texts.step_at(n) else {
            continue;
        };
        let quotes = turns.covers.get(&turn).into_iter().flatten();
        for (_, q) in quotes.filter(|(to, _)| *to == note) {
            for j in cited(q, step, texts) {
                let id = format!("{n}.{j}");
                if !hidden.contains(&id) && !out.contains(&id) {
                    out.push(id);
                }
            }
        }
    }
    out
}

/// The turns in which a `covers` of each note bound: its quote reads in text a write of
/// that turn put in, whatever the hunk is asked for.
pub(super) fn covers_bound(
    lines: &[Line],
    turns: &Turns,
    texts: &mut Texts,
    hidden: &HashSet<String>,
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
        let who = Who::of(line, step, turns);
        let (Attr::Agent, Some(turn)) = (who.attr, who.turn) else {
            continue;
        };
        let Some(covers) = turns.covers.get(&turn) else {
            continue;
        };
        for (to, q) in covers {
            let shown = cited(q, step, texts)
                .into_iter()
                .any(|j| !hidden.contains(&format!("{}.{j}", line.n)));
            if shown {
                bound.entry(*to).or_default().insert(turn);
            }
        }
    }
    bound
}

/// Ids of the hunks that render the same as the text they replaced: whitespace only,
/// and the page they make reads the same with that hunk alone applied.
pub(super) fn hidden(lines: &[Line], texts: &mut Texts) -> HashSet<String> {
    let mut hidden = HashSet::new();
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
        let spaced: Vec<usize> = (0..hunks.len())
            .filter(|&j| collapse(&hunks[j].d.concat()) == collapse(&hunks[j].i.concat()))
            .collect();
        if spaced.is_empty() {
            continue;
        }
        let Some(from) = texts.text(&step.from) else {
            continue;
        };
        let before = page(&from);
        for j in spaced {
            let after = crate::review::hunk::apply(&from, std::slice::from_ref(&hunks[j]));
            if after.is_ok_and(|after| page(&after) == before) {
                hidden.insert(format!("{}.{j}", line.n));
            }
        }
    }
    hidden
}

/// The HTML a text renders to, without line numbers, and with whitespace runs as one
/// space outside `<pre>` and `<code>`, as a browser shows it.
fn page(text: &str) -> String {
    let mut options = anchor::parse_options();
    options.render.sourcepos = false;
    let html = comrak::markdown_to_html(text, &options);
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    // Code keeps its spaces on the page (`white-space: break-spaces`)
    let verbatim = [("<pre", "</pre>"), ("<code", "</code>")];
    while let Some((at, close)) = verbatim
        .iter()
        .filter_map(|(open, close)| Some((rest.find(open)?, *close)))
        .min_by_key(|(at, _)| *at)
    {
        out.push_str(&squash(&rest[..at]));
        let end = rest[at..].find(close).map_or(rest.len(), |e| at + e);
        out.push_str(&rest[at..end]);
        rest = &rest[end..];
        if let Some(after) = rest.strip_prefix(close) {
            out.push_str(close);
            rest = after;
        }
    }
    out.push_str(&squash(rest));
    out
}

fn squash(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut space = false;
    for c in html.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            space = false;
            out.push(c);
        }
    }
    out
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

/// A change's lines followed through the writes after it.
struct Followed {
    /// Puts back what the change replaced, over the lines its span holds now.
    reverse: Hunk,
    /// No later write put lines inside the span, so `reverse` restores it exactly.
    exact: bool,
}

/// Follows `reverse`, a change's undo at 0-based line `a` of its write's text, through
/// `later`. A hunk over its lines supersedes it, unless the hunk renders the same: that
/// one is taken into the span, and the undo puts back its old lines around the change's.
/// A removal's gap is written over only by a hunk taking lines on both sides of it.
fn follow(later: &[Step], hidden: &HashSet<String>, a: u32, reverse: &Hunk) -> Option<Followed> {
    follow_through(later, hidden, a, reverse, false)
}

/// The lines that hold a change's written text now, taking in whatever later writes put
/// over it; `None` once nothing of it is left.
fn span_now(
    later: &[Step],
    hidden: &HashSet<String>,
    a: u32,
    reverse: &Hunk,
) -> Option<(u32, u32)> {
    let f = follow_through(later, hidden, a, reverse, true)?;
    let len = f.reverse.d.len() as u32;
    (len > 0).then(|| (f.reverse.a + 1, f.reverse.a + len))
}

/// [`follow`], or with `loose` a span a later write overlaps takes that write's lines in.
fn follow_through(
    later: &[Step],
    hidden: &HashSet<String>,
    a: u32,
    reverse: &Hunk,
    loose: bool,
) -> Option<Followed> {
    let (mut s, mut d, mut i) = (a, reverse.d.clone(), reverse.i.clone());
    let mut exact = true;
    for step in later {
        let hunks = step.hunks.as_ref()?;
        let mut end = s + d.len() as u32;
        let gap = d.is_empty();
        let over = |h: &Hunk| {
            let h_end = h.a + h.d.len() as u32;
            if gap {
                h.a < s && s < h_end
            } else if h.d.is_empty() {
                s < h.a && h.a < end
            } else {
                s.max(h.a) < end.min(h_end)
            }
        };
        let mut shift = 0i64;
        let mut taken: Vec<(usize, &Hunk)> = Vec::new();
        for (j, h) in hunks.iter().enumerate() {
            if over(h) {
                taken.push((j, h));
            } else if h.a + h.d.len() as u32 <= s && h.a <= s {
                shift += h.i.len() as i64 - h.d.len() as i64;
            }
        }
        // From the last, so the lines before each hunk are still the text it was made on
        for &(j, h) in taken.iter().rev() {
            let shown = hidden.contains(&format!("{}.{j}", step.n));
            if !shown && !h.d.is_empty() && !loose {
                return None;
            }
            // A line put inside the span is the writer's, and an undo would drop it
            exact &= shown;
            let h_end = h.a + h.d.len() as u32;
            let mut before = d;
            let after = before.split_off(before.len().min(h_end.saturating_sub(s) as usize));
            before.truncate(h.a.saturating_sub(s) as usize);
            d = [before, h.i.clone(), after].concat();
            let head = &h.d[..s.saturating_sub(h.a) as usize];
            let tail = &h.d[h.d.len().min(end.saturating_sub(h.a) as usize)..];
            i = [head, &i, tail].concat();
            (s, end) = (s.min(h.a), end.max(h_end));
        }
        s = (i64::from(s) + shift) as u32;
    }
    Some(Followed {
        reverse: Hunk { a: s, d, i },
        exact,
    })
}

/// Where a followed change is in the current text, from its write's lines `new`; a
/// removal of `whole` blocks has no block of its own, and any other keeps the block it
/// cut into.
fn place(followed: Option<&Followed>, new: (u32, u32), whole: bool, head: &Doc) -> Placed {
    let gone = Placed {
        status: RangeStatus::Superseded,
        lines: new,
        bytes: (0, 0),
        sp: None,
    };
    let Some(f) = followed else {
        return gone;
    };
    let at = f.reverse.a + 1;
    let lines = (at, f.reverse.a + f.reverse.d.len() as u32);
    let status = if lines == new {
        RangeStatus::Ok
    } else {
        RangeStatus::Moved
    };
    let sp = if new.1 < new.0 {
        (!whole)
            .then(|| stop_holding(head, at).or_else(|| stop_holding(head, at - 1)))
            .flatten()
    } else {
        stop_holding(head, at)
    };
    if f.reverse.d.is_empty() {
        let byte = head.lines.range(at).map_or(head.text.len(), |r| r.start);
        return Placed {
            status,
            lines,
            bytes: (byte, byte),
            sp,
        };
    }
    let (Some(first), Some(last)) = (head.lines.range(lines.0), head.lines.range(lines.1)) else {
        return gone;
    };
    Placed {
        status,
        lines,
        bytes: (first.start, last.end),
        sp,
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

/// The first removed line with text, without its terminator.
fn first_line(removed: &[String]) -> Option<String> {
    removed
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim_end_matches(['\r', '\n']).trim_end().to_string())
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
