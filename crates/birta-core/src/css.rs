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
    }

    #[test]
    fn diff_uses_github_primer_colours_for_github() {
        // Primer diffBlob tokens: addition/deletion line, number and word backgrounds
        for token in [
            "#dafbe1", "#aceebb", "#ffebe9", "#ffcecb", "#ddf4ff", "#b6e3ff",
        ] {
            assert!(super::DIFF.contains(token), "light {token}");
        }
        for token in [
            "#2ea04326",
            "#3fb9504d",
            "#2ea04366",
            "#f851491a",
            "#f851494d",
            "#f8514966",
        ] {
            assert!(super::DIFF.contains(token), "dark {token}");
        }
        // Tokens sit on the data-theme element itself, so the nearest one decides the mode,
        // and the diff paints text and background from the same tokens as its rows
        assert!(super::DIFF.contains("[data-theme=\"light\"] {\n  --birta-diff-fg: #1f2328;"));
        assert!(super::DIFF.contains("[data-theme=\"dark\"] {\n  --birta-diff-fg: #f0f6fc;"));
        assert!(!super::DIFF.contains("[data-theme=\"light\"] .birta-diff"));
        assert!(
            super::DIFF
                .contains("color: var(--birta-diff-fg);\n  background: var(--birta-diff-bg);")
        );
        // Derived colours apply to every birta theme except github itself
        assert!(
            super::DIFF
                .contains("[data-birta-theme]:not([data-birta-theme=\"github\"]) .birta-diff {")
        );
    }

    #[test]
    fn diff_css_resets_syntax_spans_and_scopes_the_container() {
        assert!(super::DIFF.contains(".birta-diff .diff-text span {"));
        assert!(super::DIFF.contains(
            ".birta-diff[data-layout=\"auto\"] {\n  container: birta-diff / inline-size;"
        ));
        // Forced layouts must not become size containers, which collapse in shrink-to-fit parents
        assert_eq!(super::DIFF.matches("container: birta-diff").count(), 1);
    }

    #[test]
    fn syntax_colours_follow_the_nearest_data_theme() {
        assert!(super::SYNTAX.contains("[data-theme=\"light\"] {\n  --birta-syn-bg: #f6f8fa;"));
        assert!(super::SYNTAX.contains("[data-theme=\"dark\"] {\n  --birta-syn-bg: #161b22;"));
        // No rule may key a token colour on an ancestor's mode
        assert!(!super::SYNTAX.contains("[data-theme=\"dark\"] ."));
        assert!(!super::SYNTAX.contains(":root:not([data-theme"));
    }

    #[test]
    fn markdown_source_colours_apply_to_the_github_theme_only() {
        let guard = ":is(:root:not([data-birta-theme]), [data-birta-theme=\"github\"])";
        for scope in [
            ".markup.heading.markdown",
            ".markup.raw.inline.markdown",
            ".punctuation.definition.raw.markdown",
            ".punctuation.definition.bold.markdown",
            ".punctuation.definition.list_item.markdown",
            ".constant.other.language-name.markdown",
            ".markup.underline.link.markdown",
        ] {
            assert!(
                super::SYNTAX.contains(&format!("{guard} {scope}")),
                "{scope}"
            );
        }
        for (token, light, dark) in [
            ("md-heading", "#0550ae", "#1f6feb"),
            ("md-string", "#0a3069", "#a5d6ff"),
            ("md-code", "#0550ae", "#79c0ff"),
            ("md-bullet", "#953800", "#ffa657"),
            ("md-lang", "#6639ba", "#d2a8ff"),
        ] {
            assert!(
                super::SYNTAX.contains(&format!("--birta-syn-{token}: {light};")),
                "{token}"
            );
            assert!(
                super::SYNTAX.contains(&format!("--birta-syn-{token}: {dark};")),
                "{token}"
            );
        }
    }
}
