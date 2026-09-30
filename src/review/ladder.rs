//! Re-locating a note's anchor on the doc's current text. Each tier is tried in turn:
//! the anchor's own rev, the chain's line mapping, a search of the whole text, and a
//! fuzzy match of the lines near where the chain says the anchor went.

use std::collections::HashMap;
use std::ops::Range;

use similar::TextDiff;

use super::chain::{Cause, Chain, Step};
use super::doc::Doc;
use super::ops::{Anchor, Scope};
use super::state::AnchorStatus;
use crate::anchor::{self, OwnerKind, Stop, StopKind, collapse_str, is_js_space};

/// Code points of source an anchor keeps on each side of its quote.
pub(crate) const CONTEXT: usize = 32;
/// Matching context on one side of a search hit that makes it the anchor's.
const EVIDENCE: usize = 8;
/// The least `similar` char ratio a fuzzy match needs.
const FUZZY_RATIO: f32 = 0.75;
/// Lines searched on each side of where the chain put a changed anchor.
const FUZZY_REACH: u32 = 5;
/// At most this many lines are tried as fuzzy windows.
const FUZZY_LINES: u32 = 200;
/// A write from outside that deletes more than this share of the lines may be undone.
const REWRITE_SHARE: f64 = 0.4;

/// Where an anchor is in the current text. Nothing is set when it can't be placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub status: AnchorStatus,
    pub lines: Option<(u32, u32)>,
    pub bytes: Option<Range<usize>>,
    /// Lines of the innermost stop holding the first line.
    pub block: Option<(u32, u32)>,
    /// Rendered text of the innermost heading whose section holds the first line.
    pub section: Option<String>,
}

impl Located {
    fn nowhere(status: AnchorStatus) -> Self {
        Self {
            status,
            lines: None,
            bytes: None,
            block: None,
            section: None,
        }
    }
}

/// Places `anchor` on `doc`, using `chain` to follow it from the rev it was made on.
pub fn relocate(anchor: &Anchor, doc: &Doc, chain: &Chain) -> Located {
    let steps = if anchor.rev == doc.rev {
        Some(&[][..])
    } else {
        chain.steps(&anchor.rev, &doc.rev)
    };
    // A section anchor's lines are the whole section; only its heading line is tracked
    let span = match anchor.scope {
        Scope::Section => (anchor.lines.0, anchor.lines.0),
        Scope::Quote | Scope::Block => anchor.lines,
    };
    let mapped = steps.map_or(Mapped::Unknown, |steps| map_span(steps, span));
    let found = match anchor.scope {
        Scope::Quote => place_quote(anchor, doc, span, mapped),
        Scope::Block | Scope::Section => place_block(anchor, doc, span, mapped),
    };
    match found {
        Some(located) if located.status != AnchorStatus::Fuzzy => located,
        _ if steps.is_some_and(|steps| may_be_undone(steps, doc)) => {
            Located::nowhere(AnchorStatus::Suspended)
        }
        found => found.unwrap_or_else(|| Located::nowhere(AnchorStatus::Orphaned)),
    }
}

/// Up to [`CONTEXT`] code points of whitespace-collapsed source on each side of `bytes`.
pub(crate) fn context(text: &str, bytes: &Range<usize>) -> (String, String) {
    let mut pre: Vec<char> = spaced(text[..bytes.start].chars().rev())
        .take(CONTEXT)
        .collect();
    pre.reverse();
    let suf = spaced(text[bytes.end..].chars()).take(CONTEXT).collect();
    (pre.into_iter().collect(), suf)
}

/// Where the chain puts a span of old lines.
#[derive(Clone, Copy, Debug)]
enum Mapped {
    /// No step touched the span; these are its lines now.
    Exact(u32, u32),
    /// A step changed the span; these lines cover what replaced it.
    Near(u32, u32),
    /// A step has no hunks, or the chain doesn't join the two revs.
    Unknown,
}

