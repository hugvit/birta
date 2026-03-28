use crate::theme::ResolvedTheme;

const VIEWER_HTML: &str = include_str!("../assets/viewer.html");
const GITHUB_CSS: &str = include_str!("../assets/github-markdown.css");
const PAGE_CSS: &str = include_str!("../assets/page.css");
const SYNTAX_CSS: &str = include_str!("../assets/syntax.css");
const ALERTS_CSS: &str = include_str!("../assets/alerts.css");
const THEME_OVERRIDES: &str = include_str!("../assets/theme-overrides.css");

pub fn render_page(
    filename: &str,
    content_html: &str,
    custom_css: Option<&str>,
    font_css: Option<&str>,
    show_header: bool,
    theme: &ResolvedTheme,
    theme_names: &[&str],
) -> String {
    let custom_style = match custom_css {
        Some(css) => format!("<style>{css}</style>"),
        None => String::new(),
    };

    let active = theme.active_data();
    let is_github = theme.is_github();

    // Always include syntax.css — it provides CSS-class-based highlighting
    // for the github theme. When a tmTheme is active (custom themes), syntect
    // emits inline styles that naturally override these class rules. Keeping
    // it present ensures theme hot-swap to github works correctly.
    let syntax_css = SYNTAX_CSS;

    // GitHub uses the vendored CSS untouched — theme-overrides.css provides
    // standalone [data-theme] selectors so the dark/light toggle works
    // regardless of OS preference. Always included since custom theme CSS vars
    // (injected after, in the theme-vars block) override at equal specificity.
    let theme_vars_css = if is_github {
        String::new()
    } else {
        active.css_vars.clone()
    };

    // data-sheen-theme gates alert color overrides — skip for github
    // so the vendored github-markdown.css alert rules apply untouched.
    let theme_attr = if is_github {
        String::new()
    } else {
        format!("data-sheen-theme=\"{}\"", theme.name)
    };

    // Theme mode for the browser JS
    let theme_mode = if theme.has_toggle() {
        "toggle"
    } else {
        match theme.active_variant {
            crate::theme::Variant::Light => "fixed-light",
            crate::theme::Variant::Dark => "fixed-dark",
        }
    };

    // Active variant for the browser JS
    let active_variant = theme.active_variant.as_str();

    // Theme dropdown options (for hot-swap)
    let theme_options: String = theme_names
        .iter()
        .map(|&name| {
            let selected = if name == theme.name { " selected" } else { "" };
            format!("<option value=\"{name}\"{selected}>{name}</option>")
        })
        .collect::<Vec<_>>()
        .join("\n      ");

    // Available variants for JS
    let variants_json =
        serde_json::to_string(&theme.variant_names()).unwrap_or_else(|_| "[]".to_string());

    VIEWER_HTML
        .replace("{{GITHUB_CSS}}", GITHUB_CSS)
        .replace("{{THEME_OVERRIDES}}", THEME_OVERRIDES)
        .replace("{{PAGE_CSS}}", PAGE_CSS)
        .replace("{{SYNTAX_CSS}}", syntax_css)
        .replace("{{ALERTS_CSS}}", ALERTS_CSS)
        .replace("{{THEME_VARS_CSS}}", &theme_vars_css)
        .replace("{{FONT_CSS}}", font_css.unwrap_or(""))
        .replace("{{CUSTOM_CSS}}", &custom_style)
        .replace("{{HEADER_CLASS}}", if show_header { "" } else { " header-hidden" })
        .replace("{{THEME_MODE}}", theme_mode)
        .replace("{{THEME_ATTR}}", &theme_attr)
        .replace("{{ACTIVE_VARIANT}}", active_variant)
        .replace("{{THEME_OPTIONS}}", &theme_options)
        .replace("{{VARIANTS_JSON}}", &variants_json)
        .replace("{{FILENAME}}", filename)
        .replace("{{CONTENT}}", content_html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ResolvedTheme, ThemeVariants, Variant, VariantData};

    fn github_theme() -> ResolvedTheme {
        ResolvedTheme {
            name: "github".to_string(),
            variants: ThemeVariants::Both {
                light: Box::new(VariantData {
                    css_vars: String::new(),
                    syntax: None,
                }),
                dark: Box::new(VariantData {
                    css_vars: String::new(),
                    syntax: None,
                }),
            },
            active_variant: Variant::Dark,
        }
    }

    #[test]
    fn render_page_contains_filename() {
        let page = render_page(
            "test.md",
            "<p>hello</p>",
            None,
            None,
            true,
            &github_theme(),
            &["github"],
        );
        assert!(
            page.contains("test.md"),
            "rendered page should contain the filename"
        );
    }

    #[test]
    fn render_page_contains_content() {
        let page = render_page(
            "test.md",
            "<p>hello</p>",
            None,
            None,
            true,
            &github_theme(),
            &["github"],
        );
        assert!(
            page.contains("<p>hello</p>"),
            "rendered page should contain the content HTML"
        );
    }

    #[test]
    fn render_page_contains_markdown_body_class() {
        let page = render_page("test.md", "", None, None, true, &github_theme(), &["github"]);
        assert!(
            page.contains("markdown-body"),
            "rendered page should contain the markdown-body class"
        );
    }

    #[test]
    fn render_page_contains_github_css() {
        let page = render_page("test.md", "", None, None, true, &github_theme(), &["github"]);
        assert!(
            page.contains(".markdown-body"),
            "rendered page should contain github-markdown-css rules"
        );
    }

    #[test]
    fn render_page_includes_custom_css() {
        let page = render_page(
            "test.md",
            "",
            Some("body { color: red; }"),
            None,
            true,
            &github_theme(),
            &["github"],
        );
        assert!(
            page.contains("body { color: red; }"),
            "rendered page should contain the custom CSS"
        );
    }
}
