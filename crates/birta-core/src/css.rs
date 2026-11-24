//! The stylesheets that give rendered documents birta's look.
//!
//! Include them in this order, then the active theme's
//! [`VariantData::css_vars`](crate::theme::VariantData::css_vars), which
//! overrides the colours at equal specificity.

/// GitHub's markdown stylesheet. Styles everything inside `.markdown-body`.
pub const GITHUB_MARKDOWN: &str = include_str!("../assets/github-markdown.css");

/// `[data-theme="light"|"dark"]` selectors, so a light/dark toggle works
/// regardless of the OS colour scheme.
pub const THEME_OVERRIDES: &str = include_str!("../assets/theme-overrides.css");

/// Page-level styles: the `--birta-*` colour variables, the body background,
/// the highlighted-source view from
/// [`render_source`](crate::render::render_source), and the chrome of birta's
/// own viewer (header, theme controls, reading mode).
pub const PAGE: &str = include_str!("../assets/page.css");

/// Class-based code highlighting colours, used when rendering without a
/// [`SyntaxTheme`](crate::theme::SyntaxTheme).
pub const SYNTAX: &str = include_str!("../assets/syntax.css");

/// Colours for GitHub-style alerts (`> [!NOTE]`), driven by the `--alert-*`
/// variables. Applies when the root element has a `data-birta-theme` attribute.
pub const ALERTS: &str = include_str!("../assets/alerts.css");

/// Styles for [`diff`](crate::diff::diff) output, unified or side by side.
///
/// Include it after [`ALERTS`], plus [`SYNTAX`] when diffing without a
/// [`SyntaxTheme`](crate::theme::SyntaxTheme). Its colours come from the theme's
/// syntax diff scopes when the theme's CSS variables carry them, else from the
/// alert colours. In class mode the syntax classes inside a diff are unscoped:
/// a host that scopes its syntax CSS (e.g. to `pre.syntax-highlighting`) must
/// also scope it to `.birta-diff`.
pub const DIFF: &str = include_str!("../assets/diff.css");

#[cfg(test)]
mod tests {
    #[test]
    fn diff_colours_fall_back_from_syntax_theme_to_alerts() {
        assert!(super::DIFF.contains("var(--birta-diff-add, var(--alert-tip"));
        assert!(super::DIFF.contains("var(--birta-diff-del, var(--alert-caution"));
        assert!(super::DIFF.contains("var(--birta-diff-change, var(--alert-warning"));
    }
}
