//! Obsidian syntax that comrak leaves as plain text: `![[embeds]]` and `#tags`.
//!
//! Runs on the parsed tree, after URL resolution, by splitting `Text` nodes.
//! Code spans and fenced code are separate nodes, so they are never touched.

use comrak::nodes::{Ast, AstNode, NodeHtmlBlock, NodeValue, Sourcepos};
use comrak::{Arena, html::escape_href};

use crate::render::{Resolved, Resolver, html_escape};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif"];

pub(crate) struct Syntax<'r> {
    pub embeds: bool,
    pub tags: bool,
    pub resolver: Option<&'r dyn Resolver>,
    /// Renders a note embed target to HTML, or `None` to leave it as text.
    pub note: &'r dyn Fn(&str) -> Option<String>,
}

/// Targets found while rewriting, in document order without duplicates.
#[derive(Default)]
pub(crate) struct Found {
    pub embeds: Vec<String>,
    pub tags: Vec<String>,
}

enum Piece {
    Text(String),
    Html(String),
}

pub(crate) fn rewrite<'a>(
    arena: &'a Arena<'a>,
    root: &'a AstNode<'a>,
    source: &str,
    syntax: &Syntax<'_>,
) -> Found {
    let mut found = Found::default();
    if !syntax.embeds && !syntax.tags {
        return found;
    }
    let lines: Vec<&str> = source.split('\n').collect();

    if syntax.embeds {
        embed_notes(root, &lines, syntax, &mut found);
    }

    let candidates: Vec<_> = root
        .descendants()
        .filter(|node| matches!(node.data.borrow().value, NodeValue::Text(_)))
        .filter(|node| !inside_link(node))
        .collect();

    for node in candidates {
        let (literal, sourcepos) = {
            let data = node.data.borrow();
            let NodeValue::Text(text) = &data.value else {
                continue;
            };
            (text.to_string(), data.sourcepos)
        };
        let sourcepos = widen_in_table_cell(node, sourcepos);

        // Escapes and entities are already decoded in `literal`, so every
        // match is checked against the source text, and without it nothing changes
        let Some(raw) = raw_text(&lines, sourcepos) else {
            continue;
        };
        let mut pieces = vec![Piece::Text(literal.clone())];
        if syntax.embeds && literal.contains("![[") {
            pieces = split_embeds(&literal, raw, syntax, &mut found);
        }
        if syntax.tags {
            let starts_clean = starts_at_boundary(node);
            let mut cursor = 0;
            pieces = pieces
                .into_iter()
                .enumerate()
                .flat_map(|(i, piece)| match piece {
                    Piece::Text(text) => split_tags(
                        &text,
                        i == 0 && starts_clean,
                        (raw, &mut cursor),
                        syntax,
                        &mut found,
                    ),
                    html => vec![html],
                })
                .collect();
        }

        if matches!(pieces.as_slice(), [Piece::Text(t)] if *t == literal) {
            continue;
        }
        for piece in pieces {
            let value = match piece {
                Piece::Text(t) if t.is_empty() => continue,
                Piece::Text(t) => NodeValue::Text(t.into()),
                Piece::Html(h) => NodeValue::HtmlInline(h),
            };
            let new = arena.alloc(AstNode::from(Ast::new_with_sourcepos(value, sourcepos)));
            node.insert_before(new);
        }
        node.detach();
    }
    found
}

/// The source text behind a single-line node, and where it sits in its line.
#[derive(Clone, Copy)]
struct Raw<'s> {
    line: &'s str,
    start: usize,
    text: &'s str,
}

impl Raw<'_> {
    /// Whether `pos` in `text` follows an odd run of backslashes. Counts over
    /// the whole line: a node can start just after the backslash escaping it.
    fn escaped(&self, pos: usize) -> bool {
        let abs = self.start + pos;
        self.line[..abs]
            .bytes()
            .rev()
            .take_while(|b| *b == b'\\')
            .count()
            % 2
            == 1
    }
}

/// Columns in sourcepos are 1-based byte offsets.
fn raw_text<'s>(lines: &[&'s str], sourcepos: Sourcepos) -> Option<Raw<'s>> {
    if sourcepos.start.line != sourcepos.end.line {
        return None;
    }
    let line = lines.get(sourcepos.start.line.wrapping_sub(1))?;
    let start = sourcepos.start.column.saturating_sub(1);
    let text = line.get(start..sourcepos.end.column)?;
    Some(Raw { line, start, text })
}

/// comrak ends a table cell's text at its unescaped length, so `\|` cuts the
/// span short. The cell's own end still covers it, plus trailing spaces.
fn widen_in_table_cell<'a>(node: &'a AstNode<'a>, mut sourcepos: Sourcepos) -> Sourcepos {
    if let Some(parent) = node.parent() {
        let data = parent.data.borrow();
        if matches!(data.value, NodeValue::TableCell)
            && data.sourcepos.end.line == sourcepos.end.line
        {
            sourcepos.end.column = data.sourcepos.end.column;
        }
    }
    sourcepos
}

