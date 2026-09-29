use crate::theme::ResolvedTheme;

const VIEWER_HTML: &str = include_str!("../assets/viewer.html");
const GITHUB_CSS: &str = include_str!("../assets/github-markdown.css");
const PAGE_CSS: &str = include_str!("../assets/page.css");
const SYNTAX_CSS: &str = include_str!("../assets/syntax.css");
const ALERTS_CSS: &str = include_str!("../assets/alerts.css");
const THEME_OVERRIDES: &str = include_str!("../assets/theme-overrides.css");

pub struct PageOptions<'a> {
    pub filename: &'a str,
    pub file_stats: &'a str,
    pub content_html: &'a str,
    pub source_html: Option<&'a str>,
    pub custom_css: Option<&'a str>,
    pub font_css: Option<&'a str>,
    pub show_header: bool,
    pub reading_mode: bool,
    pub raw_mode: bool,
    pub theme: &'a ResolvedTheme,
    pub theme_names: &'a [&'a str],
    /// True when the variant was explicitly chosen via `--light`/`--dark` or
    /// config `theme.variant`. When true, the browser must not override it with
    /// the OS `prefers-color-scheme` preference.
    pub variant_explicit: bool,
    pub static_mode: bool,
    pub keybindings_json: &'a str,
    pub current_path: Option<&'a str>,
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

    // Whether the active variant was explicitly requested (gates the browser's
    // OS-preference fallback so `--light`/`--dark` is honoured).
    let variant_explicit = if opts.variant_explicit {
        "true"
    } else {
        "false"
    };

    // Theme dropdown items (for hot-swap)
    let theme_options: String = theme_names
        .iter()
        .map(|&name| {
            let active = if name == theme.name { " active" } else { "" };
            format!(
                "<button class=\"theme-dropdown-item{active}\" data-theme=\"{name}\">{name}</button>"
            )
        })
        .collect::<Vec<_>>()
        .join("\n            ");

    // Available variants for JS
    let variants_json =
        serde_json::to_string(&theme.variant_names()).unwrap_or_else(|_| "[]".to_string());

    fill_template(
        VIEWER_HTML,
        &[
            ("GITHUB_CSS", GITHUB_CSS),
            ("THEME_OVERRIDES", THEME_OVERRIDES),
            ("PAGE_CSS", PAGE_CSS),
            ("SYNTAX_CSS", syntax_css),
            ("ALERTS_CSS", ALERTS_CSS),
            ("THEME_VARS_CSS", &theme_vars_css),
            ("FONT_CSS", font_css.unwrap_or("")),
            ("CUSTOM_CSS", &custom_style),
            (
                "HEADER_CLASS",
                if *show_header { "" } else { " header-hidden" },
            ),
            (
                "BODY_CLASS",
                if *reading_mode { " reading-mode" } else { "" },
            ),
            ("THEME_MODE", theme_mode),
            ("THEME_ATTR", &theme_attr),
            ("ACTIVE_VARIANT", active_variant),
            ("VARIANT_EXPLICIT", variant_explicit),
            ("ACTIVE_THEME", &theme.name),
            ("THEME_OPTIONS", &theme_options),
            ("VARIANTS_JSON", &variants_json),
            ("FILENAME", filename),
            ("FILE_STATS", opts.file_stats),
            (
                "STATIC_MODE",
                if opts.static_mode { "true" } else { "false" },
            ),
            ("KEYBINDINGS_JSON", opts.keybindings_json),
            (
                "INITIAL_VIEW",
                if opts.raw_mode { "raw" } else { "preview" },
            ),
            ("CURRENT_PATH", opts.current_path.unwrap_or("")),
            ("SOURCE_HTML", opts.source_html.unwrap_or("")),
            ("CONTENT", content_html),
        ],
    )
}

