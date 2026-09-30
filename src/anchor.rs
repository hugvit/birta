//! Where rendered blocks come from in the source: focus stops and text owners, and the
//! mapping between a selection's rendered text and the source bytes behind it.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use comrak::arena_tree::NodeEdge;
use comrak::nodes::{AstNode, LineColumn, NodeCode, NodeCodeBlock, NodeValue, Sourcepos};
use comrak::{Arena, Options, parse_document};
use serde::Serialize;

/// The comrak options every parse of a doc uses, so sourcepos agree with the page.
/// `render` adds its URL rewriters on top; they don't change the tree.
pub fn parse_options() -> Options<'static> {
    let mut options = Options::default();

    // GFM extensions
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.description_lists = true;
    options.extension.shortcodes = true;
    options.extension.header_ids = Some(String::new());
    options.extension.alerts = true;
    options.extension.math_dollars = true;
    options.extension.math_code = true;

    // Parsing
    options.parse.smart = true;

    // Rendering
    options.render.github_pre_lang = true;
    options.render.r#unsafe = true;
    options.render.sourcepos = true;

    options
}

/// A block the reader can focus and give a verdict, in document order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Stop {
    /// Ordinal from 0, in document order.
    pub n: u32,
    pub kind: StopKind,
    /// The `data-sourcepos` of the block's element.
    pub sp: String,
    pub lines: (u32, u32),
    /// Heading level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// Lines a top-level heading's verdict covers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    ListItem,
    TableRow,
    Heading,
    Paragraph,
    CodeBlock,
}

/// An element whose inline text a selection on the page belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    /// The `data-sourcepos` of the element, exactly as rendered.
    pub sp: String,
    pub kind: OwnerKind,
    pub lines: (u32, u32),
    /// Source bytes the sourcepos spans.
    pub bytes: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    Paragraph,
    Item,
    Heading,
    Cell,
    Term,
    Detail,
    Code,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockIndex {
    pub stops: Vec<Stop>,
    pub owners: Vec<Owner>,
}

/// Indexes `root`, parsed from `markdown` with [`parse_options`].
pub fn index<'a>(root: &'a AstNode<'a>, markdown: &str) -> BlockIndex {
    let lines = LineMap::new(markdown);
    let mut idx = BlockIndex::default();
    // Top-level headings as (stop index, level), for sections
    let mut headings = Vec::new();
    // Footnotes render after the doc, so nothing in them is a stop
    let mut footnote_depth = 0usize;

    for edge in root.traverse() {
        let node = match edge {
            NodeEdge::Start(node) => node,
            NodeEdge::End(node) => {
                if matches!(node.data.borrow().value, NodeValue::FootnoteDefinition(_)) {
                    footnote_depth -= 1;
                }
                continue;
            }
        };
        if matches!(node.data.borrow().value, NodeValue::FootnoteDefinition(_)) {
            footnote_depth += 1;
            continue;
        }
        let Some((stop, owner)) = roles(node) else {
            continue;
        };
        let ast = node.data.borrow();
        let parent_ast = node.parent().map(|p| p.data.borrow());
        let parent = parent_ast.as_deref().map(|p| &p.value);

        let sp = ast.sourcepos.to_string();
        let span = line_span(ast.sourcepos);
        if let Some(kind) = stop.filter(|_| footnote_depth == 0) {
            let level = match &ast.value {
                NodeValue::Heading(h) => Some(h.level),
                _ => None,
            };
            if let (Some(level), Some(NodeValue::Document)) = (level, parent) {
                headings.push((idx.stops.len(), level));
            }
            idx.stops.push(Stop {
                n: idx.stops.len() as u32,
                kind,
                sp: sp.clone(),
                lines: span,
                level,
                section: None,
            });
        }
        if let Some(kind) = owner {
            idx.owners.push(Owner {
                sp,
                kind,
                lines: span,
                bytes: lines.bytes(ast.sourcepos),
            });
        }
    }

    set_sections(&mut idx.stops, &headings, lines.count());
    idx
}

/// What `node` is on the page: a focus stop, a text owner, both, or `None` for neither.
fn roles<'a>(node: &'a AstNode<'a>) -> Option<(Option<StopKind>, Option<OwnerKind>)> {
    let ast = node.data.borrow();
    let parent_ast = node.parent().map(|p| p.data.borrow());
    let parent = parent_ast.as_deref().map(|p| &p.value);
    Some(match &ast.value {
        NodeValue::Item(_) | NodeValue::TaskItem(_) => {
            let tight = matches!(parent, Some(NodeValue::List(list)) if list.tight);
            (Some(StopKind::ListItem), tight.then_some(OwnerKind::Item))
        }
        NodeValue::TableRow(header) => ((!header).then_some(StopKind::TableRow), None),
        NodeValue::TableCell => (None, Some(OwnerKind::Cell)),
        NodeValue::Heading(_) => (Some(StopKind::Heading), Some(OwnerKind::Heading)),
        NodeValue::CodeBlock(_) => (Some(StopKind::CodeBlock), Some(OwnerKind::Code)),
        NodeValue::DescriptionTerm => (None, Some(OwnerKind::Term)),
        NodeValue::DescriptionDetails => {
            let tight = matches!(parent, Some(NodeValue::DescriptionItem(item)) if item.tight);
            (None, tight.then_some(OwnerKind::Detail))
        }
        NodeValue::Paragraph => {
            let in_item = matches!(
                parent,
                Some(
                    NodeValue::Item(_)
                        | NodeValue::TaskItem(_)
                        | NodeValue::DescriptionTerm
                        | NodeValue::DescriptionDetails
                )
            );
            let stop = (!in_item).then_some(StopKind::Paragraph);
            (stop, (!renders_bare(node)).then_some(OwnerKind::Paragraph))
        }
        _ => return None,
    })
}

