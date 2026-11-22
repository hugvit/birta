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
