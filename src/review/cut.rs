//! A write's hunks cut where blocks meet, so that each change is one block's edit, or one
//! run of blocks with no counterpart. Only a write being recorded is cut: hunks already
//! in a log keep their ids.

use std::collections::HashSet;

use super::doc::Doc;
use super::hunk;
use super::ops::Hunk;
use crate::anchor::StopKind;

/// Past this many block pairings a hunk stays whole.
const MAX_PAIRINGS: usize = 10_000;
/// Words of its own an unpaired block must give its neighbour's pair to join it.
const FED_WORDS: usize = 3;

/// The hunks that turn `old` into `new`, cut at block boundaries, with removals cut again
/// before each heading they hold.
pub fn diff(old: &str, new: &str) -> Vec<Hunk> {
    let hunks = hunk::diff_anchored(old, new);
    // Only a hunk that both removes and adds lines can pair blocks
    let mixed = hunks.iter().any(|h| !h.d.is_empty() && !h.i.is_empty());
    let removal = hunks.iter().any(|h| h.i.is_empty() && h.d.len() > 1);
    if !mixed && !removal {
        return hunks;
    }
    let before = Doc::new(old);
    let mut out = if mixed {
        let after = Doc::new(new);
        let (was, now) = (Blocks::new(&before), Blocks::new(&after));
        let mut out = Vec::with_capacity(hunks.len());
        let mut shift = 0i64;
        for h in &hunks {
            let new_a = (i64::from(h.a) + shift) as u32;
            shift += h.i.len() as i64 - h.d.len() as i64;
            out.extend(cut(h, new_a, &was, &now));
        }
        out
    } else {
        hunks.clone()
    };
    out = at_headings(out, &before);
    // The pieces must rebuild the text exactly; anything else keeps the plain hunks
    if hunk::apply(old, &out).ok().as_deref() == Some(new) {
        out
    } else {
        hunks
    }
}

/// Each removal cut right before every heading it holds, so a cut section is a change of
/// its own. Blank lines above a heading stay with it.
fn at_headings(hunks: Vec<Hunk>, before: &Doc) -> Vec<Hunk> {
    let headings: HashSet<u32> = before
        .blocks
        .stops
        .iter()
        .filter(|s| s.kind == StopKind::Heading)
        .map(|s| s.lines.0)
        .collect();
    let mut out = Vec::with_capacity(hunks.len());
    for h in hunks {
        if !h.i.is_empty() {
            out.push(h);
            continue;
        }
        let mut start = 0;
        for k in 1..h.d.len() {
            let heading = headings.contains(&(h.a + k as u32 + 1));
            if heading && h.d[start..k].iter().any(|l| !l.trim().is_empty()) {
                out.push(Hunk {
                    a: h.a + start as u32,
                    d: h.d[start..k].to_vec(),
                    i: Vec::new(),
                });
                start = k;
            }
        }
        out.push(Hunk {
            a: h.a + start as u32,
            d: h.d[start..].to_vec(),
            i: Vec::new(),
        });
    }
    out
}

/// The words of markdown source that a reader sees: list, task, quote and heading
/// markers left out, and only tokens holding a letter or digit.
pub fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for line in text.lines() {
        out.extend(
            strip_markers(line)
                .split_whitespace()
                .filter(|w| w.chars().any(char::is_alphanumeric)),
        );
    }
    out
}

fn strip_markers(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        let before = rest;
        if let Some(r) = rest.strip_prefix('>') {
            rest = r.trim_start();
        }
        rest = strip_list_marker(rest);
        for task in ["[ ]", "[x]", "[X]"] {
            if let Some(r) = rest.strip_prefix(task)
                && (r.is_empty() || r.starts_with(char::is_whitespace))
            {
                rest = r.trim_start();
            }
        }
        let hashes = rest.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) {
            let r = &rest[hashes..];
            if r.is_empty() || r.starts_with(char::is_whitespace) {
                rest = r.trim_start();
            }
        }
        if rest == before {
            return rest;
        }
    }
}

fn strip_list_marker(s: &str) -> &str {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    let marker = match s.as_bytes().first() {
        Some(b'-' | b'*' | b'+') => 1,
        _ if (1..=9).contains(&digits) && matches!(s.as_bytes().get(digits), Some(b'.' | b')')) => {
            digits + 1
        }
        _ => return s,
    };
    let r = &s[marker..];
    if r.is_empty() || r.starts_with(char::is_whitespace) {
        r.trim_start()
    } else {
        s
    }
}

