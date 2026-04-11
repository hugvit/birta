use comrak::plugins::syntect::{SyntectAdapter, SyntectAdapterBuilder};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::html::{
    ClassStyle, ClassedHTMLGenerator, IncludeBackground, append_highlighted_html_for_styled_line,
};
use syntect::parsing::SyntaxSet;
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
pub fn highlight_source(source: &str, syntax_theme: Option<&SyntaxTheme>) -> String {
    let ss = SyntaxSet::load_defaults_newlines();
    let syntax = ss
        .find_syntax_by_extension("md")
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    match syntax_theme {
        None => highlight_classed(source, syntax, &ss),
        Some(st) => highlight_styled(source, syntax, &ss, st),
    }
}

/// CSS-class mode: produces `<span class="keyword">` etc. matching `syntax.css`.
fn highlight_classed(
    source: &str,
    syntax: &syntect::parsing::SyntaxReference,
    ss: &SyntaxSet,
) -> String {
    let mut generator = ClassedHTMLGenerator::new_with_class_style(syntax, ss, ClassStyle::Spaced);
    for line in LinesWithEndings::from(source) {
        // Errors here are non-fatal — skip unparseable lines
        let _ = generator.parse_html_for_line_which_includes_newline(line);
    }
    generator.finalize()
}

/// Inline-style mode: produces `<span style="color:#cf222e">` etc. from a tmTheme.
fn highlight_styled(
    source: &str,
    syntax: &syntect::parsing::SyntaxReference,
    ss: &SyntaxSet,
    syntax_theme: &SyntaxTheme,
) -> String {
    let mut highlighter = HighlightLines::new(syntax, &syntax_theme.theme);
    let mut html = String::new();
    for line in LinesWithEndings::from(source) {
        let Ok(regions) = highlighter.highlight_line(line, ss) else {
            html.push_str(&crate::render::html_escape(line));
            continue;
        };
        let _ = append_highlighted_html_for_styled_line(&regions, IncludeBackground::No, &mut html);
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