fn map_span(steps: &[Step], (mut lo, mut hi): (u32, u32)) -> Mapped {
    let mut exact = true;
    for step in steps {
        let Some(hunks) = &step.hunks else {
            return Mapped::Unknown;
        };
        let (mut new_lo, mut new_hi) = (None, None);
        let mut shift = 0i64;
        for hunk in hunks {
            let (a, d, i) = (i64::from(hunk.a), hunk.d.len() as i64, hunk.i.len() as i64);
            let (lo_, hi_) = (i64::from(lo), i64::from(hi));
            exact &= !(d > 0 && a < hi_ && a + d >= lo_);
            // Old lines a+1..=a+d became new lines at+1..=at+i
            let at = a + shift;
            if new_lo.is_none() {
                if lo_ <= a {
                    new_lo = Some(lo_ + shift);
                } else if lo_ <= a + d {
                    new_lo = Some(at + 1);
                }
            }
            if new_hi.is_none() {
                if hi_ <= a {
                    new_hi = Some(hi_ + shift);
                } else if hi_ <= a + d {
                    new_hi = Some(at + i);
                }
            }
            shift += i - d;
        }
        let lo_ = new_lo.unwrap_or(i64::from(lo) + shift).max(1);
        let hi_ = new_hi.unwrap_or(i64::from(hi) + shift).max(lo_);
        (lo, hi) = (lo_ as u32, hi_ as u32);
    }
    if exact {
        Mapped::Exact(lo, hi)
    } else {
        Mapped::Near(lo, hi)
    }
}

fn place_quote(anchor: &Anchor, doc: &Doc, span: (u32, u32), mapped: Mapped) -> Option<Located> {
    let search = doc.search();
    let score = |hit: &Range<usize>| context_score(&doc.text, hit, &anchor.pre, &anchor.suf);

    if let Mapped::Exact(lo, hi) = mapped
        && let Some(within) = line_bytes(doc, lo, hi)
    {
        let best = search
            .hits(&anchor.quote, within)
            .into_iter()
            .filter(|hit| lines_of(doc, hit) == (lo, hi))
            .rev()
            .max_by_key(|hit| {
                let (pre, suf) = score(hit);
                pre + suf
            });
        if let Some(hit) = best {
            let status = if (lo, hi) == span {
                AnchorStatus::Ok
            } else {
                AnchorStatus::Moved
            };
            return Some(located(doc, status, hit));
        }
    }

    let near = mapped.near().unwrap_or(span);
    let scored: Vec<(Range<usize>, usize, usize)> = search
        .hits(&anchor.quote, 0..doc.text.len())
        .into_iter()
        .map(|hit| {
            let (pre, suf) = score(&hit);
            (hit, pre, suf)
        })
        .collect();
    if let Some(best) = scored.iter().map(|(_, pre, suf)| pre + suf).max() {
        let top: Vec<&(Range<usize>, usize, usize)> = scored
            .iter()
            .filter(|(_, pre, suf)| pre + suf == best)
            .collect();
        if let [(hit, pre, suf)] = top[..] {
            let line = doc.lines.line_of(hit.start);
            let sure = (*pre).max(*suf) >= EVIDENCE || same_section(doc, near.0, line);
            let status = if sure {
                AnchorStatus::Moved
            } else {
                AnchorStatus::Fuzzy
            };
            return Some(located(doc, status, hit.clone()));
        }
        let (hit, _, _) = top
            .into_iter()
            .min_by_key(|(hit, _, _)| distance(doc.lines.line_of(hit.start), near))?;
        return Some(located(doc, AnchorStatus::Fuzzy, hit.clone()));
    }

    let bytes = fuzzy_window(anchor, doc, near, span, false)?;
    Some(located(doc, AnchorStatus::Fuzzy, bytes))
}