/// Whether comrak renders `paragraph` without its own `<p>`, as the text of its parent.
fn renders_bare<'a>(paragraph: &'a AstNode<'a>) -> bool {
    let Some(parent) = paragraph.parent() else {
        return false;
    };
    if matches!(parent.data.borrow().value, NodeValue::DescriptionTerm) {
        return true;
    }
    parent
        .parent()
        .is_some_and(|grand| match &grand.data.borrow().value {
            NodeValue::List(list) => list.tight,
            NodeValue::DescriptionItem(item) => item.tight,
            _ => false,
        })
}

/// A heading's section ends before the next top-level heading of its level or above.
fn set_sections(stops: &mut [Stop], headings: &[(usize, u8)], last: u32) {
    let mut open: Vec<(usize, u8)> = Vec::new();
    for &(i, level) in headings {
        let start = stops[i].lines.0;
        while let Some(&(j, _)) = open.last().filter(|(_, l)| *l >= level) {
            stops[j].section = Some((stops[j].lines.0, start - 1));
            open.pop();
        }
        open.push((i, level));
    }
    for (j, _) in open {
        stops[j].section = Some((stops[j].lines.0, last.max(stops[j].lines.0)));
    }
}

/// First and last line of `sp`; an end at column 0 means the line before ended the node.
fn line_span(sp: Sourcepos) -> (u32, u32) {
    let (start, end) = (sp.start.line as u32, sp.end.line as u32);
    let end = if sp.end.column == 0 {
        end.saturating_sub(1)
    } else {
        end
    };
    (start, end.max(start))
}

/// Byte offsets of line starts and ends, split the way CommonMark splits lines.
struct LineMap<'t> {
    text: &'t str,
    /// Start of each line.
    starts: Vec<usize>,
    /// End of each line, before its terminator.
    ends: Vec<usize>,
    len: usize,
}

impl<'t> LineMap<'t> {
    fn new(text: &'t str) -> Self {
        let bytes = text.as_bytes();
        let (mut starts, mut ends) = (vec![0], Vec::new());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    ends.push(i);
                    starts.push(i + 1);
                }
                b'\r' => {
                    ends.push(i);
                    if bytes.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    starts.push(i + 1);
                }
                _ => {}
            }
            i += 1;
        }
        ends.push(bytes.len());
        Self {
            text,
            starts,
            ends,
            len: bytes.len(),
        }
    }

    /// Byte offset of 1-based `line` and byte `column` (0 = before the line).
    fn offset(&self, at: LineColumn) -> usize {
        match (at.line.checked_sub(1), at.column) {
            (Some(i), 0) => i
                .checked_sub(1)
                .and_then(|prev| self.ends.get(prev))
                .copied()
                .unwrap_or(0),
            (Some(i), col) => self.starts.get(i).map_or(self.len, |s| s + col - 1),
            (None, _) => 0,
        }
        .min(self.len)
    }

    /// Start, end before the terminator, and start of the next line, for 1-based `line`.
    fn line(&self, line: usize) -> Option<(usize, usize, usize)> {
        let i = line.checked_sub(1)?;
        let (start, end) = (*self.starts.get(i)?, *self.ends.get(i)?);
        Some((
            start,
            end,
            self.starts.get(i + 1).copied().unwrap_or(self.len),
        ))
    }

    /// 1-based line holding `byte`.
    fn line_of(&self, byte: usize) -> u32 {
        self.starts.partition_point(|&s| s <= byte) as u32
    }

    /// Lines as `str::lines` counts them: a final terminator starts no new line.
    fn count(&self) -> u32 {
        let ends_open = self.starts.last() == Some(&self.len) && self.len > 0;
        (self.starts.len() - usize::from(ends_open)) as u32
    }

    fn bytes(&self, sp: Sourcepos) -> Range<usize> {
        let raw = self.raw_bytes(sp);
        // comrak's sp can end mid-character (a `dt` is always `L:1-L:1`)
        self.snap_down(raw.start)..self.snap_up(raw.end)
    }

    /// `sp` as byte offsets, which may fall inside a character.
    fn raw_bytes(&self, sp: Sourcepos) -> Range<usize> {
        let start = self.offset(sp.start);
        let end = match sp.end.column {
            0 => self.offset(sp.end),
            // The end column is the node's last byte, so the range ends one past it
            _ => (self.offset(sp.end) + 1).min(self.len),
        };
        start..end.max(start)
    }

    fn snap_down(&self, mut at: usize) -> usize {
        at = at.min(self.len);
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    fn snap_up(&self, mut at: usize) -> usize {
        at = at.min(self.len);
        while !self.text.is_char_boundary(at) {
            at += 1;
        }
        at
    }
}

