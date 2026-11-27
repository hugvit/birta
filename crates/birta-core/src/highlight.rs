use comrak::plugins::syntect::{SyntectAdapter, SyntectAdapterBuilder};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style, ThemeSet};
use syntect::html::{ClassStyle, IncludeBackground, append_highlighted_html_for_styled_line};
use syntect::parsing::{BasicScopeStackOp, ParseState, Scope, ScopeStack, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::theme::SyntaxTheme;

/// Create a syntax highlighter adapter using CSS classes (theme-agnostic).
///
/// Light/dark theming is handled by `syntax.css` in the browser.
pub fn adapter() -> SyntectAdapter {
    SyntectAdapter::new(None)
}

/// Create a syntax highlighter adapter using inline styles from a loaded theme.
pub fn adapter_with_theme(syntax_theme: &SyntaxTheme) -> SyntectAdapter {
    let mut theme_set = ThemeSet::new();
    theme_set
        .themes
        .insert(syntax_theme.theme_name.clone(), syntax_theme.theme.clone());
    SyntectAdapterBuilder::new()
        .theme(&syntax_theme.theme_name)
        .theme_set(theme_set)
        .build()
}

/// Highlight a full markdown file as source code.
///
/// Returns the inner HTML content (spans + text, no wrapping `<pre>` tag).
/// In CSS-class mode (no theme), produces `<span class="...">` matching `syntax.css`.
/// In inline-style mode (with theme), produces `<span style="color:...">` spans.
/// Lines over 8 KiB (e.g. a data URI) are left unhighlighted; the rest of the
/// file is highlighted as usual.
pub fn highlight_source(source: &str, syntax_theme: Option<&SyntaxTheme>) -> String {
    let ss = syntax_set();
    let syntax = ss
        .find_syntax_by_extension("md")
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let html = match syntax_theme {
        None => highlight_classed(source, syntax, ss),
        Some(st) => highlight_styled(source, syntax, ss, st),
    };
    // The HTML parser normalizes raw CRs to LF, which breaks CRLF line alignment and copying
    html.replace('\r', "&#13;")
}

/// Lines longer than this many bytes are left unhighlighted; syntect's cost
/// on one long line grows much faster than linearly.
pub(crate) const HIGHLIGHT_MAX_LINE: usize = 8 * 1024;

// Loading the default syntaxes takes milliseconds; a diff highlights twice per call
fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// CSS-class mode: produces `<span class="keyword">` etc. matching `syntax.css`.
///
/// Same output as syntect's `ClassedHTMLGenerator`, except that lines over
/// [`HIGHLIGHT_MAX_LINE`] are escaped inside the spans already open, unparsed.
fn highlight_classed(
    source: &str,
    syntax: &syntect::parsing::SyntaxReference,
    ss: &SyntaxSet,
) -> String {
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut open_spans: isize = 0;
    let mut html = String::with_capacity(source.len() * 2);
    for line in LinesWithEndings::from(source) {
        if line.len() > HIGHLIGHT_MAX_LINE {
            html.push_str(&crate::render::html_escape(line));
            continue;
        }
        let spans = state.parse_line(line, ss).ok().and_then(|ops| {
            syntect::html::line_tokens_to_classed_spans(line, &ops, ClassStyle::Spaced, &mut stack)
                .ok()
        });
        match spans {
            Some((formatted, delta)) => {
                open_spans += delta;
                html.push_str(&formatted);
            }
            None => html.push_str(&crate::render::html_escape(line)),
        }
    }
    for _ in 0..open_spans {
        html.push_str("</span>");
    }
    html
}

/// Inline-style mode: produces `<span style="color:#cf222e">` etc. from a tmTheme.
/// Lines over [`HIGHLIGHT_MAX_LINE`] are escaped, unparsed.
fn highlight_styled(
    source: &str,
    syntax: &syntect::parsing::SyntaxReference,
    ss: &SyntaxSet,
    syntax_theme: &SyntaxTheme,
) -> String {
    let mut highlighter = HighlightLines::new(syntax, &syntax_theme.theme);
    let mut html = String::new();
    for line in LinesWithEndings::from(source) {
        if line.len() > HIGHLIGHT_MAX_LINE {
            html.push_str(&crate::render::html_escape(line));
            continue;
        }
        let Ok(regions) = highlighter.highlight_line(line, ss) else {
            html.push_str(&crate::render::html_escape(line));
            continue;
        };
        let _ = append_highlighted_html_for_styled_line(&regions, IncludeBackground::No, &mut html);
    }
    html
}

/// Highlight `lines` as markdown, one balanced HTML string per line.
///
/// Parser state carries across lines, so multi-line constructs such as fenced
/// code and front matter colour the same as in [`highlight_source`]. Byte
/// ranges in `marks[i]` are wrapped in `<{mark_tag} class="diff-word">` as the
/// innermost element; a missing or empty entry means no marks.
pub(crate) fn highlight_line_segments(
    lines: &[&str],
    syntax_theme: Option<&SyntaxTheme>,
    marks: &[Vec<std::ops::Range<usize>>],
    mark_tag: &'static str,
) -> Vec<String> {
    let ss = syntax_set();
    let syntax = ss
        .find_syntax_by_extension("md")
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let marks_for = |i: usize| marks.get(i).map(Vec::as_slice).unwrap_or(&[]);
    match syntax_theme {
        None => {
            let mut state = ParseState::new(syntax);
            let mut stack = ScopeStack::new();
            lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    if line.len() > HIGHLIGHT_MAX_LINE {
                        return plain_line(line, marks_for(i), mark_tag);
                    }
                    classed_line(line, &mut state, &mut stack, ss, marks_for(i), mark_tag)
                })
                .collect()
        }
        Some(st) => {
            let mut highlighter = HighlightLines::new(syntax, &st.theme);
            lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    if line.len() > HIGHLIGHT_MAX_LINE {
                        return plain_line(line, marks_for(i), mark_tag);
                    }
                    let with_newline = format!("{line}\n");
                    let mut out = String::new();
                    match highlighter.highlight_line(&with_newline, ss) {
                        Ok(regions) => {
                            let mut offset = 0;
                            for (style, text) in regions {
                                let end = (offset + text.len()).min(line.len());
                                if offset < end {
                                    out.push_str(&style_open(&style));
                                    push_marked(
                                        &mut out,
                                        line,
                                        offset..end,
                                        marks_for(i),
                                        mark_tag,
                                    );
                                    out.push_str("</span>");
                                }
                                offset += text.len();
                            }
                        }
                        Err(_) => {
                            push_marked(&mut out, line, 0..line.len(), marks_for(i), mark_tag)
                        }
                    }
                    out
                })
                .collect()
        }
    }
}