/// Replace each paragraph that is nothing but a note embed with the rendered
/// note. Inline note embeds stay text: a `<div>` cannot sit inside a `<p>`.
fn embed_notes<'a>(root: &'a AstNode<'a>, lines: &[&str], syntax: &Syntax<'_>, found: &mut Found) {
    let paragraphs: Vec<_> = root
        .descendants()
        .filter(|n| matches!(n.data.borrow().value, NodeValue::Paragraph))
        .collect();

    for paragraph in paragraphs {
        let Some(child) = paragraph.first_child() else {
            continue;
        };
        if child.next_sibling().is_some() {
            continue;
        }
        let (literal, sourcepos) = match &child.data.borrow().value {
            NodeValue::Text(t) => (t.to_string(), child.data.borrow().sourcepos),
            _ => continue,
        };
        if !matches!(find_embeds(&literal).as_slice(), [(0, end, _)] if *end == literal.len()) {
            continue;
        }
        let Some(raw) = raw_text(lines, sourcepos) else {
            continue;
        };
        let inner = match find_embeds(raw.text).as_slice() {
            [(start, _, inner)] if !raw.escaped(*start) => inner.to_string(),
            _ => continue,
        };
        let target = inner.split('|').next().unwrap_or_default().trim();
        if target.is_empty() || is_image(target) {
            continue;
        }
        push_unique(&mut found.embeds, target);
        if let Some(html) = (syntax.note)(target) {
            paragraph.data.borrow_mut().value = NodeValue::HtmlBlock(NodeHtmlBlock {
                block_type: 6,
                literal: html,
            });
            child.detach();
        }
    }
}

fn inside_link<'a>(node: &'a AstNode<'a>) -> bool {
    node.ancestors().any(|a| {
        matches!(
            a.data.borrow().value,
            NodeValue::Link(_) | NodeValue::WikiLink(_) | NodeValue::Image(_)
        )
    })
}

/// A tag may open a text node only when nothing inline sits right before it.
fn starts_at_boundary<'a>(node: &'a AstNode<'a>) -> bool {
    match node.previous_sibling() {
        None => true,
        Some(prev) => matches!(
            prev.data.borrow().value,
            NodeValue::SoftBreak | NodeValue::LineBreak
        ),
    }
}

/// `![[inner]]` spans in `text` as (start, end, inner) byte ranges.
fn find_embeds(text: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = text[from..].find("![[").map(|i| i + from) {
        let body = open + 3;
        let Some(close) = text[body..].find("]]").map(|i| i + body) else {
            break;
        };
        let inner = &text[body..close];
        if inner.is_empty() || inner.contains(['[', ']']) {
            from = body;
            continue;
        }
        out.push((open, close + 2, inner));
        from = close + 2;
    }
    out
}

/// Split embeds out of a text node. Matches and targets come from `raw`, the
/// node's source text: smart punctuation has rewritten `literal` (`it's` to
/// `it’s`) and escapes are gone from it (`\![[x]]` reads `![[x]]`).
fn split_embeds(literal: &str, raw: Raw<'_>, syntax: &Syntax<'_>, found: &mut Found) -> Vec<Piece> {
    let matches = find_embeds(literal);
    let raw_matches = find_embeds(raw.text);
    // An escape or entity created or hid a match (`!\[[x]]`, `&#33;[[x]]`)
    if matches.len() != raw_matches.len() {
        return vec![Piece::Text(literal.to_string())];
    }

    let mut pieces = Vec::new();
    let mut last = 0;
    for ((start, end, _), (raw_start, _, inner)) in matches.into_iter().zip(raw_matches) {
        if raw.escaped(raw_start) {
            continue;
        }
        // Obsidian escapes the pipe as `\|` inside tables
        let inner = inner.replace("\\|", "|");
        let (target, option) = match inner.split_once('|') {
            Some((t, o)) => (t.trim(), Some(o.trim())),
            None => (inner.trim(), None),
        };
        if target.is_empty() {
            continue;
        }
        push_unique(&mut found.embeds, target);
        if !is_image(target) {
            continue;
        }
        pieces.push(Piece::Text(literal[last..start].to_string()));
        pieces.push(Piece::Html(image_html(target, option, syntax.resolver)));
        last = end;
    }
    pieces.push(Piece::Text(literal[last..].to_string()));
    pieces
}