/// A selection on the page: the text owner it starts in and its rendered text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveReq {
    /// The owner's `data-sourcepos`.
    pub sp: String,
    pub quote: String,
    /// Up to 32 code points of the owner's text before and after the quote.
    pub pre: String,
    pub suf: String,
    /// Code-point offset of the quote in the owner's text, as the page counted it.
    pub off: Option<usize>,
    /// Code-point length of the owner's text on the page.
    pub tl: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    Exact {
        lines: (u32, u32),
        bytes: Range<usize>,
        /// `markdown[bytes]`, what an agent reads.
        source: String,
        /// The quote as the user saw it.
        shown: String,
        /// The range grew to a whole token, link or emphasis.
        widened: bool,
        text_len: usize,
    },
    /// The quote occurs more than once and nothing tells the occurrences apart.
    Candidates { items: Vec<Cand>, text_len: usize },
    Block {
        reason: Reason,
        kept_quote: String,
        text_len: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// `sp` names no text owner; the caller rejects the anchor.
    BadSp,
    /// The owner is fine but the quote can't be placed in it.
    NoMatch,
}

/// One place a quote could point at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cand {
    /// The owner, or `None` for a hit in raw HTML.
    pub sp: Option<String>,
    /// Code-point offset in the owner's text.
    pub off: Option<usize>,
    pub bytes: Range<usize>,
    pub source: String,
    /// What the hit points at, for a pick list: `→ ## Heading` or `→ "…text…"`.
    pub preview: String,
}

/// Where source bytes sit on the page: an owner and a code-point range of its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Loc {
    pub sp: String,
    pub off: (usize, usize),
    pub tl: usize,
}

const MAX_CANDIDATES: usize = 5;
/// Code points of context the page sends on each side of a quote.
const CONTEXT: usize = 32;

/// Places a selection's rendered text in `markdown`.
pub fn resolve(markdown: &str, req: &ResolveReq) -> Resolved {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &parse_options());
    let lines = LineMap::new(markdown);
    let block = |reason, text_len| Resolved::Block {
        reason,
        kept_quote: req.quote.clone(),
        text_len,
    };
    let Some((node, kind)) = owners(root).find(|(n, _)| sp_of(n) == req.sp) else {
        return block(Reason::BadSp, 0);
    };
    let text = OwnerText::new(node, kind, &lines);
    let t = text.chars();
    if !text.mapped || text.diagram {
        return block(Reason::NoMatch, t.len());
    }
    let norm = |s: &str| {
        if text.code {
            s.chars().collect()
        } else {
            collapse_str(s)
        }
    };
    let q: Vec<char> = norm(&req.quote);
    if q.iter().all(|&c| is_js_space(c)) {
        return block(Reason::NoMatch, t.len());
    }

    let at_off = req.off.filter(|_| req.tl == Some(t.len())).filter(|&a| {
        a.checked_add(q.len())
            .is_some_and(|b| t.get(a..b) == Some(&q[..]))
    });
    let a = match at_off {
        Some(a) => a,
        None => {
            // The page sends at most this much context; more would only cost time
            let pre = norm(&req.pre);
            let pre = &pre[pre.len().saturating_sub(CONTEXT)..];
            let suf = norm(&req.suf);
            let suf = &suf[..suf.len().min(CONTEXT)];
            let scored: Vec<(usize, usize)> = occurrences(&t, &q)
                .into_iter()
                .map(|a| {
                    let before = t[..a].iter().rev().zip(pre.iter().rev());
                    let after = t[a + q.len()..].iter().zip(suf.iter());
                    let score = before.take_while(|(x, y)| x == y).count()
                        + after.take_while(|(x, y)| x == y).count();
                    (a, score)
                })
                .collect();
            let best = scored.iter().map(|&(_, score)| score).max().unwrap_or(0);
            let top: Vec<usize> = scored
                .into_iter()
                .filter(|&(_, score)| score == best)
                .map(|(a, _)| a)
                .collect();
            match top[..] {
                [] => return block(Reason::NoMatch, t.len()),
                [a] => a,
                _ => {
                    let items = top
                        .into_iter()
                        .take(MAX_CANDIDATES)
                        .map(|a| text.cand(node, kind, &req.sp, markdown, a, q.len()))
                        .collect();
                    return Resolved::Candidates {
                        items,
                        text_len: t.len(),
                    };
                }
            }
        }
    };
    let (bytes, widened) = text.span(a, a + q.len());
    let Some(source) = markdown.get(bytes.clone()) else {
        return block(Reason::NoMatch, t.len());
    };
    let last = bytes.end.max(bytes.start + 1) - 1;
    Resolved::Exact {
        lines: (lines.line_of(bytes.start), lines.line_of(last)),
        source: source.to_string(),
        bytes,
        shown: req.quote.clone(),
        widened,
        text_len: t.len(),
    }
}

/// Places typed text: every hit in any owner's rendered text, else in raw source.
pub fn search(markdown: &str, text: &str) -> Vec<Cand> {
    let typed = collapse_str(text);
    let q = trim_spaces(&typed);
    if q.is_empty() {
        return Vec::new();
    }
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &parse_options());
    let lines = LineMap::new(markdown);
    let mut found = Vec::new();
    for (node, kind) in owners(root) {
        let owned = OwnerText::new(node, kind, &lines);
        if !owned.mapped || owned.diagram {
            continue;
        }
        let sp = sp_of(node);
        for a in occurrences(&owned.chars(), q) {
            found.push(owned.cand(node, kind, &sp, markdown, a, q.len()));
            if found.len() == MAX_CANDIDATES {
                return found;
            }
        }
    }
    if found.is_empty() {
        // Text inside raw HTML has no owner, so only the source knows it
        let needle: String = q.iter().collect();
        found = markdown
            .match_indices(&needle)
            .take(MAX_CANDIDATES)
            .map(|(i, hit)| Cand {
                sp: None,
                off: None,
                bytes: i..i + hit.len(),
                source: hit.to_string(),
                preview: format!("→ \"{hit}\""),
            })
            .collect();
    }
    found
}