/// Block and section anchors follow a stop: its first 60 code points, or a heading's text.
fn place_block(anchor: &Anchor, doc: &Doc, span: (u32, u32), mapped: Mapped) -> Option<Located> {
    let search = doc.search();
    let stops = &doc.blocks.stops;
    let section = anchor.scope == Scope::Section;
    let heading = section || anchor.quote.starts_with('#');
    let want = if heading {
        heading_text(&anchor.quote)
    } else {
        trimmed(&anchor.quote)
    };
    if want.is_empty() {
        return None;
    }
    let fits = |i: usize| {
        let stop = &stops[i];
        let source = &doc.text[search.stop_bytes[i].clone()];
        (!section || stop.section.is_some())
            && if heading {
                stop.kind == StopKind::Heading && heading_text(source) == want
            } else {
                starts_with_spaced(source, &want)
            }
    };

    if let Mapped::Exact(lo, _) = mapped
        && let Some(i) = (0..stops.len()).find(|&i| stops[i].lines.0 == lo && fits(i))
    {
        // The block may have grown or shrunk; only where it starts says it moved
        let status = if lo == span.0 {
            AnchorStatus::Ok
        } else {
            AnchorStatus::Moved
        };
        return Some(at_stop(doc, i, section, status));
    }

    let near = mapped.near().unwrap_or(span);
    let found: Vec<usize> = (0..stops.len()).filter(|&i| fits(i)).collect();
    match found[..] {
        [] => {}
        // A whole block's opening, or a heading's text, is evidence enough
        [i] => return Some(at_stop(doc, i, section, AnchorStatus::Moved)),
        _ => {
            let i = found
                .into_iter()
                .min_by_key(|&i| distance(stops[i].lines.0, near))?;
            return Some(at_stop(doc, i, section, AnchorStatus::Fuzzy));
        }
    }

    let bytes = fuzzy_window(anchor, doc, near, span, !heading)?;
    let i = innermost(stops, doc.lines.line_of(bytes.start))?;
    (!section || stops[i].section.is_some()).then(|| at_stop(doc, i, section, AnchorStatus::Fuzzy))
}

/// The best window of whole lines near `near` whose text is like the quote.
/// `prefix` compares only as much of each window as the quote is long.
fn fuzzy_window(
    anchor: &Anchor,
    doc: &Doc,
    near: (u32, u32),
    span: (u32, u32),
    prefix: bool,
) -> Option<Range<usize>> {
    let quote = trimmed(&anchor.quote);
    let q_len = quote.chars().count();
    if q_len == 0 {
        return None;
    }
    let count = doc.lines.count();
    let from = near.0.saturating_sub(FUZZY_REACH).max(1);
    let to = (near.1 + FUZZY_REACH).min(count).min(from + FUZZY_LINES);
    let size = span.1 - span.0 + 1;
    let mut best: Option<(f32, u32, Range<usize>)> = None;
    for start in from..=to {
        for size in size.saturating_sub(1).max(1)..=size + 1 {
            let end = start + size - 1;
            let (Some(a), Some(b)) = (doc.lines.range(start), doc.lines.range(end)) else {
                break;
            };
            let bytes = trim_bytes(&doc.text, a.start..b.end);
            if bytes.is_empty() {
                continue;
            }
            let mut window = trimmed(&doc.text[bytes.clone()]);
            if prefix {
                window = window.chars().take(q_len).collect();
            }
            let w_len = window.chars().count();
            // The ratio can't reach the bar when the lengths are too far apart
            if (2 * q_len.min(w_len)) as f32 / ((q_len + w_len) as f32) < FUZZY_RATIO {
                continue;
            }
            let ratio = TextDiff::from_chars(quote.as_str(), window.as_str()).ratio();
            let dist = distance(start, near);
            let better = best
                .as_ref()
                .is_none_or(|(r, d, _)| ratio > *r || (ratio == *r && dist < *d));
            if ratio >= FUZZY_RATIO && better {
                best = Some((ratio, dist, bytes));
            }
        }
    }
    best.map(|(_, _, bytes)| bytes)
}

/// Whether the write that made the head removed text that may come back: one from
/// outside birta and any agent's turn, made by git or replacing most of the doc.
fn may_be_undone(steps: &[Step], doc: &Doc) -> bool {
    let Some(last) = steps.last() else {
        return false;
    };
    let Cause::Outside { git } = last.cause else {
        return false;
    };
    git || last.hunks.as_ref().is_some_and(|hunks| {
        let deleted: usize = hunks.iter().map(|h| h.d.len()).sum();
        let inserted: usize = hunks.iter().map(|h| h.i.len()).sum();
        let old = (doc.lines.count() as usize + deleted).saturating_sub(inserted);
        old > 0 && deleted as f64 > REWRITE_SHARE * old as f64
    })
}

impl Mapped {
    fn near(self) -> Option<(u32, u32)> {
        match self {
            Self::Exact(lo, hi) | Self::Near(lo, hi) => Some((lo, hi)),
            Self::Unknown => None,
        }
    }
}

