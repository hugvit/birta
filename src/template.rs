use crate::theme::ResolvedTheme;

const VIEWER_HTML: &str = include_str!("../assets/viewer.html");
const GITHUB_CSS: &str = include_str!("../assets/github-markdown.css");
const PAGE_CSS: &str = include_str!("../assets/page.css");
const SYNTAX_CSS: &str = include_str!("../assets/syntax.css");
const ALERTS_CSS: &str = include_str!("../assets/alerts.css");
const THEME_OVERRIDES: &str = include_str!("../assets/theme-overrides.css");

pub struct PageOptions<'a> {
    pub filename: &'a str,
    pub content_html: &'a str,
    pub custom_css: Option<&'a str>,
    pub font_css: Option<&'a str>,
    pub show_header: bool,
    pub reading_mode: bool,
    pub theme: &'a ResolvedTheme,
    pub theme_names: &'a [&'a str],
    pub static_mode: bool,
    pub keybindings_json: &'a str,
}

pub fn render_page(opts: &PageOptions<'_>) -> String {
    let PageOptions {
        filename,
        content_html,
        custom_css,
        font_css,
        show_header,
        reading_mode,
        theme,
        theme_names,
        ..
    } = opts;
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

    // data-birta-theme gates alert color overrides — skip for github
    // so the vendored github-markdown.css alert rules apply untouched.
    let theme_attr = if is_github {
        String::new()
    } else {
        format!("data-birta-theme=\"{}\"", theme.name)
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
        .replace(
            "{{HEADER_CLASS}}",
            if *show_header { "" } else { " header-hidden" },
        )
        .replace(
            "{{BODY_CLASS}}",
            if *reading_mode { " reading-mode" } else { "" },
        )
        .replace("{{THEME_MODE}}", theme_mode)
        .replace("{{THEME_ATTR}}", &theme_attr)
        .replace("{{ACTIVE_VARIANT}}", active_variant)
        .replace("{{THEME_OPTIONS}}", &theme_options)
        .replace("{{VARIANTS_JSON}}", &variants_json)
        .replace("{{FILENAME}}", filename)
        .replace(
            "{{STATIC_MODE}}",
            if opts.static_mode { "true" } else { "false" },
        )
        .replace("{{KEYBINDINGS_JSON}}", opts.keybindings_json)
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

    fn test_page(content: &str, custom_css: Option<&str>) -> String {
        let theme = github_theme();
        render_page(&PageOptions {
            filename: "test.md",
            content_html: content,
            custom_css,
            font_css: None,
            show_header: true,
            reading_mode: false,
            theme: &theme,
            theme_names: &["github"],
            static_mode: false,
            keybindings_json: "{}",
        })
    }

    #[test]
    fn render_page_contains_filename() {
        let page = test_page("<p>hello</p>", None);
        assert!(page.contains("test.md"));
    }

    #[test]
    fn render_page_contains_content() {
        let page = test_page("<p>hello</p>", None);
        assert!(page.contains("<p>hello</p>"));
    }

    #[test]
    fn render_page_contains_markdown_body_class() {
        let page = test_page("", None);
        assert!(page.contains("markdown-body"));
    }

    #[test]
    fn render_page_contains_github_css() {
        let page = test_page("", None);
        assert!(page.contains(".markdown-body"));
    }

    #[test]
    fn render_page_includes_custom_css() {
        let page = test_page("", Some("body { color: red; }"));
        assert!(page.contains("body { color: red; }"));
    }

    /// Build PageOptions with full control over all fields.
    fn render_with(
        show_header: bool,
        reading_mode: bool,
        static_mode: bool,
        font_css: Option<&str>,
        theme: &ResolvedTheme,
        theme_names: &[&str],
    ) -> String {
        render_page(&PageOptions {
            filename: "test.md",
            content_html: "<p>test</p>",
            custom_css: None,
            font_css,
            show_header,
            reading_mode,
            theme,
            theme_names,
            static_mode,
            keybindings_json: "{}",
        })
    }

    fn dark_only_theme() -> ResolvedTheme {
        ResolvedTheme {
            name: "dracula".to_string(),
            variants: ThemeVariants::Single(Box::new(VariantData {
                css_vars: ":root { --birta-fg: #f8f8f2; }".to_string(),
                syntax: None,
            })),
            active_variant: Variant::Dark,
        }
    }

    #[test]
    fn render_page_hidden_header() {
        let theme = github_theme();
        let page = render_with(false, false, false, None, &theme, &["github"]);
        assert!(
            page.contains("class=\"header header-hidden\""),
            "should add header-hidden class when show_header is false"
        );
    }

    #[test]
    fn render_page_visible_header_has_no_hidden_class() {
        let theme = github_theme();
        let page = render_with(true, false, false, None, &theme, &["github"]);
        assert!(
            page.contains("class=\"header\""),
            "header should have no hidden class when show_header is true"
        );
    }

    #[test]
    fn render_page_reading_mode_class() {
        let theme = github_theme();
        let page = render_with(true, true, false, None, &theme, &["github"]);
        assert!(
            page.contains("<body class=\" reading-mode\">"),
            "should add reading-mode to body class"
        );
    }

    #[test]
    fn render_page_no_reading_mode_class() {
        let theme = github_theme();
        let page = render_with(true, false, false, None, &theme, &["github"]);
        assert!(
            page.contains("<body class=\"\">"),
            "body class should be empty when reading mode is disabled"
        );
    }

    #[test]
    fn render_page_static_mode_true() {
        let theme = github_theme();
        let page = render_with(true, false, true, None, &theme, &["github"]);
        assert!(
            page.contains("var STATIC_MODE = true;"),
            "should set STATIC_MODE to true"
        );
    }

    #[test]
    fn render_page_static_mode_false() {
        let theme = github_theme();
        let page = render_with(true, false, false, None, &theme, &["github"]);
        assert!(
            page.contains("var STATIC_MODE = false;"),
            "should set STATIC_MODE to false"
        );
    }

    #[test]
    fn render_page_theme_mode_toggle() {
        let theme = github_theme();
        let page = render_with(true, false, false, None, &theme, &["github"]);
        assert!(
            page.contains("var THEME_MODE = 'toggle';"),
            "dual-variant theme should produce toggle mode"
        );
    }

    #[test]
    fn render_page_theme_mode_fixed_dark() {
        let theme = dark_only_theme();
        let page = render_with(true, false, false, None, &theme, &["dracula"]);
        assert!(
            page.contains("var THEME_MODE = 'fixed-dark';"),
            "single dark-only theme should produce fixed-dark mode"
        );
    }

    #[test]
    fn render_page_theme_mode_fixed_light() {
        let theme = ResolvedTheme {
            name: "custom-light".to_string(),
            variants: ThemeVariants::Single(Box::new(VariantData {
                css_vars: String::new(),
                syntax: None,
            })),
            active_variant: Variant::Light,
        };
        let page = render_with(true, false, false, None, &theme, &["custom-light"]);
        assert!(
            page.contains("var THEME_MODE = 'fixed-light';"),
            "single light-only theme should produce fixed-light mode"
        );
    }

    #[test]
    fn render_page_font_css_injected() {
        let theme = github_theme();
        let css = ".markdown-body { font-family: Georgia, serif !important; }";
        let page = render_with(true, false, false, Some(css), &theme, &["github"]);
        assert!(
            page.contains(css),
            "font CSS should appear in rendered page"
        );
    }

    #[test]
    fn render_page_theme_dropdown_options() {
        let theme = github_theme();
        let page = render_with(true, false, false, None, &theme, &["github", "dracula"]);
        assert!(
            page.contains("<option value=\"github\" selected>github</option>"),
            "active theme should be selected in dropdown"
        );
        assert!(
            page.contains("<option value=\"dracula\">dracula</option>"),
            "other themes should appear in dropdown"
        );
    }

    #[test]
    fn render_page_custom_theme_sets_data_attr() {
        let theme = dark_only_theme();
        let page = render_with(true, false, false, None, &theme, &["dracula"]);
        assert!(
            page.contains("data-birta-theme=\"dracula\""),
            "non-github theme should set data-birta-theme attribute"
        );
    }

    #[test]
    fn render_page_github_theme_no_data_attr() {
        let theme = github_theme();
        let page = render_with(true, false, false, None, &theme, &["github"]);
        // The html tag should not have data-birta-theme for github
        assert!(
            !page.contains("data-birta-theme=\"github\""),
            "github theme should not set data-birta-theme attribute on html element"
        );
    }
}