/// Where `bytes` show on the page, if they fall in one owner's text.
pub fn locate(markdown: &str, bytes: Range<usize>) -> Option<Loc> {
    if bytes.start >= bytes.end || bytes.end > markdown.len() {
        return None;
    }
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &parse_options());
    let lines = LineMap::new(markdown);
    let mut found = None;
    for (node, kind) in owners(root) {
        let owned = OwnerText::new(node, kind, &lines);
        if !owned.mapped || owned.diagram {
            continue;
        }
        let touched: Vec<usize> = (0..owned.units.len())
            .filter(|&i| {
                let u = &owned.units[i];
                u.s < u.e && u.s < bytes.end && u.e > bytes.start
            })
            .collect();
        let (Some(&first), Some(&last)) = (touched.first(), touched.last()) else {
            continue;
        };
        let node_bytes = lines.bytes(node.data.borrow().sourcepos);
        let text_bytes = owned
            .units
            .iter()
            .map(|u| u.s)
            .min()
            .unwrap_or(node_bytes.start)
            ..owned
                .units
                .iter()
                .map(|u| u.e)
                .max()
                .unwrap_or(node_bytes.end);
        let inside = bytes.start >= node_bytes.start.min(text_bytes.start)
            && bytes.end <= node_bytes.end.max(text_bytes.end);
        if found.is_some() || !inside {
            return None;
        }
        found = Some(Loc {
            sp: sp_of(node),
            off: (first, last + 1),
            tl: owned.units.len(),
        });
    }
    found
}

/// The rendered text of the owner at `sp`, whitespace collapsed as the page does.
pub fn owner_text(markdown: &str, sp: &str) -> Option<String> {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &parse_options());
    let lines = LineMap::new(markdown);
    let (node, kind) = owners(root).find(|(n, _)| sp_of(n) == sp)?;
    Some(
        OwnerText::new(node, kind, &lines)
            .chars()
            .into_iter()
            .collect(),
    )
}

/// The rendered text of every heading, by sourcepos, from one parse.
pub(crate) fn heading_texts(markdown: &str) -> HashMap<String, String> {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &parse_options());
    let lines = LineMap::new(markdown);
    owners(root)
        .filter(|(_, kind)| *kind == OwnerKind::Heading)
        .map(|(node, kind)| {
            let text = OwnerText::new(node, kind, &lines).chars().into_iter();
            (sp_of(node), text.collect())
        })
        .collect()
}

/// Source bytes of each `data-sourcepos`, counted as [`Owner::bytes`] counts them.
pub(crate) fn sp_bytes<'s>(
    markdown: &str,
    sps: impl IntoIterator<Item = &'s str>,
) -> Vec<Option<Range<usize>>> {
    let lines = LineMap::new(markdown);
    let point = |s: &str| {
        let (line, column) = s.split_once(':')?;
        Some(LineColumn {
            line: line.parse().ok()?,
            column: column.parse().ok()?,
        })
    };
    sps.into_iter()
        .map(|sp| {
            let (start, end) = sp.split_once('-')?;
            let sp = Sourcepos {
                start: point(start)?,
                end: point(end)?,
            };
            Some(lines.bytes(sp))
        })
        .collect()
}

fn owners<'a>(root: &'a AstNode<'a>) -> impl Iterator<Item = (&'a AstNode<'a>, OwnerKind)> {
    root.descendants().filter_map(|node| {
        roles(node)
            .and_then(|(_, owner)| owner)
            .map(|kind| (node, kind))
    })
}

fn sp_of<'a>(node: &'a AstNode<'a>) -> String {
    node.data.borrow().sourcepos.to_string()
}

/// One code point of rendered text and the source bytes it came from.
#[derive(Clone, Copy, Debug)]
struct Unit {
    ch: char,
    s: usize,
    e: usize,
    /// Part of a token (`&amp;`, `--`, `:tada:`, math) no range may split.
    atomic: bool,
}

impl Unit {
    fn new(ch: char, s: usize, e: usize) -> Self {
        Self {
            ch,
            s,
            e,
            atomic: false,
        }
    }

    fn atomic(ch: char, s: usize, e: usize) -> Self {
        Self {
            ch,
            s,
            e,
            atomic: true,
        }
    }
}

/// An inline container a range must not cut in half.
struct Wrap {
    link: bool,
    bytes: Range<usize>,
    /// Bytes from its first to its last unit.
    text: Option<(usize, usize)>,
}

/// An owner's text as units, whitespace collapsed like the page's.
struct OwnerText<'m> {
    units: Vec<Unit>,
    wraps: Vec<Wrap>,
    code: bool,
    /// Mermaid or math: drawn as a picture, so nothing in it is selectable.
    diagram: bool,
    /// False when some node's bytes can't be trusted, so no range in this owner can be.
    mapped: bool,
    /// Where each `\|` in a table cell starts, for comrak's shifted inline sourcepos.
    escapes: Vec<usize>,
    lines: &'m LineMap<'m>,
}