fn located(doc: &Doc, status: AnchorStatus, bytes: Range<usize>) -> Located {
    let lines = lines_of(doc, &bytes);
    Located {
        status,
        lines: Some(lines),
        bytes: Some(bytes),
        block: innermost(&doc.blocks.stops, lines.0).map(|i| doc.blocks.stops[i].lines),
        section: section_at(doc, lines.0),
    }
}

fn at_stop(doc: &Doc, i: usize, section: bool, status: AnchorStatus) -> Located {
    let stop = &doc.blocks.stops[i];
    let bytes = doc.search().stop_bytes[i].clone();
    let (lines, bytes) = match stop.section.filter(|_| section) {
        Some((a, b)) => {
            let end = doc.lines.range(b).map_or(bytes.end, |r| r.end);
            ((a, b), bytes.start..end.max(bytes.end))
        }
        None => (stop.lines, bytes),
    };
    Located {
        status,
        lines: Some(lines),
        bytes: Some(bytes),
        block: Some(stop.lines),
        section: section_at(doc, stop.lines.0),
    }
}

fn lines_of(doc: &Doc, bytes: &Range<usize>) -> (u32, u32) {
    let last = bytes.end.max(bytes.start + 1) - 1;
    (doc.lines.line_of(bytes.start), doc.lines.line_of(last))
}

/// Bytes of lines `lo..=hi`, from the first's start to the last's end.
fn line_bytes(doc: &Doc, lo: u32, hi: u32) -> Option<Range<usize>> {
    Some(doc.lines.range(lo)?.start..doc.lines.range(hi)?.end)
}

fn distance(line: u32, (lo, hi): (u32, u32)) -> u32 {
    lo.saturating_sub(line).max(line.saturating_sub(hi))
}

/// The last-starting stop holding `line`, which is the innermost one.
fn innermost(stops: &[Stop], line: u32) -> Option<usize> {
    let before = stops.partition_point(|s| s.lines.0 <= line);
    (0..before).rev().find(|&i| stops[i].lines.1 >= line)
}

/// The innermost heading whose section holds `line`, as a stop index.
fn section_stop(stops: &[Stop], line: u32) -> Option<usize> {
    let before = stops.partition_point(|s| s.lines.0 <= line);
    (0..before)
        .rev()
        .find(|&i| stops[i].section.is_some_and(|(_, end)| end >= line))
}

fn section_at(doc: &Doc, line: u32) -> Option<String> {
    let i = section_stop(&doc.blocks.stops, line)?;
    doc.search().headings.get(&doc.blocks.stops[i].sp).cloned()
}

fn same_section(doc: &Doc, a: u32, b: u32) -> bool {
    let stops = &doc.blocks.stops;
    section_stop(stops, a).is_some_and(|i| section_stop(stops, b) == Some(i))
}

/// How many code points of `pre` end right before `hit`, and of `suf` start right after.
fn context_score(text: &str, hit: &Range<usize>, pre: &str, suf: &str) -> (usize, usize) {
    let pre = collapse_str(pre);
    let suf = collapse_str(suf);
    let before = spaced(text[..hit.start].chars().rev())
        .zip(pre.iter().rev())
        .take_while(|(a, b)| a == *b)
        .count();
    let after = spaced(text[hit.end..].chars())
        .zip(suf.iter())
        .take_while(|(a, b)| a == *b)
        .count();
    (before, after)
}

/// `chars` with each whitespace run as one space.
fn spaced(chars: impl Iterator<Item = char>) -> impl Iterator<Item = char> {
    let mut in_run = false;
    chars.filter_map(move |c| {
        let space = is_js_space(c);
        let skip = space && in_run;
        in_run = space;
        (!skip).then_some(if space { ' ' } else { c })
    })
}

/// `s` with whitespace runs as one space and none at either end.
fn trimmed(s: &str) -> String {
    spaced(s.trim_matches(is_js_space).chars()).collect()
}

fn starts_with_spaced(source: &str, want: &str) -> bool {
    let mut have = spaced(source.trim_start_matches(is_js_space).chars());
    want.chars().all(|c| have.next() == Some(c))
}

