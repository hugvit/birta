use comrak::plugins::syntect::SyntectAdapter;

/// Create a syntax highlighter adapter using CSS classes (theme-agnostic).
///
/// Light/dark theming is handled by `syntax.css` in the browser.
pub fn adapter() -> SyntectAdapter {
    SyntectAdapter::new(None)
}