impl<'m> OwnerText<'m> {
    fn new<'a>(node: &'a AstNode<'a>, kind: OwnerKind, lines: &'m LineMap<'m>) -> Self {
        let mut text = Self {
            units: Vec::new(),
            wraps: Vec::new(),
            code: false,
            diagram: false,
            mapped: true,
            escapes: Vec::new(),
            lines,
        };
        match kind {
            OwnerKind::Code => {
                if let NodeValue::CodeBlock(block) = &node.data.borrow().value {
                    text.code = true;
                    let info = block.info.split_whitespace().next().unwrap_or("");
                    text.diagram = matches!(info, "mermaid" | "math");
                    let start = node.data.borrow().sourcepos.start.line;
                    match code_units(block, start, lines) {
                        Some(units) => text.units = units,
                        None => {
                            text.mapped = false;
                            let literal = block.literal.replace("\r\n", "\n");
                            text.units = literal.chars().map(|c| Unit::new(c, 0, 0)).collect();
                        }
                    }
                }
            }
            // These render their paragraphs' inlines without a <p>
            OwnerKind::Item | OwnerKind::Term | OwnerKind::Detail => {
                let mut units = Vec::new();
                for child in node.children() {
                    if matches!(child.data.borrow().value, NodeValue::Paragraph) {
                        let at = lines.bytes(child.data.borrow().sourcepos).start;
                        units.push(Unit::new(' ', at, at));
                        text.collect(child, &mut units);
                    }
                }
                text.units = collapse(units);
            }
            _ => {
                if kind == OwnerKind::Cell {
                    let cell = lines.bytes(node.data.borrow().sourcepos);
                    let src = lines.text.get(cell.clone()).unwrap_or("");
                    text.escapes = src
                        .match_indices("\\|")
                        .map(|(i, _)| cell.start + i)
                        .collect();
                }
                let mut units = Vec::new();
                text.collect(node, &mut units);
                text.units = collapse(units);
            }
        }
        text.mapped &= text.ordered();
        if text.mapped {
            text.wraps = std::mem::take(&mut text.wraps)
                .into_iter()
                .map(|wrap| Wrap {
                    text: text.text_span(&wrap.bytes),
                    ..wrap
                })
                .collect();
        }
        text
    }

    fn chars(&self) -> Vec<char> {
        self.units.iter().map(|u| u.ch).collect()
    }

    /// Units run forward through the source on character boundaries.
    fn ordered(&self) -> bool {
        let src = self.lines.text;
        let fits = |u: &Unit| u.s <= u.e && src.is_char_boundary(u.s) && src.is_char_boundary(u.e);
        self.units.iter().all(fits) && self.units.windows(2).all(|w| w[0].s <= w[1].s)
    }

    /// An inline node's source bytes.
    fn bytes(&self, sp: Sourcepos) -> Range<usize> {
        if self.escapes.is_empty() {
            return self.lines.bytes(sp);
        }
        let raw = self.lines.raw_bytes(sp);
        if raw.is_empty() {
            return self.lines.bytes(sp);
        }
        // comrak places a cell's inlines as if each `\|` before them were one byte
        let shift = |at: usize, on_pipe: bool| {
            self.escapes.iter().fold(at, |at, &e| {
                if e < at || (on_pipe && e == at) {
                    at + 1
                } else {
                    at
                }
            })
        };
        let start = shift(raw.start, false);
        let end = shift(raw.end - 1, true) + 1;
        self.lines.snap_down(start)..self.lines.snap_up(end.max(start))
    }

    fn collect<'a>(&mut self, node: &'a AstNode<'a>, out: &mut Vec<Unit>) {
        for child in node.children() {
            let ast = child.data.borrow();
            let bytes = self.bytes(ast.sourcepos);
            let src = self.lines.text.get(bytes.clone()).unwrap_or("");
            if !plausible(&ast.value, src) {
                self.mapped = false;
            }
            match &ast.value {
                NodeValue::Text(literal) => self.text_units(child, literal, bytes, out),
                NodeValue::Code(code) => {
                    match code_span_units(code, bytes.start, self.lines.text) {
                        Some(units) => out.extend(units),
                        None => {
                            self.mapped = false;
                            out.extend(code.literal.chars().map(|c| Unit::new(c, 0, 0)));
                        }
                    }
                }
                NodeValue::ShortCode(short) => out.extend(
                    short
                        .emoji
                        .chars()
                        .map(|c| Unit::atomic(c, bytes.start, bytes.end)),
                ),
                NodeValue::SoftBreak | NodeValue::LineBreak => {
                    out.push(Unit::new(' ', bytes.start, bytes.end))
                }
                // The page shows these as one object: a formula, a picture, a footnote mark
                NodeValue::Math(_) | NodeValue::Image(_) | NodeValue::FootnoteReference(_) => {
                    out.push(Unit::atomic('\u{fffc}', bytes.start, bytes.end))
                }
                // The browser shows a raw <br> as a break and a raw <img> as a picture
                NodeValue::HtmlInline(tag) => match raw_tag_name(tag).as_deref() {
                    Some("br") => out.push(Unit::atomic(' ', bytes.start, bytes.end)),
                    Some("img") => out.push(Unit::atomic('\u{fffc}', bytes.start, bytes.end)),
                    _ => {}
                },
                NodeValue::Link(_)
                | NodeValue::Emph
                | NodeValue::Strong
                | NodeValue::Strikethrough => {
                    let link = matches!(ast.value, NodeValue::Link(_));
                    self.wraps.push(Wrap {
                        link,
                        bytes,
                        text: None,
                    });
                    self.collect(child, out);
                }
                _ => self.collect(child, out),
            }
        }
    }

    fn text_units<'a>(
        &mut self,
        node: &'a AstNode<'a>,
        literal: &str,
        bytes: Range<usize>,
        out: &mut Vec<Unit>,
    ) {
        let src = self.lines.text;
        // comrak starts a text that opens with an escape after its backslash
        let escaped = bytes.start.checked_sub(1).filter(|&back| {
            src.as_bytes()[back] == b'\\'
                && literal.starts_with(|c: char| c.is_ascii_punctuation())
                && node
                    .previous_sibling()
                    .is_none_or(|prev| self.bytes(prev.data.borrow().sourcepos).end <= back)
        });
        let bytes = match escaped {
            Some(back) => back..bytes.end,
            None => bytes,
        };
        let strict = src
            .get(bytes.clone())
            .and_then(|slice| align_text(literal, slice, bytes.start, true));
        if let Some(units) = strict {
            out.extend(units);
            return;
        }
        // comrak can end a text's sourcepos early
        let limit = match node.next_sibling() {
            Some(next) => self.bytes(next.data.borrow().sourcepos).start,
            None => node.parent().map_or(bytes.end, |p| {
                let parent = p.data.borrow();
                match parent.value {
                    NodeValue::TableCell => self.lines.bytes(parent.sourcepos).end,
                    _ => self.bytes(parent.sourcepos).end,
                }
            }),
        };
        let loose = src
            .get(bytes.start..limit.max(bytes.end))
            .and_then(|slice| align_text(literal, slice, bytes.start, false));
        match loose {
            Some(units) => out.extend(units),
            None => {
                self.mapped = false;
                out.extend(literal.chars().map(|c| Unit::new(c, 0, 0)));
            }
        }
    }

    /// Source bytes of units `a..b`, grown to whole tokens and balanced markup.
    fn span(&self, a: usize, b: usize) -> (Range<usize>, bool) {
        let picked = &self.units[a..b];
        let mut start = picked.iter().map(|u| u.s).min().unwrap_or(0);
        let mut end = picked.iter().map(|u| u.e).max().unwrap_or(start);
        let mut widened = picked.iter().any(|u| u.atomic);
        // Containers nest, so each pass can only reach one level further out
        loop {
            let mut grew = false;
            for wrap in &self.wraps {
                let Some((first, last)) = wrap.text else {
                    continue;
                };
                if start >= last || end <= first {
                    continue;
                }
                // A link moves as a whole; emphasis only when a range reaches its edge
                if wrap.link || start <= first || end >= last {
                    widened = true;
                    if start > wrap.bytes.start || end < wrap.bytes.end {
                        start = start.min(wrap.bytes.start);
                        end = end.max(wrap.bytes.end);
                        grew = true;
                    }
                }
            }
            if !grew {
                return (start..end, widened);
            }
        }
    }

    /// Bytes from the first to the last unit inside `bytes`; units are in source order.
    fn text_span(&self, bytes: &Range<usize>) -> Option<(usize, usize)> {
        let from = self.units.partition_point(|u| u.s < bytes.start);
        let to = self.units.partition_point(|u| u.s < bytes.end);
        let inside = self.units.get(from..to)?;
        let mut real = inside.iter().filter(|u| u.s < u.e && u.e <= bytes.end);
        let first = real.next()?;
        let last = real.next_back().unwrap_or(first);
        Some((first.s, last.e))
    }

    fn cand<'a>(
        &self,
        node: &'a AstNode<'a>,
        kind: OwnerKind,
        sp: &str,
        markdown: &str,
        a: usize,
        len: usize,
    ) -> Cand {
        let (bytes, _) = self.span(a, a + len);
        let preview = if kind == OwnerKind::Heading {
            let heading = markdown
                .get(self.lines.bytes(node.data.borrow().sourcepos))
                .unwrap_or("");
            format!("→ {}", heading.lines().next().unwrap_or("").trim())
        } else {
            format!("→ \"{}\"", self.excerpt(a, a + len))
        };
        Cand {
            sp: Some(sp.to_string()),
            off: Some(a),
            source: markdown.get(bytes.clone()).unwrap_or("").to_string(),
            bytes,
            preview,
        }
    }

    /// Units `a..b` with a little text either side, cut with `…`.
    fn excerpt(&self, a: usize, b: usize) -> String {
        const SIDE: usize = 16;
        let chars = self.chars();
        let (from, to) = (a.saturating_sub(SIDE), (b + SIDE).min(chars.len()));
        let mut out = String::new();
        if from > 0 {
            out.push('…');
        }
        out.extend(&chars[from..to]);
        if to < chars.len() {
            out.push('…');
        }
        out
    }
}

