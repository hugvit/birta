//! Where rendered blocks come from in the source: focus stops and text owners.

use std::ops::Range;

use comrak::Options;
use comrak::arena_tree::NodeEdge;
use comrak::nodes::{AstNode, LineColumn, NodeValue, Sourcepos};
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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
        let ast = node.data.borrow();
        let parent_ast = node.parent().map(|p| p.data.borrow());
        let parent = parent_ast.as_deref().map(|p| &p.value);
        let (stop, owner) = match &ast.value {
            NodeValue::FootnoteDefinition(_) => {
                footnote_depth += 1;
                continue;
            }
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
            _ => continue,
        };

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

    /// Lines as `str::lines` counts them: a final terminator starts no new line.
    fn count(&self) -> u32 {
        let ends_open = self.starts.last() == Some(&self.len) && self.len > 0;
        (self.starts.len() - usize::from(ends_open)) as u32
    }

    fn bytes(&self, sp: Sourcepos) -> Range<usize> {
        let start = self.offset(sp.start);
        let end = match sp.end.column {
            0 => self.offset(sp.end),
            // The end column is the node's last byte, so the range ends one past it
            _ => (self.offset(sp.end) + 1).min(self.len),
        };
        // comrak's sp can end mid-character (a `dt` is always `L:1-L:1`)
        let mut end = end.max(start);
        while !self.text.is_char_boundary(end) {
            end += 1;
        }
        start..end
    }
}
