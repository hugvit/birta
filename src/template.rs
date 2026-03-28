use crate::theme::ResolvedTheme;

const VIEWER_HTML: &str = include_str!("../assets/viewer.html");
const GITHUB_CSS: &str = include_str!("../assets/github-markdown.css");
const PAGE_CSS: &str = include_str!("../assets/page.css");
const SYNTAX_CSS: &str = include_str!("../assets/syntax.css");
const ALERTS_CSS: &str = include_str!("../assets/alerts.css");

pub fn render_page(
    filename: &str,
    content_html: &str,
    custom_css: Option<&str>,
    theme: &ResolvedTheme,
    theme_names: &[&str],
) -> String {
    let custom_style = match custom_css {
        Some(css) => format!("<style>{css}</style>"),
        None => String::new(),
    };

    let active = theme.active_data();
    let has_syntax_theme = active.syntax.is_some();

    // When a syntax theme is active, omit syntax.css (inline styles replace it)
    let syntax_css = if has_syntax_theme { "" } else { SYNTAX_CSS };

    // Theme CSS variables — injected as a replaceable <style> block
    // All themes (including github) need this for variant toggling to work,
    // since the vendored github-markdown.css wraps [data-theme] inside
    // @media (prefers-color-scheme) queries that only fire when OS matches.
    let theme_vars_css = active.css_vars.clone();

    // Theme attribute on <html> — gates alert color overrides in alerts.css
    let theme_attr = format!("data-sheen-theme=\"{}\"", theme.name);

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
        .replace("{{PAGE_CSS}}", PAGE_CSS)
        .replace("{{SYNTAX_CSS}}", syntax_css)
        .replace("{{ALERTS_CSS}}", ALERTS_CSS)
        .replace("{{THEME_VARS_CSS}}", &theme_vars_css)
        .replace("{{CUSTOM_CSS}}", &custom_style)
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
        let page = render_page("test.md", "", None, &github_theme(), &["github"]);
        assert!(
            page.contains("markdown-body"),
            "rendered page should contain the markdown-body class"
        );
    }

    #[test]
    fn render_page_contains_github_css() {
        let page = render_page("test.md", "", None, &github_theme(), &["github"]);
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
            &github_theme(),
            &["github"],
        );
        assert!(
            page.contains("body { color: red; }"),
            "rendered page should contain the custom CSS"
        );
    }
}