fn is_image(target: &str) -> bool {
    target
        .rsplit_once('.')
        .is_some_and(|(_, ext)| IMAGE_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// `|300` sets the width, `|300x200` both sides, anything else is alt text.
fn image_html(target: &str, option: Option<&str>, resolver: Option<&dyn Resolver>) -> String {
    let src = match resolver.map(|r| r.embed(target)) {
        Some(Resolved::Found(url)) => url,
        Some(Resolved::Missing(_)) => {
            return format!(
                "<span data-embed=\"image\" data-unresolved=\"true\">{}</span>",
                html_escape(target)
            );
        }
        None => target.to_string(),
    };
    let mut href = String::new();
    escape_href(&mut href, &src, false).expect("writing to a String cannot fail");

    let (alt, size) = match option {
        Some(o) if parse_size(o).is_some() => (target, parse_size(o)),
        Some(o) if !o.is_empty() => (o, None),
        _ => (target, None),
    };
    let size = match size {
        Some((w, Some(h))) => format!(" width=\"{w}\" height=\"{h}\""),
        Some((w, None)) => format!(" width=\"{w}\""),
        None => String::new(),
    };
    format!(
        "<img src=\"{href}\" alt=\"{}\"{size} data-embed=\"image\" />",
        html_escape(alt)
    )
}

fn parse_size(option: &str) -> Option<(u32, Option<u32>)> {
    match option.split_once('x') {
        Some((w, h)) => Some((w.parse().ok()?, Some(h.parse().ok()?))),
        None => Some((option.parse().ok()?, None)),
    }
}

/// Split `#tags` out of a text piece. `starts_clean` says whether a tag may
/// sit at offset 0; elsewhere it must follow whitespace. Each tag must also
/// appear unescaped in `raw`, searched forward from `cursor`.
fn split_tags(
    text: &str,
    starts_clean: bool,
    (raw, cursor): (Raw<'_>, &mut usize),
    syntax: &Syntax<'_>,
    found: &mut Found,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut last = 0;
    let mut prev: Option<char> = None;
    let mut iter = text.char_indices().peekable();

    while let Some((i, c)) = iter.next() {
        let at_boundary = match prev {
            None => starts_clean,
            Some(p) => opens_tag(p),
        };
        prev = Some(c);
        if c != '#' || !at_boundary {
            continue;
        }
        let name_start = i + 1;
        let name_end = text[name_start..]
            .find(|ch: char| !is_tag_char(ch))
            .map_or(text.len(), |n| n + name_start);
        let name = &text[name_start..name_end];
        if name.is_empty()
            || name
                .chars()
                .all(|ch| ch.is_ascii_digit() || ch == '/' || ch == '-' || ch == '_')
            || !in_source(raw, cursor, name)
        {
            continue;
        }

        pieces.push(Piece::Text(text[last..i].to_string()));
        pieces.extend(tag_nodes(name, syntax.resolver));
        push_unique(&mut found.tags, name);
        last = name_end;
        while iter.peek().is_some_and(|(j, _)| *j < name_end) {
            prev = iter.next().map(|(_, ch)| ch);
        }
    }
    pieces.push(Piece::Text(text[last..].to_string()));
    pieces
}

/// Find `#name` in `raw` from `cursor` on, moving the cursor past it. False
/// when it is escaped (`\#name`) or absent, as for an entity (`&#35;name`).
fn in_source(raw: Raw<'_>, cursor: &mut usize, name: &str) -> bool {
    let needle = format!("#{name}");
    let mut from = *cursor;
    while let Some(at) = raw
        .text
        .get(from..)
        .and_then(|r| r.find(&needle))
        .map(|i| i + from)
    {
        let after = at + needle.len();
        let before = raw.line[..raw.start + at].chars().next_back();
        // `\` stays a candidate here so that the escape check below rejects it
        let at_boundary = at == 0 || before.is_some_and(|c| c == '\\' || opens_tag(c));
        if !at_boundary || raw.text[after..].chars().next().is_some_and(is_tag_char) {
            from = at + 1;
            continue;
        }
        *cursor = after;
        return !raw.escaped(at);
    }
    false
}

/// Whether a tag may start right after `c`.
fn opens_tag(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | '[' | '"' | '\'' | '“' | '‘')
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

/// The tag text stays a `Text` node so heading ids are slugged as before.
fn tag_nodes(name: &str, resolver: Option<&dyn Resolver>) -> [Piece; 3] {
    let data = html_escape(name);
    let (open, close) = match resolver.and_then(|r| r.tag(name)) {
        Some(url) => {
            let mut href = String::new();
            escape_href(&mut href, &url, false).expect("writing to a String cannot fail");
            (
                format!("<a class=\"tag\" href=\"{href}\" data-tag=\"{data}\">"),
                "</a>",
            )
        }
        None => (
            format!("<span class=\"tag\" data-tag=\"{data}\">"),
            "</span>",
        ),
    };
    [
        Piece::Html(open),
        Piece::Text(format!("#{name}")),
        Piece::Html(close.to_string()),
    ]
}

fn push_unique(list: &mut Vec<String>, item: &str) {
    if !list.iter().any(|x| x == item) {
        list.push(item.to_string());
    }
}