/// Every start of `q` in `t`, overlapping ones included, in linear time (Knuth–Morris–Pratt).
fn occurrences(t: &[char], q: &[char]) -> Vec<usize> {
    if q.is_empty() || q.len() > t.len() {
        return Vec::new();
    }
    // border[i]: length of the longest proper prefix of q[..=i] that is also its suffix
    let mut border = vec![0; q.len()];
    let mut k = 0;
    for i in 1..q.len() {
        while k > 0 && q[i] != q[k] {
            k = border[k - 1];
        }
        if q[i] == q[k] {
            k += 1;
        }
        border[i] = k;
    }
    let mut hits = Vec::new();
    k = 0;
    for (i, c) in t.iter().enumerate() {
        while k > 0 && *c != q[k] {
            k = border[k - 1];
        }
        if *c == q[k] {
            k += 1;
        }
        if k == q.len() {
            hits.push(i + 1 - k);
            k = border[k - 1];
        }
    }
    hits
}

/// Whitespace as JavaScript's `\s` sees it, which the page uses: no U+0085, plus U+FEFF.
pub(crate) fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// Runs of whitespace become one space, which keeps the first one's bytes; the ends are trimmed.
fn collapse(units: Vec<Unit>) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::with_capacity(units.len());
    for unit in units {
        if !is_js_space(unit.ch) {
            out.push(unit);
        } else if out.last().is_some_and(|last| last.ch != ' ') {
            out.push(Unit { ch: ' ', ..unit });
        }
    }
    if out.last().is_some_and(|last| last.ch == ' ') {
        out.pop();
    }
    out
}