/// Each line's innermost stop, for one text.
struct Blocks<'d> {
    doc: &'d Doc,
    /// By 0-based line.
    inner: Vec<Option<usize>>,
}

impl<'d> Blocks<'d> {
    fn new(doc: &'d Doc) -> Self {
        let mut inner = vec![None; doc.lines.count() as usize];
        // Stops come parent first, so an inner stop overwrites its parent's lines
        for (i, s) in doc.blocks.stops.iter().enumerate() {
            for line in s.lines.0..=s.lines.1 {
                if let Some(slot) = (line as usize)
                    .checked_sub(1)
                    .and_then(|l| inner.get_mut(l))
                {
                    *slot = Some(i);
                }
            }
        }
        Self { doc, inner }
    }

    fn text(&self, line: u32) -> &str {
        self.doc.lines.range(line).map_or("", |r| &self.doc.text[r])
    }

    /// The blocks with a line in `first..=last`, in order; blank lines outside any block
    /// belong to none.
    fn segs(&self, first: u32, last: u32) -> Vec<Seg> {
        let mut segs: Vec<Seg> = Vec::new();
        for line in first..=last {
            let stop = self.inner.get(line as usize - 1).copied().flatten();
            let key = match stop {
                Some(s) => Key::Stop(s),
                None if self.text(line).trim().is_empty() => continue,
                None => Key::Other,
            };
            match segs.last_mut() {
                Some(seg) if seg.key == key && (stop.is_some() || seg.lines.1 + 1 == line) => {
                    seg.lines.1 = line;
                }
                _ => segs.push(Seg {
                    key,
                    kind: stop.map(|s| self.doc.blocks.stops[s].kind.clone()),
                    lines: (line, line),
                    words: HashSet::new(),
                }),
            }
        }
        for seg in &mut segs {
            let text: Vec<&str> = (seg.lines.0..=seg.lines.1).map(|l| self.text(l)).collect();
            seg.words = words(&text.join("\n"))
                .into_iter()
                .map(|w| {
                    w.trim_matches(|c: char| !c.is_alphanumeric())
                        .to_lowercase()
                })
                .collect();
        }
        segs
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Stop(usize),
    /// Lines no stop holds, such as a table's header.
    Other,
}

struct Seg {
    key: Key,
    kind: Option<StopKind>,
    lines: (u32, u32),
    words: HashSet<String>,
}

impl Seg {
    /// How much of this block's wording `other` shares, from 0 to 1.
    fn likeness(&self, other: &Seg) -> f64 {
        let shared = self.words.intersection(&other.words).count();
        shared as f64 / self.words.len().max(other.words.len()).max(1) as f64
    }

    /// Most of this block's words are in `other`, as when blocks are merged or split.
    fn within(&self, other: &Seg) -> bool {
        let shared = self.words.intersection(&other.words).count();
        !self.words.is_empty() && shared * 2 > self.words.len()
    }

    /// Of this block's words that `beside` lacks, enough read in `into` that the two
    /// were merged into it, or split from it, with the wording reworked.
    fn feeds(&self, into: &Seg, beside: &Seg) -> bool {
        let own: Vec<&String> = self.words.difference(&beside.words).collect();
        let kept = own.iter().filter(|w| into.words.contains(**w)).count();
        kept >= FED_WORDS && kept * 4 >= own.len()
    }