fn plain_line(line: &str, marks: &[std::ops::Range<usize>], mark_tag: &str) -> String {
    let mut out = String::new();
    push_marked(&mut out, line, 0..line.len(), marks, mark_tag);
    out
}

fn classed_line(
    line: &str,
    state: &mut ParseState,
    stack: &mut ScopeStack,
    ss: &SyntaxSet,
    marks: &[std::ops::Range<usize>],
    mark_tag: &str,
) -> String {
    let mut out = String::new();
    let Ok(ops) = state.parse_line(&format!("{line}\n"), ss) else {
        push_marked(&mut out, line, 0..line.len(), marks, mark_tag);
        return out;
    };
    // Spans may span lines in syntect's output; reopen the stack so each line stands alone
    for &scope in stack.as_slice() {
        open_classed(&mut out, scope);
    }
    let mut cur = 0;
    for (index, op) in &ops {
        let index = (*index).min(line.len());
        if index > cur {
            push_marked(&mut out, line, cur..index, marks, mark_tag);
            cur = index;
        }
        let applied = stack.apply_with_hook(op, |basic, _| match basic {
            BasicScopeStackOp::Push(scope) => open_classed(&mut out, scope),
            BasicScopeStackOp::Pop => out.push_str("</span>"),
        });
        if applied.is_err() {
            break;
        }
    }
    push_marked(&mut out, line, cur..line.len(), marks, mark_tag);
    for _ in 0..stack.len() {
        out.push_str("</span>");
    }
    out
}

// Same class names as syntect's `ClassStyle::Spaced`, so `syntax.css` applies
fn open_classed(out: &mut String, scope: Scope) {
    out.push_str("<span class=\"");
    out.push_str(&scope.build_string().replace('.', " "));
    out.push_str("\">");
}

// Mirrors syntect's inline-style span, which can't nest word marks inside its spans
fn style_open(style: &Style) -> String {
    let mut css = String::new();
    if style.font_style.contains(FontStyle::UNDERLINE) {
        css.push_str("text-decoration:underline;");
    }
    if style.font_style.contains(FontStyle::BOLD) {
        css.push_str("font-weight:bold;");
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        css.push_str("font-style:italic;");
    }
    css.push_str(&format!(
        "color:{};",
        crate::theme::color_to_hex(style.foreground)
    ));
    format!("<span style=\"{css}\">")
}