/// Typed or quoted text with whitespace runs as single spaces, ends kept.
pub(crate) fn collapse_str(s: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        if !is_js_space(c) {
            out.push(c);
        } else if out.last() != Some(&' ') {
            out.push(' ');
        }
    }
    out
}

fn trim_spaces(chars: &[char]) -> &[char] {
    let start = chars.iter().position(|&c| c != ' ').unwrap_or(chars.len());
    let end = chars
        .iter()
        .rposition(|&c| c != ' ')
        .map_or(start, |i| i + 1);
    &chars[start..end]
}

/// Maps each char of a text node's `literal` to the bytes of `src` (at `base`) it came from.
/// Escapes, entities, smart punctuation and skipped whitespace are the only differences.
/// `strict` also requires nothing but whitespace to be left over in `src`.
fn align_text(literal: &str, src: &str, base: usize, strict: bool) -> Option<Vec<Unit>> {
    let sc: Vec<(usize, char)> = src.char_indices().collect();
    let lc: Vec<char> = literal.chars().collect();
    let at = |k: usize| base + sc.get(k).map_or(src.len(), |&(b, _)| b);
    let mut out = Vec::with_capacity(lc.len());
    let (mut i, mut j) = (0, 0);
    while j < lc.len() {
        let &(b, c) = sc.get(i)?;
        let l = lc[j];
        let escaped = sc.get(i + 1).map(|&(_, n)| n);
        if c == '\\' && escaped.is_some_and(|n| n.is_ascii_punctuation() && n == l) {
            out.push(Unit::new(l, at(i), at(i + 2)));
            i += 2;
            j += 1;
        } else if let Some((len, decoded)) = (c == '&').then(|| entity_at(&src[b..])).flatten()
            && lc[j..].starts_with(&decoded.chars().collect::<Vec<_>>())
        {
            let end = base + b + len;
            out.extend(decoded.chars().map(|d| Unit::atomic(d, base + b, end)));
            j += decoded.chars().count();
            while sc.get(i).is_some_and(|&(k, _)| k < b + len) {
                i += 1;
            }
        } else if matches!(c, '-' | '.') && l != c && matches!(l, '–' | '—' | '…') {
            // Maximal runs on both sides, so cmark's run-splitting rules needn't be redone
            let mut ie = i;
            while sc.get(ie).is_some_and(|&(_, x)| x == c) {
                ie += 1;
            }
            let mut je = j;
            while lc
                .get(je)
                .is_some_and(|&x| x == c || matches!(x, '–' | '—' | '…'))
            {
                je += 1;
            }
            let (s, e) = (at(i), at(ie));
            out.extend(lc[j..je].iter().map(|&x| Unit::atomic(x, s, e)));
            i = ie;
            j = je;
        } else if (matches!(c, '"' | '\'') && matches!(l, '“' | '”' | '‘' | '’')) || c == l
        {
            out.push(Unit::new(l, at(i), at(i + 1)));
            i += 1;
            j += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else {
            return None;
        }
    }
    let rest_blank = sc[i.min(sc.len())..]
        .iter()
        .all(|&(_, c)| c.is_whitespace());
    (!strict || rest_blank).then_some(out)
}

/// The entity at the start of `src` (`&amp;`, `&#35;`, `&#x1F389;`): its byte length and text.
fn entity_at(src: &str) -> Option<(usize, String)> {
    let end = src.find(';').filter(|&e| e < 40)?;
    let token = &src[..=end];
    let body = &token[1..end];
    if let Some(num) = body.strip_prefix('#') {
        let (digits, radix, max) = match num.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16, 6),
            None => (num, 10, 7),
        };
        if digits.is_empty() || digits.len() > max {
            return None;
        }
        let code = u32::from_str_radix(digits, radix).ok()?;
        let ch = char::from_u32(code)
            .filter(|&c| c != '\0')
            .unwrap_or('\u{fffd}');
        return Some((token.len(), ch.to_string()));
    }
    static NAMED: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let named = NAMED.get_or_init(|| {
        entities::ENTITIES
            .iter()
            .filter(|e| e.entity.ends_with(';'))
            .map(|e| (e.entity, e.characters))
            .collect()
    });
    named.get(token).map(|text| (token.len(), text.to_string()))
}