    /// This block, of `beside`'s kind, joins the pair of `beside` and `into`.
    fn joins(&self, into: &Seg, beside: &Seg) -> bool {
        self.kind == beside.kind && (self.within(into) || self.feeds(into, beside))
    }
}

/// A run of old blocks and the new blocks that took their place, by index.
#[derive(Clone, Copy)]
struct Piece {
    old: (usize, usize),
    new: (usize, usize),
}

impl Piece {
    fn pure(&self) -> bool {
        self.old.0 == self.old.1 || self.new.0 == self.new.1
    }
}

fn cut(h: &Hunk, new_a: u32, was: &Blocks, now: &Blocks) -> Vec<Hunk> {
    let whole = || vec![h.clone()];
    let (dl, il) = (h.d.len() as u32, h.i.len() as u32);
    if dl == 0 || il == 0 {
        return whole();
    }
    let old = was.segs(h.a + 1, h.a + dl);
    let new = now.segs(new_a + 1, new_a + il);
    if old.len() + new.len() < 3 || old.len() * new.len() > MAX_PAIRINGS {
        return whole();
    }
    let pieces = pieces(&old, &new);
    if pieces.len() < 2 {
        return whole();
    }
    let old_ends = bounds(&pieces, &old, |p| p.old, (h.a, h.a + dl));
    let new_ends = bounds(&pieces, &new, |p| p.new, (new_a, new_a + il));
    let (mut o, mut n) = (h.a, new_a);
    let mut out = Vec::with_capacity(pieces.len());
    for (oe, ne) in old_ends.into_iter().zip(new_ends) {
        let d = h.d[(o - h.a) as usize..(oe - h.a) as usize].to_vec();
        let i = h.i[(n - new_a) as usize..(ne - new_a) as usize].to_vec();
        if !d.is_empty() || !i.is_empty() {
            out.push(Hunk { a: o, d, i });
        }
        (o, n) = (oe, ne);
    }
    out
}

/// Old and new blocks paired in order, kind with kind, the likest first; merged or
/// split blocks join their pair, and blocks left between pairs make one piece.
fn pieces(old: &[Seg], new: &[Seg]) -> Vec<Piece> {
    let (n, m) = (old.len(), new.len());
    let score = |i: usize, j: usize| {
        (old[i].kind == new[j].kind).then(|| 1000 + (old[i].likeness(&new[j]) * 1000.0) as u32)
    };
    let mut best = vec![vec![0u32; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            let paired = score(i - 1, j - 1).map_or(0, |s| best[i - 1][j - 1] + s);
            best[i][j] = best[i - 1][j].max(best[i][j - 1]).max(paired);
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        if score(i - 1, j - 1).is_some_and(|s| best[i][j] == best[i - 1][j - 1] + s) {
            pairs.push((i - 1, j - 1));
            (i, j) = (i - 1, j - 1);
        } else if best[i][j] == best[i - 1][j] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();

    let mut out: Vec<Piece> = Vec::new();
    let (mut oi, mut nj) = (0, 0);
    for k in 0..=pairs.len() {
        let next = pairs.get(k).copied();
        let (oe, ne) = next.unwrap_or((n, m));
        let (mut os, mut ns) = (oi, nj);
        let (mut oend, mut nend) = (oe, ne);
        if let Some(prev) = out.last_mut().filter(|_| k > 0) {
            let (pi, pj) = pairs[k - 1];
            while os < oend && old[os].joins(&new[pj], &old[pi]) {
                os += 1;
            }
            while ns < nend && new[ns].joins(&old[pi], &new[pj]) {
                ns += 1;
            }
            prev.old.1 = os;
            prev.new.1 = ns;
        }
        if let Some((pi, pj)) = next {
            while oend > os && old[oend - 1].joins(&new[pj], &old[pi]) {
                oend -= 1;
            }
            while nend > ns && new[nend - 1].joins(&old[pi], &new[pj]) {
                nend -= 1;
            }
        }
        if os < oend || ns < nend {
            out.push(Piece {
                old: (os, oend),
                new: (ns, nend),
            });
        }
        if let Some((pi, pj)) = next {
            out.push(Piece {
                old: (oend, pi + 1),
                new: (nend, pj + 1),
            });
            (oi, nj) = (pi + 1, pj + 1);
        }
    }
    out
}

/// Where each piece ends on one side, as a 0-based line past its last. Lines between
/// blocks go to a piece with nothing on the other side, else to the earlier piece.
fn bounds(
    pieces: &[Piece],
    segs: &[Seg],
    side: impl Fn(&Piece) -> (usize, usize),
    (start, end): (u32, u32),
) -> Vec<u32> {
    let mut ends = vec![start; pieces.len()];
    let holding: Vec<usize> = (0..pieces.len())
        .filter(|&p| side(&pieces[p]).0 < side(&pieces[p]).1)
        .collect();
    for (k, &p) in holding.iter().enumerate() {
        let (_, last) = side(&pieces[p]);
        let block_end = segs[last - 1].lines.1;
        let Some(&q) = holding.get(k + 1) else {
            for e in &mut ends[p..] {
                *e = end;
            }
            break;
        };
        let next_start = segs[side(&pieces[q]).0].lines.0 - 1;
        let to_later = !pieces[p].pure() && pieces[q].pure();
        let at = if to_later { block_end } else { next_start };
        for e in &mut ends[p..q] {
            *e = at;
        }
    }
    ends
}