/// Escape `line[range]`, wrapping the parts covered by `marks` in `mark_tag`.
fn push_marked(
    out: &mut String,
    line: &str,
    range: std::ops::Range<usize>,
    marks: &[std::ops::Range<usize>],
    mark_tag: &str,
) {
    let mut pos = range.start;
    while pos < range.end {
        let inside = marks.iter().find(|m| m.start <= pos && pos < m.end);
        let next = match inside {
            Some(m) => m.end.min(range.end),
            None => marks
                .iter()
                .map(|m| m.start)
                .filter(|&start| start > pos)
                .min()
                .unwrap_or(range.end)
                .min(range.end),
        };
        let Some(text) = line.get(pos..next) else {
            break;
        };
        let escaped = escape_text(text);
        match inside {
            Some(_) => {
                out.push_str(&format!(
                    "<{mark_tag} class=\"diff-word\">{escaped}</{mark_tag}>"
                ));
            }
            None => out.push_str(&escaped),
        }
        pos = next;
    }
}

fn escape_text(text: &str) -> String {
    crate::render::html_escape(text).replace('\r', "&#13;")
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub(crate) fn default_syntax_theme() -> SyntaxTheme {
        SyntaxTheme {
            theme: ThemeSet::load_defaults().themes["InspiredGitHub"].clone(),
            theme_name: "InspiredGitHub".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::tests_support::default_syntax_theme;

    const CRLF_SOURCE: &str = "# H1\r\n## H2 text\r\n\r\nbody\rlone cr\r\n";

    #[test]
    fn highlight_source_escapes_carriage_returns_css_class_mode() {
        let html = highlight_source(CRLF_SOURCE, None);
        assert!(
            !html.contains('\r'),
            "raw CR would be normalized by the HTML parser: {html:?}"
        );
        assert_eq!(html.matches("&#13;").count(), 5);
    }

    #[test]
    fn highlight_source_escapes_carriage_returns_styled_mode() {
        let html = highlight_source(CRLF_SOURCE, Some(&default_syntax_theme()));
        assert!(
            !html.contains('\r'),
            "raw CR would be normalized by the HTML parser: {html:?}"
        );
        assert_eq!(html.matches("&#13;").count(), 5);
    }

    #[test]
    fn highlight_source_css_class_mode() {
        let html = highlight_source("# Hello\n", None);
        assert!(
            html.contains("<span"),
            "CSS-class mode should produce span elements, got: {html}"
        );
        assert!(
            !html.contains("style="),
            "CSS-class mode should not produce inline styles, got: {html}"
        );
    }

    #[test]
    fn highlight_source_preserves_content() {
        let html = highlight_source("hello world\n", None);
        assert!(
            html.contains("hello world"),
            "highlighted output should contain the source text, got: {html}"
        );
    }

    #[test]
    fn highlight_source_empty_input() {
        let html = highlight_source("", None);
        // Empty input should return something (possibly empty) without panicking
        assert!(html.is_empty() || html.contains("<span") || html.contains("</span>"));
    }

    fn strip_tags(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#13;", "\r")
            .replace("&amp;", "&")
    }

    const LINES: &[&str] = &[
        "---",
        "title: \"a & b\"",
        "---",
        "# Heading <x>",
        "",
        "```rust",
        "fn main() { let s = \"þú ✓\"; }",
        "```",
        "tail\rcr",
    ];

    #[test]
    fn line_segments_are_balanced_and_lossless_in_both_modes() {
        let theme = default_syntax_theme();
        for mode in [None, Some(&theme)] {
            let out = highlight_line_segments(LINES, mode, &[], "del");
            assert_eq!(out.len(), LINES.len());
            for (html, line) in out.iter().zip(LINES) {
                assert_eq!(
                    html.matches("<span").count(),
                    html.matches("</span>").count(),
                    "{html}"
                );
                assert_eq!(&strip_tags(html), line, "{html}");
                assert!(!html.contains('\r'), "{html:?}");
            }
        }
    }

    #[test]
    fn line_segments_carry_state_into_fenced_code() {
        let out = highlight_line_segments(LINES, None, &[], "del");
        // Highlighted alone this line is a paragraph; only carried state knows it is fenced
        assert!(out[6].contains("code-fence"), "{}", out[6]);
        let alone = highlight_line_segments(&[LINES[6]], None, &[], "del");
        assert!(!alone[0].contains("code-fence"), "{}", alone[0]);
    }

    #[test]
    fn line_segments_wrap_marks_innermost() {
        let line = "# Hello world";
        let marks = vec![std::ops::Range { start: 8, end: 13 }];
        for mode in [None, Some(&default_syntax_theme())] {
            let out = highlight_line_segments(&[line], mode, std::slice::from_ref(&marks), "ins");
            let html = &out[0];
            assert!(
                html.contains("<ins class=\"diff-word\">world</ins>"),
                "{html}"
            );
            let ins_at = html.find("<ins").unwrap();
            assert!(
                html[..ins_at].contains("<span"),
                "mark should sit inside a token span: {html}"
            );
            assert_eq!(strip_tags(html), line);
        }
    }

    #[test]
    fn line_segments_split_marks_across_tokens() {
        let line = "**bold** plain";
        let marks = vec![std::ops::Range { start: 4, end: 11 }];
        let out = highlight_line_segments(&[line], None, std::slice::from_ref(&marks), "del");
        let html = &out[0];
        assert!(
            html.matches("<del class=\"diff-word\">").count() >= 2,
            "{html}"
        );
        assert_eq!(
            html.matches("<span").count(),
            html.matches("</span>").count()
        );
        assert_eq!(strip_tags(html), line);
    }

    #[test]
    fn overlong_lines_are_escaped_without_spans() {
        let long = "<b>x</b> ".repeat(HIGHLIGHT_MAX_LINE / 4);
        let lines = ["# Title", long.as_str()];
        let out = highlight_line_segments(&lines, None, &[], "del");
        assert!(out[0].contains("<span"));
        assert!(!out[1].contains("<span") && out[1].starts_with("&lt;b&gt;"));
    }

    #[test]
    fn highlight_source_matches_syntect_generators() {
        let source = "---\ntitle: x\n---\n# Title\r\n\n```rust\nfn main() {}\n```\n\n> *quote* `code` [link](url)\n";

        // The reference: a fresh syntax set and syntect's own generators
        let ss = SyntaxSet::load_defaults_newlines();
        let syntax = ss.find_syntax_by_extension("md").unwrap();
        let mut generator = syntect::html::ClassedHTMLGenerator::new_with_class_style(
            syntax,
            &ss,
            ClassStyle::Spaced,
        );
        for line in LinesWithEndings::from(source) {
            generator
                .parse_html_for_line_which_includes_newline(line)
                .unwrap();
        }
        let classed = generator.finalize().replace('\r', "&#13;");
        assert_eq!(highlight_source(source, None), classed);

        let theme = default_syntax_theme();
        let mut highlighter = HighlightLines::new(syntax, &theme.theme);
        let mut styled = String::new();
        for line in LinesWithEndings::from(source) {
            let regions = highlighter.highlight_line(line, &ss).unwrap();
            append_highlighted_html_for_styled_line(&regions, IncludeBackground::No, &mut styled)
                .unwrap();
        }
        assert_eq!(
            highlight_source(source, Some(&theme)),
            styled.replace('\r', "&#13;")
        );
    }

    #[test]
    fn highlight_source_leaves_only_long_lines_plain() {
        let data_uri = format!("![img](data:image/png;base64,{})", "QUJD".repeat(5_000));
        let row = format!("| {} <b> |", "cell ".repeat(1_900));
        let source = format!("# Title\n\n{data_uri}\n\n{row}\n\n**bold**\n");
        assert!(data_uri.len() > HIGHLIGHT_MAX_LINE && row.len() > HIGHLIGHT_MAX_LINE);

        let theme = default_syntax_theme();
        for mode in [None, Some(&theme)] {
            let html = highlight_source(&source, mode);
            let lines: Vec<&str> = html.split('\n').collect();
            assert!(lines[0].contains("<span"), "{}", lines[0]);
            assert!(lines[6].contains("<span"), "{}", lines[6]);
            for long in [lines[2], lines[4]] {
                let text = long.trim_start_matches("</span>");
                assert!(!text.contains("<span"), "long line was highlighted");
            }
            assert!(lines[4].contains("&lt;b&gt;"));
            assert_eq!(strip_tags(&html), source);
        }
    }

    #[test]
    fn highlight_source_is_fast_on_a_huge_line() {
        let source = format!("# Title\n\n{}\n", "*a* _b_ `c` [d](e) ".repeat(50_000));
        let started = std::time::Instant::now();
        for mode in [None, Some(&default_syntax_theme())] {
            highlight_source(&source, mode);
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }
}
