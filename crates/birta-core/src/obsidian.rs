//! Obsidian syntax that comrak leaves as plain text: `![[embeds]]` and `#tags`.
//!
//! Runs on the parsed tree, after URL resolution, by splitting `Text` nodes.
//! Code spans and fenced code are separate nodes, so they are never touched.

use comrak::nodes::{Ast, AstNode, NodeHtmlBlock, NodeValue, Sourcepos};
use comrak::{Arena, html::escape_href};

use crate::render::{Resolver, html_escape};

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

        let mut pieces = vec![Piece::Text(literal.clone())];
        if syntax.embeds && literal.contains("![[") {
            let raw = raw_text(&lines, sourcepos);
            pieces = split_embeds(&literal, raw, syntax, &mut found);
        }
        if syntax.tags {
            let starts_clean = starts_at_boundary(node);
            pieces = pieces
                .into_iter()
                .enumerate()
                .flat_map(|(i, piece)| match piece {
                    Piece::Text(text) => {
                        split_tags(&text, i == 0 && starts_clean, syntax, &mut found)
                    }
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

/// The source text behind a single-line node, columns being byte offsets.
fn raw_text<'s>(lines: &[&'s str], sourcepos: Sourcepos) -> Option<&'s str> {
    if sourcepos.start.line != sourcepos.end.line {
        return None;
    }
    let line = lines.get(sourcepos.start.line.wrapping_sub(1))?;
    line.get(sourcepos.start.column.saturating_sub(1)..sourcepos.end.column)
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
        let inner = match find_embeds(&literal).as_slice() {
            [(0, end, inner)] if *end == literal.len() => inner.to_string(),
            _ => continue,
        };
        let inner = raw_text(lines, sourcepos)
            .and_then(|raw| match find_embeds(raw).as_slice() {
                [(_, _, raw_inner)] => Some(raw_inner.to_string()),
                _ => None,
            })
            .unwrap_or(inner);
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

/// Split embeds out of a text node. Targets come from `raw`, the node's
/// source text, because smart punctuation has already rewritten `literal`
/// (`it's` to `it’s`, `--` to `–`) and file names must match on disk.
fn split_embeds(
    literal: &str,
    raw: Option<&str>,
    syntax: &Syntax<'_>,
    found: &mut Found,
) -> Vec<Piece> {
    let matches = find_embeds(literal);
    let raw_inners: Vec<&str> = raw
        .map(|r| find_embeds(r).into_iter().map(|m| m.2).collect())
        .unwrap_or_default();
    let use_raw = raw_inners.len() == matches.len();

    let mut pieces = Vec::new();
    let mut last = 0;
    for (n, (start, end, inner)) in matches.into_iter().enumerate() {
        let inner = if use_raw { raw_inners[n] } else { inner };
        let (target, option) = match inner.split_once('|') {
            Some((t, o)) => (t.trim(), Some(o.trim())),
            None => (inner.trim(), None),
        };
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
    let src = match resolver {
        Some(r) => r.embed(target),
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
/// sit at offset 0; elsewhere it must follow whitespace.
fn split_tags(
    text: &str,
    starts_clean: bool,
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
            Some(p) => p.is_whitespace() || matches!(p, '(' | '[' | '"' | '\'' | '“' | '‘'),
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
