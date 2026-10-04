//! What kind of change a hunk is and how big, in the words the page shows: `9 words
//! changed`, `41 words → 18`, `section removed · 3 blocks`, `3 lines → 2`.

use similar::{DiffTag, TextDiff};

use crate::anchor::{Stop, StopKind};
use crate::review::cut::words;
use crate::review::doc::Doc;
use crate::review::hunk;
use crate::review::ops::Hunk;

/// A change of more than 1 / `RESIZE` of the words is a resize, not an edit.
const RESIZE: usize = 4;

pub(super) struct Label {
    pub text: String,
    /// What a removal of whole blocks took, for its tombstone.
    pub removed: Option<String>,
    /// A removal's heading and first block, or its first block, as raw markdown.
    pub head: Option<String>,
    /// The removed stops after the head.
    pub more: Option<u32>,
}

impl Label {
    pub fn whole_removal(&self) -> bool {
        self.removed.is_some()
    }
}

/// One write: the lines before and after it, and how many fence lines open or close
/// before each line it started from.
pub(super) struct Write<'t> {
    from: Vec<&'t str>,
    to: Vec<&'t str>,
    fences: Vec<u32>,
}

impl<'t> Write<'t> {
    pub fn new(from: &'t str, to: &'t str) -> Self {
        let from = hunk::lines(from);
        let mut fences = Vec::with_capacity(from.len() + 1);
        let mut seen = 0;
        fences.push(0);
        for line in &from {
            seen += u32::from(is_fence(line));
            fences.push(seen);
        }
        Self {
            from,
            to: hunk::lines(to),
            fences,
        }
    }

    /// The label of hunk `j` of `hunks`, this write's. Only the blocks around the hunk
    /// are parsed: from a blank line outside any fence to another, and no further than
    /// the hunks either side.
    pub fn label(&self, hunks: &[Hunk], j: usize) -> Option<Label> {
        let h = &hunks[j];
        let (dl, il) = (h.d.len(), h.i.len());
        let at = h.a as usize;
        let inside = |i: usize| self.fences.get(i).is_some_and(|f| !f.is_multiple_of(2));
        // In a fence, and not opening or closing one, a hunk is lines of code
        if inside(at) && !h.d.iter().chain(&h.i).any(|l| is_fence(l)) {
            return Some(code_lines(dl, il));
        }
        let shift: i64 = hunks[..j]
            .iter()
            .map(|h| h.i.len() as i64 - h.d.len() as i64)
            .sum();
        let floor = j
            .checked_sub(1)
            .map_or(0, |k| hunks[k].a as usize + hunks[k].d.len());
        let ceiling = hunks.get(j + 1).map_or(self.from.len(), |n| n.a as usize);
        // 0-based: the window is `start..end` of the text before
        let start = self.edge((floor..at).rev()).unwrap_or(floor);
        let end = self
            .edge(at + dl..ceiling)
            .unwrap_or(ceiling)
            .clamp(at + dl, self.from.len());
        let to_start = (start as i64 + shift) as usize;
        let to_end = (end as i64 + shift) as usize + il - dl;
        let old = Doc::new(&self.from.get(start..end)?.concat());
        let new = Doc::new(&self.to.get(to_start..to_end)?.concat());
        let local = (at - start) as u32;
        let h = Hunk {
            a: local,
            d: h.d.clone(),
            i: h.i.clone(),
        };
        Some(of(&h, local, &old, &new))
    }

    /// The first line in `range` where blocks start afresh: blank, outside any fence,
    /// with an unindented line after it.
    fn edge(&self, mut range: impl Iterator<Item = usize>) -> Option<usize> {
        range.find(|&i| {
            self.from.get(i).is_some_and(|l| l.trim().is_empty())
                && self
                    .from
                    .get(i + 1)
                    .is_none_or(|l| !l.starts_with([' ', '\t']))
                && self.fences[i].is_multiple_of(2)
        })
    }
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start_matches(' ');
    line.len() - t.len() < 4 && (t.starts_with("```") || t.starts_with("~~~"))
}

/// Code is counted in lines.
fn code_lines(dl: usize, il: usize) -> Label {
    let text = match (dl, il) {
        (d, 0) => format!("{} removed", count(d, "line", "lines")),
        (0, i) => format!("{} added", count(i, "line", "lines")),
        (d, i) if d == i => format!("{} changed", count(d, "line", "lines")),
        (d, i) => format!("{} → {i}", count(d, "line", "lines")),
    };
    Label::plain(text)
}

impl Label {
    fn plain(text: String) -> Self {
        Self {
            text,
            removed: None,
            head: None,
            more: None,
        }
    }
}

/// The label of hunk `h`, which turned `from` into `to` and starts at `new_a` in `to`.
fn of(h: &Hunk, new_a: u32, from: &Doc, to: &Doc) -> Label {
    let (dl, il) = (h.d.len() as u32, h.i.len() as u32);
    let old = Side::new(from, (h.a + 1, h.a + dl));
    let new = Side::new(to, (new_a + 1, new_a + il));
    let plain = Label::plain;
    let code = |s: &Side| s.stops.iter().all(|s| s.kind == StopKind::CodeBlock);
    if !(old.stops.is_empty() && new.stops.is_empty()) && code(&old) && code(&new) {
        return code_lines(dl as usize, il as usize);
    }
    if new.stops.is_empty() && old.whole() {
        let what = blocks(&old.stops);
        let (head, more) = head(&old);
        return Label {
            text: removed_or_added(&what, "removed"),
            removed: Some(what),
            head: Some(head),
            more: Some(more),
        };
    }
    if old.stops.is_empty() && new.whole() {
        return plain(removed_or_added(&blocks(&new.stops), "added"));
    }
    let (n, m) = (old.stops.len(), new.stops.len());
    if n > 0 && m > 0 && n != m {
        let kind = old.stops[0].kind.clone();
        let same = old.stops.iter().chain(&new.stops).all(|s| s.kind == kind);
        let (one, many) = if same {
            name(&kind)
        } else {
            ("block", "blocks")
        };
        return plain(format!("{} → {m}", count(n, one, many)));
    }
    plain(by_words(h, new_a, &old, &new))
}