/// The lowercased name of an opening tag like `<BR/>` or `<img src=x>`.
fn raw_tag_name(tag: &str) -> Option<String> {
    let rest = tag.strip_prefix('<')?;
    let end = rest
        .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        .unwrap_or(rest.len());
    let name = &rest[..end];
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| name.to_ascii_lowercase())
}

/// Units of a code span that starts at `start`. comrak can misplace a multi-line span's end,
/// so the closing run of backticks is found in the source instead.
fn code_span_units(code: &NodeCode, start: usize, src: &str) -> Option<Vec<Unit>> {
    let ticks = code.num_backticks;
    let rest = src.get(start..)?;
    let body = rest.get(ticks..)?;
    if !rest[..ticks].bytes().all(|b| b == b'`') || body.starts_with('`') {
        return None;
    }
    let mut at = 0;
    let close = loop {
        let i = at + body[at..].find('`')?;
        let run = body[i..].len() - body[i..].trim_start_matches('`').len();
        if run == ticks {
            break i;
        }
        at = i + run;
    };
    let base = start + ticks;
    let sc: Vec<(usize, char)> = body[..close].char_indices().collect();
    let lc: Vec<char> = code.literal.chars().collect();
    let at = |k: usize| base + sc.get(k).map_or(close, |&(b, _)| b);
    let mut out = Vec::with_capacity(lc.len());
    let (mut i, mut j) = (0, 0);
    while j < lc.len() {
        let (c, l) = (sc.get(i)?.1, lc[j]);
        let next = sc.get(i + 1).map(|&(_, n)| n);
        if c == l {
            out.push(Unit::new(l, at(i), at(i + 1)));
            i += 1;
            j += 1;
        } else if c == '\\' && next == Some('|') && l == '|' {
            // A table cell unescapes pipes even inside code
            out.push(Unit::new(l, at(i), at(i + 2)));
            i += 2;
            j += 1;
        } else if matches!(c, '\n' | '\r') && l == ' ' {
            // A line ending shows as a space, and the next line's indent is not in the literal
            let width = if c == '\r' && next == Some('\n') {
                2
            } else {
                1
            };
            out.push(Unit::new(l, at(i), at(i + width)));
            i += width;
            j += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else {
            return None;
        }
    }
    sc[i..]
        .iter()
        .all(|&(_, c)| c.is_whitespace())
        .then_some(out)
}

/// Whether `src` could be the source of an inline `value`. comrak misplaces some inline
/// sourcepos, and a range built on a wrong one would quote the wrong text.
fn plausible(value: &NodeValue, src: &str) -> bool {
    let wrapped = |open: &[char], close: &[char]| src.starts_with(open) && src.ends_with(close);
    match value {
        NodeValue::Emph | NodeValue::Strong => wrapped(&['*', '_'], &['*', '_']),
        NodeValue::Strikethrough => wrapped(&['~'], &['~']),
        // Bare autolinks (`www.x.org`, `a@b.c`) are their own text
        NodeValue::Link(_) => {
            wrapped(&['['], &[')', ']'])
                || wrapped(&['<'], &['>'])
                || src.starts_with(char::is_alphanumeric)
        }
        NodeValue::Image(_) => src.starts_with("![") && src.ends_with([')', ']']),
        NodeValue::FootnoteReference(_) => src.starts_with("[^") && src.ends_with(']'),
        NodeValue::Math(_) => wrapped(&['$'], &['$']),
        NodeValue::ShortCode(_) => wrapped(&[':'], &[':']),
        NodeValue::HtmlInline(_) => wrapped(&['<'], &['>']),
        NodeValue::SoftBreak | NodeValue::LineBreak => src
            .trim_matches(char::is_whitespace)
            .trim_start_matches('\\')
            .is_empty(),
        _ => true,
    }
}

/// Maps a code block's literal line by line onto its source lines, `None` if one doesn't match.
fn code_units(block: &NodeCodeBlock, start: usize, lines: &LineMap) -> Option<Vec<Unit>> {
    let first = if block.fenced { start + 1 } else { start };
    let src = lines.text;
    let mut out = Vec::with_capacity(block.literal.len());
    let mut pieces = block.literal.split('\n').peekable();
    let mut n = first;
    while let Some(piece) = pieces.next() {
        let terminated = pieces.peek().is_some();
        if !terminated && piece.is_empty() {
            break;
        }
        let line = piece.strip_suffix('\r').unwrap_or(piece);
        let (line_start, line_end, next) = lines.line(n)?;
        let source = &src[line_start..line_end];
        // The literal is the line less its container prefix and indent; an indent that
        // ends inside a tab leaves that tab's remaining columns as spaces
        let spaces = line.len() - line.trim_start_matches(' ').len();
        let (split, rest) = (0..=spaces).find_map(|k| {
            let rest = &line[k..];
            let at = source.len().checked_sub(rest.len())?;
            let fits = source.ends_with(rest) && (k == 0 || source[..at].ends_with('\t'));
            fits.then_some((k, rest))
        })?;
        let at = line_end - rest.len();
        out.extend((0..split).map(|_| Unit::atomic(' ', at - 1, at)));
        out.extend(
            rest.char_indices()
                .map(|(k, c)| Unit::new(c, at + k, at + k + c.len_utf8())),
        );
        if terminated {
            out.push(Unit::new('\n', line_end, next));
        }
        n += 1;
    }
    Some(out)
}