/// A heading's text without its `#` markers or setext underline.
fn heading_text(source: &str) -> String {
    let source = source.trim_matches(is_js_space);
    let source = match source.rsplit_once(['\n', '\r']) {
        Some((text, underline)) if underline.trim().chars().all(|c| c == '=' || c == '-') => text,
        _ => source,
    };
    let text = source.trim_start_matches('#').trim_matches(is_js_space);
    let text = match text.trim_end_matches('#') {
        closed if closed.ends_with(is_js_space) => closed,
        _ => text,
    };
    trimmed(text)
}

fn trim_bytes(text: &str, bytes: Range<usize>) -> Range<usize> {
    let s = &text[bytes.clone()];
    let start = bytes.start + (s.len() - s.trim_start_matches(is_js_space).len());
    let end = bytes.start + s.trim_end_matches(is_js_space).len();
    start..end.max(start)
}

/// A doc's text prepared for re-locating anchors, built once per doc.
pub(crate) struct Search {
    /// Prose matches with whitespace runs collapsed; code only with line endings as `\n`.
    prose: Folded,
    code: Folded,
    code_bytes: Vec<Range<usize>>,
    /// Source bytes of each stop, in stop order.
    stop_bytes: Vec<Range<usize>>,
    /// Rendered heading texts by sourcepos.
    headings: HashMap<String, String>,
}

impl Search {
    pub(crate) fn new(doc: &Doc) -> Self {
        let text = &doc.text;
        let stops = &doc.blocks.stops;
        let stop_bytes = anchor::sp_bytes(text, stops.iter().map(|s| s.sp.as_str()))
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect();
        let mut code_bytes: Vec<Range<usize>> = doc
            .blocks
            .owners
            .iter()
            .filter(|o| o.kind == OwnerKind::Code)
            .map(|o| o.bytes.clone())
            .collect();
        code_bytes.sort_by_key(|r| r.start);
        let has_headings = stops.iter().any(|s| s.kind == StopKind::Heading);
        Self {
            prose: Folded::new(text, true),
            code: Folded::new(text, false),
            code_bytes,
            stop_bytes,
            headings: if has_headings {
                anchor::heading_texts(text)
            } else {
                HashMap::new()
            },
        }
    }

    /// Source bytes inside `within` that match `quote`, in order. Prose compares with
    /// whitespace collapsed; code keeps its whitespace.
    fn hits(&self, quote: &str, within: Range<usize>) -> Vec<Range<usize>> {
        let mut found = Vec::new();
        for (folded, code) in [(&self.prose, false), (&self.code, true)] {
            let needle: String = if code {
                eol_folded(quote)
            } else {
                collapse_str(quote).into_iter().collect()
            };
            if needle.chars().all(is_js_space) {
                continue;
            }
            let (lo, hi) = (folded.folded(within.start), folded.folded(within.end));
            for (i, _) in folded.text[lo..hi].match_indices(&needle) {
                let start = folded.at[lo + i];
                let end = folded.at[lo + i + needle.len()];
                if end <= within.end && self.in_code(start) == code {
                    found.push(start..end);
                }
            }
        }
        found.sort_by_key(|r| (r.start, r.end));
        found.dedup();
        found
    }

    fn in_code(&self, byte: usize) -> bool {
        let i = self.code_bytes.partition_point(|r| r.start <= byte);
        i > 0 && self.code_bytes[i - 1].contains(&byte)
    }
}

/// A folded copy of a text that maps each of its bytes back to the source.
struct Folded {
    text: String,
    /// Source offset of the char each byte of `text` belongs to, plus the source length.
    at: Vec<usize>,
}

impl Folded {
    /// `prose` collapses whitespace runs to one space; otherwise `\r\n` and `\r` become `\n`.
    fn new(source: &str, prose: bool) -> Self {
        let mut text = String::with_capacity(source.len());
        let mut at = Vec::with_capacity(source.len() + 1);
        let mut chars = source.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let c = if prose && is_js_space(c) {
                if text.ends_with(' ') {
                    continue;
                }
                ' '
            } else if !prose && c == '\r' {
                chars.next_if(|&(_, next)| next == '\n');
                '\n'
            } else {
                c
            };
            text.push(c);
            at.extend(std::iter::repeat_n(i, c.len_utf8()));
        }
        at.push(source.len());
        Self { text, at }
    }

    /// The first byte of `text` at or after source byte `byte`.
    fn folded(&self, byte: usize) -> usize {
        self.at.partition_point(|&at| at < byte)
    }
}

fn eol_folded(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}