/// One side of a hunk: its lines and the stops they touch.
struct Side<'d> {
    doc: &'d Doc,
    lines: (u32, u32),
    stops: Vec<&'d Stop>,
}

impl<'d> Side<'d> {
    fn new(doc: &'d Doc, lines: (u32, u32)) -> Self {
        let stops = doc
            .blocks
            .stops
            .iter()
            .filter(|s| s.lines.0 <= lines.1 && lines.0 <= s.lines.1)
            .collect();
        Self { doc, lines, stops }
    }

    /// Every block it touches lies wholly inside it.
    fn whole(&self) -> bool {
        !self.stops.is_empty()
            && self
                .stops
                .iter()
                .all(|s| self.lines.0 <= s.lines.0 && s.lines.1 <= self.lines.1)
    }

    /// The lines from `first` to `last`, without the last one's terminator.
    fn text(&self, first: u32, last: u32) -> &'d str {
        let (Some(a), Some(b)) = (self.doc.lines.range(first), self.doc.lines.range(last)) else {
            return "";
        };
        self.doc.text.get(a.start..b.end).unwrap_or("")
    }
}

/// The words of the whole blocks the hunk touches, before and after.
fn by_words(h: &Hunk, new_a: u32, old: &Side, new: &Side) -> String {
    let delta = i64::from(new.lines.1) - i64::from(new.lines.0) - i64::from(old.lines.1)
        + i64::from(old.lines.0);
    let first = |s: &Side| s.stops.iter().map(|s| s.lines.0).fold(s.lines.0, u32::min);
    let last = |s: &Side| s.stops.iter().map(|s| s.lines.1).fold(s.lines.1, u32::max);
    // Lines before the hunk number the same on both sides
    let start = first(old).min(first(new)).min(h.a + 1).min(new_a + 1);
    let end_old = i64::from(last(old)).max(i64::from(last(new)) - delta);
    let end_new = end_old + delta;
    let was = words(old.text(start, end_old.max(0) as u32));
    let now = words(new.text(start, end_new.max(0) as u32));
    let (wo, wn) = (was.len(), now.len());
    if wo.abs_diff(wn) * RESIZE > wo {
        return format!("{} → {wn}", count(wo, "word", "words"));
    }
    let diff = TextDiff::configure().diff_slices(&was, &now);
    let (mut gone, mut came) = (0, 0);
    for op in diff.ops() {
        match op.tag() {
            DiffTag::Equal => {}
            _ => {
                gone += op.old_range().len();
                came += op.new_range().len();
            }
        }
    }
    match gone.max(came) {
        0 => "formatting changed".to_string(),
        n => format!("{} changed", count(n, "word", "words")),
    }
}

/// A removal's head, a heading with its first block or else the first block, and how
/// many of its stops come after it.
fn head(side: &Side) -> (String, u32) {
    let n = stop_count(&side.stops);
    let first = side.stops[0];
    let second = side
        .stops
        .get(1)
        .filter(|_| first.kind == StopKind::Heading);
    let last = second.unwrap_or(&first);
    let taken = 1 + usize::from(second.is_some());
    let text = side.text(first.lines.0, last.lines.1).to_string();
    (text, n.saturating_sub(taken) as u32)
}

/// How many blocks a removal or an addition holds; a block nested in another is its part.
fn stop_count(stops: &[&Stop]) -> usize {
    // Stops come parent first, so a nested one follows the block holding it
    stops
        .iter()
        .enumerate()
        .filter(|(k, s)| {
            !stops[..*k]
                .iter()
                .any(|p| p.lines.0 <= s.lines.0 && s.lines.1 <= p.lines.1)
        })
        .count()
}

/// The blocks `stops` are, as a removal or an addition names them.
fn blocks(stops: &[&Stop]) -> String {
    let n = stop_count(stops);
    if n > 1 && stops.iter().any(|s| s.kind == StopKind::Heading) {
        return format!("section · {}", count(n, "block", "blocks"));
    }
    let kind = &stops[0].kind;
    let (one, many) = if stops.iter().all(|s| s.kind == *kind) {
        name(kind)
    } else {
        ("block", "blocks")
    };
    if n == 1 {
        one.to_string()
    } else {
        format!("{n} {many}")
    }
}

/// `section · 2 blocks` as `section removed · 2 blocks`, `paragraph` as `paragraph removed`.
fn removed_or_added(what: &str, verb: &str) -> String {
    match what.split_once(" · ") {
        Some((head, rest)) => format!("{head} {verb} · {rest}"),
        None => format!("{what} {verb}"),
    }
}

fn name(kind: &StopKind) -> (&'static str, &'static str) {
    match kind {
        StopKind::ListItem => ("list item", "list items"),
        StopKind::TableRow => ("table row", "table rows"),
        StopKind::Heading => ("heading", "headings"),
        StopKind::Paragraph => ("paragraph", "paragraphs"),
        StopKind::CodeBlock => ("code block", "code blocks"),
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