/// Fills `{{KEY}}` placeholders in a single pass, so substituted values are
/// never rescanned — user content containing `{{CONTENT}}` etc. stays literal.
fn fill_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out =
        String::with_capacity(template.len() + vars.iter().map(|(_, v)| v.len()).sum::<usize>());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let value = after.find("}}").and_then(|end| {
            vars.iter()
                .find(|(k, _)| *k == &after[..end])
                .map(|(_, v)| (end, *v))
        });
        match value {
            Some((end, v)) => {
                out.push_str(&rest[..start]);
                out.push_str(v);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[..start + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
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
            file_stats: "1 lines (1 loc) · 5 B",
            content_html: content,
            source_html: None,
            custom_css,
            font_css: None,
            show_header: true,
            reading_mode: false,
            raw_mode: false,
            theme: &theme,
            theme_names: &["github"],
            variant_explicit: false,
            static_mode: false,
            keybindings_json: "{}",
            current_path: None,
        })
    }

    #[test]
    fn render_page_contains_filename() {
        let page = test_page("<p>hello</p>", None);
        assert!(page.contains("test.md"));
    }

    #[test]
    fn render_page_contains_file_stats() {
        let page = test_page("<p>hello</p>", None);
        assert!(page.contains("1 lines (1 loc) · 5 B"));
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
            file_stats: "1 lines (1 loc) · 7 B",
            content_html: "<p>test</p>",
            source_html: None,
            custom_css: None,
            font_css,
            show_header,
            reading_mode,
            raw_mode: false,
            theme,
            theme_names,
            variant_explicit: false,
            static_mode,
            keybindings_json: "{}",
            current_path: None,
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

    /// Build a page with explicit control over the `variant_explicit` flag.
    fn render_with_explicit(variant_explicit: bool, theme: &ResolvedTheme) -> String {
        render_page(&PageOptions {
            filename: "test.md",
            file_stats: "1 lines (1 loc) · 7 B",
            content_html: "<p>test</p>",
            source_html: None,
            custom_css: None,
            font_css: None,
            show_header: true,
            reading_mode: false,
            raw_mode: false,
            theme,
            theme_names: &["github"],
            variant_explicit,
            static_mode: false,
            keybindings_json: "{}",
            current_path: None,
        })
    }

    #[test]
    fn render_page_variant_explicit_true() {
        let theme = github_theme();
        let page = render_with_explicit(true, &theme);
        assert!(
            page.contains("var VARIANT_EXPLICIT = true;"),
            "explicit --light/--dark should set VARIANT_EXPLICIT to true so the \
             browser does not override it with the OS preference"
        );
    }

    #[test]
    fn render_page_variant_explicit_false() {
        let theme = github_theme();
        let page = render_with_explicit(false, &theme);
        assert!(
            page.contains("var VARIANT_EXPLICIT = false;"),
            "without an explicit variant the browser should fall back to OS preference"
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
            page.contains("data-theme=\"github\">github</button>"),
            "active theme should appear in dropdown"
        );
        assert!(
            page.contains("theme-dropdown-item active"),
            "active theme should have active class"
        );
        assert!(
            page.contains("data-theme=\"dracula\">dracula</button>"),
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

    fn render_user_values(filename: &str, content: &str, source: &str) -> String {
        let theme = github_theme();
        render_page(&PageOptions {
            filename,
            file_stats: "1 lines (1 loc) · 5 B",
            content_html: content,
            source_html: Some(source),
            custom_css: None,
            font_css: None,
            show_header: true,
            reading_mode: false,
            raw_mode: false,
            theme: &theme,
            theme_names: &["github"],
            variant_explicit: false,
            static_mode: false,
            keybindings_json: "{}",
            current_path: None,
        })
    }

    #[test]
    fn render_page_fills_every_placeholder() {
        let page = render_user_values("test.md", "<p>hi</p>", "hi");
        assert!(!page.contains("{{"), "unfilled placeholder left in page");
    }

    #[test]
    fn render_page_does_not_expand_placeholders_in_source() {
        let source = "<code>{{CONTENT}} {{FILENAME}} {{SOURCE_HTML}}</code>";
        let page = render_user_values("test.md", "<p>RENDERED</p>", source);
        assert!(
            page.contains(source),
            "source placeholders must stay literal"
        );
        assert_eq!(page.matches("RENDERED").count(), 1);
    }

    #[test]
    fn render_page_does_not_expand_placeholders_in_content() {
        let content = "<p>{{SOURCE_HTML}} {{FILENAME}} {{PAGE_CSS}}</p>";
        let page = render_user_values("test.md", content, "SRC_MARKER");
        assert!(
            page.contains(content),
            "content placeholders must stay literal"
        );
        assert_eq!(page.matches("SRC_MARKER").count(), 1);
    }

    #[test]
    fn render_page_does_not_expand_placeholders_in_filename() {
        let page = render_user_values("{{CONTENT}}.md", "<p>RENDERED</p>", "");
        assert!(page.contains("{{CONTENT}}.md"));
        assert_eq!(page.matches("RENDERED").count(), 1);
    }

    #[test]
    fn fill_template_replaces_known_keys() {
        let out = fill_template("a{{X}}b{{Y}}{{X}}", &[("X", "1"), ("Y", "2")]);
        assert_eq!(out, "a1b21");
    }

    #[test]
    fn fill_template_keeps_unknown_and_unclosed_braces() {
        let out = fill_template("{{NOPE}} {{X}} {{ open", &[("X", "1")]);
        assert_eq!(out, "{{NOPE}} 1 {{ open");
    }

    #[test]
    fn fill_template_does_not_rescan_values() {
        let out = fill_template("{{A}}|{{B}}", &[("A", "{{B}}"), ("B", "{{A}}")]);
        assert_eq!(out, "{{B}}|{{A}}");
    }

    #[test]
    fn fill_template_handles_multibyte_text() {
        let out = fill_template("þ{{X}}ð🦀{{", &[("X", "日本")]);
        assert_eq!(out, "þ日本ð🦀{{");
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
