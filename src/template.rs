use std::sync::OnceLock;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

use crate::anchor::Stop;
use crate::security::{html_escape_attr, json_for_script};
use crate::theme::ResolvedTheme;

const VIEWER_HTML: &str = include_str!("../assets/viewer.html");
const PATCH_JS: &str = include_str!("../assets/patch.js");
const GITHUB_CSS: &str = include_str!("../assets/github-markdown.css");
const PAGE_CSS: &str = include_str!("../assets/page.css");
const SYNTAX_CSS: &str = include_str!("../assets/syntax.css");
const ALERTS_CSS: &str = include_str!("../assets/alerts.css");
const THEME_OVERRIDES: &str = include_str!("../assets/theme-overrides.css");

/// Path prefixes of the pinned CDN releases `viewer.html` loads.
const KATEX_CDN: &str = "https://cdn.jsdelivr.net/npm/katex@0.16.21/";
const MERMAID_CDN: &str = "https://cdn.jsdelivr.net/npm/mermaid@11.17.2/";

/// Policy for exported pages. There is no server to send a header or a fresh nonce,
/// so the viewer script is allowed by hash and the CDN by exact release.
fn static_csp() -> &'static str {
    static CSP: OnceLock<String> = OnceLock::new();
    CSP.get_or_init(|| {
        let hash = STANDARD.encode(Sha256::digest(viewer_script().as_bytes()));
        format!(
            "script-src 'sha256-{hash}' {KATEX_CDN} {MERMAID_CDN}; \
             style-src 'unsafe-inline' https:; font-src https: data:; \
             img-src file: data: http: https: 'self'; media-src file: http: https: 'self'; \
             base-uri 'none'; form-action 'none'; object-src 'none'; frame-src https:"
        )
    })
}

/// Body of the viewer's inline script with the patcher filled in, byte for byte as served.
fn viewer_script() -> &'static str {
    static SCRIPT: OnceLock<String> = OnceLock::new();
    SCRIPT.get_or_init(|| {
        let open = "<script nonce=\"{{NONCE}}\">";
        let start = VIEWER_HTML
            .find(open)
            .expect("viewer.html has no inline script")
            + open.len();
        let len = VIEWER_HTML[start..]
            .find("</script>")
            .expect("unclosed inline script");
        fill_template(&VIEWER_HTML[start..start + len], &[("PATCH_JS", PATCH_JS)])
    })
}

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
    /// Git blob hash of the bytes rendered into `content_html`; checkbox writes send it back.
    pub rev: Option<&'a str>,
    pub nonce: &'a str,
    pub extra_boot: Option<&'a serde_json::Value>,
    /// The beacon token when review is on; `None` leaves every review surface out.
    pub review_token: Option<&'a str>,
    /// A folder is served, so review acts on every doc under it.
    pub review_dir: bool,
    /// The canonical path this server serves, which a page checks a restarted server against.
    pub review_served: Option<&'a str>,
    /// The shell command that serves the same path again.
    pub review_restart: Option<&'a str>,
    /// Focus stops of `content_html`, left out of BOOT when `None`.
    pub stops: Option<&'a [Stop]>,
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
        format!("data-birta-theme=\"{}\"", html_escape_attr(&theme.name))
    };

    let theme_mode = if theme.has_toggle() {
        "toggle"
    } else {
        match theme.active_variant {
            crate::theme::Variant::Light => "fixed-light",
            crate::theme::Variant::Dark => "fixed-dark",
        }
    };

    // Theme dropdown items (for hot-swap)
    let theme_options: String = theme_names
        .iter()
        .map(|&name| {
            let active = if name == theme.name { " active" } else { "" };
            let name = html_escape_attr(name);
            format!(
                "<button class=\"theme-dropdown-item{active}\" data-theme=\"{name}\">{name}</button>"
            )
        })
        .collect::<Vec<_>>()
        .join("\n            ");

    let keybindings: serde_json::Value =
        serde_json::from_str(opts.keybindings_json).unwrap_or_else(|_| serde_json::json!({}));
    // Read by the viewer script via JSON.parse, so that script is identical on every page
    let mut boot = serde_json::json!({
        "staticMode": opts.static_mode,
        "themeMode": theme_mode,
        "activeVariant": theme.active_variant.as_str(),
        "variantExplicit": opts.variant_explicit,
        "variants": theme.variant_names(),
        "keybindings": keybindings,
        "currentPath": opts.current_path.unwrap_or(""),
        "rev": opts.rev,
        "initialView": if opts.raw_mode { "raw" } else { "preview" },
        "extra": opts.extra_boot,
        "review": match opts.review_token {
            Some(token) => {
                let mut review =
                    serde_json::json!({"enabled": true, "token": token, "dir": opts.review_dir});
                if let (Some(served), Some(restart)) = (opts.review_served, opts.review_restart) {
                    review["served"] = served.into();
                    review["restart"] = restart.into();
                }
                review
            }
            None => serde_json::json!({"enabled": false}),
        },
    });
    if let Some(stops) = opts.stops {
        boot["stops"] = serde_json::json!(stops);
    }
    let boot_json = json_for_script(&boot);

    let review_tags = match opts.review_token {
        Some(_) => format!(
            "<link rel=\"stylesheet\" href=\"/_birta/review.css\">\n  \
             <script defer src=\"/_birta/review.js\" nonce=\"{}\"></script>",
            opts.nonce
        ),
        None => String::new(),
    };

    let csp_meta = if opts.static_mode {
        format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">",
            html_escape_attr(static_csp())
        )
    } else {
        String::new()
    };

    fill_template(
        VIEWER_HTML,
        &[
            ("CSP_META", &csp_meta),
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
            ("THEME_ATTR", &theme_attr),
            ("ACTIVE_THEME", &html_escape_attr(&theme.name)),
            ("THEME_OPTIONS", &theme_options),
            ("FILENAME", &html_escape_attr(filename)),
            ("FILE_STATS", &html_escape_attr(opts.file_stats)),
            ("BOOT_JSON", &boot_json),
            ("NONCE", opts.nonce),
            ("REVIEW_TAGS", &review_tags),
            ("PATCH_JS", PATCH_JS),
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
            nonce: "",
            extra_boot: None,
            review_token: None,
            review_dir: false,
            review_served: None,
            review_restart: None,
            stops: None,
            rev: None,
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
            nonce: "",
            extra_boot: None,
            review_token: None,
            review_dir: false,
            review_served: None,
            review_restart: None,
            stops: None,
            rev: None,
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
            boot(&page)["staticMode"] == true,
            "should set staticMode to true"
        );
    }

    #[test]
    fn render_page_static_mode_false() {
        let theme = github_theme();
        let page = render_with(true, false, false, None, &theme, &["github"]);
        assert!(
            boot(&page)["staticMode"] == false,
            "should set staticMode to false"
        );
    }

    #[test]
    fn render_page_theme_mode_toggle() {
        let theme = github_theme();
        let page = render_with(true, false, false, None, &theme, &["github"]);
        assert!(
            boot(&page)["themeMode"] == "toggle",
            "dual-variant theme should produce toggle mode"
        );
    }

    #[test]
    fn render_page_theme_mode_fixed_dark() {
        let theme = dark_only_theme();
        let page = render_with(true, false, false, None, &theme, &["dracula"]);
        assert!(
            boot(&page)["themeMode"] == "fixed-dark",
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
            boot(&page)["themeMode"] == "fixed-light",
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
            nonce: "",
            extra_boot: None,
            review_token: None,
            review_dir: false,
            review_served: None,
            review_restart: None,
            stops: None,
            rev: None,
        })
    }

    #[test]
    fn render_page_variant_explicit_true() {
        let theme = github_theme();
        let page = render_with_explicit(true, &theme);
        assert!(
            boot(&page)["variantExplicit"] == true,
            "explicit --light/--dark should set variantExplicit to true so the \
             browser does not override it with the OS preference"
        );
    }

    #[test]
    fn render_page_variant_explicit_false() {
        let theme = github_theme();
        let page = render_with_explicit(false, &theme);
        assert!(
            boot(&page)["variantExplicit"] == false,
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
            nonce: "",
            extra_boot: None,
            review_token: None,
            review_dir: false,
            review_served: None,
            review_restart: None,
            stops: None,
            rev: None,
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

    fn base_opts(theme: &ResolvedTheme) -> PageOptions<'_> {
        PageOptions {
            filename: "test.md",
            file_stats: "1 lines (1 loc) · 5 B",
            content_html: "<p>test</p>",
            source_html: Some("test"),
            custom_css: None,
            font_css: None,
            show_header: true,
            reading_mode: false,
            raw_mode: false,
            theme,
            theme_names: &["github"],
            variant_explicit: false,
            static_mode: false,
            keybindings_json: "{}",
            current_path: Some("test.md"),
            nonce: "",
            extra_boot: None,
            review_token: None,
            review_dir: false,
            review_served: None,
            review_restart: None,
            stops: None,
            rev: None,
        }
    }

    /// Raw text of the `birta-boot` data block; panics unless it is a JSON `<script>` in `<head>`.
    fn boot_raw(page: &str) -> &str {
        let id = page.find("id=\"birta-boot\"").expect("no birta-boot block");
        let open = page[..id].rfind('<').unwrap();
        let body = id + page[id..].find('>').unwrap() + 1;
        let tag = &page[open..body];
        assert!(
            tag.starts_with("<script"),
            "birta-boot is not a <script>: {tag}"
        );
        assert!(
            tag.contains("type=\"application/json\""),
            "not a JSON block: {tag}"
        );
        assert!(
            open < page.find("</head>").unwrap(),
            "birta-boot is not in <head>"
        );
        &page[body..body + page[body..].find("</script>").unwrap()]
    }

    fn boot(page: &str) -> serde_json::Value {
        serde_json::from_str(boot_raw(page)).expect("birta-boot is not valid JSON")
    }

    /// `(open tag, body)` of every `<script>` in the page.
    fn scripts(page: &str) -> Vec<(&str, &str)> {
        page.match_indices("<script")
            .map(|(i, _)| {
                let body = i + page[i..].find('>').unwrap() + 1;
                let end = body + page[body..].find("</script>").unwrap();
                (&page[i..body], &page[body..end])
            })
            .collect()
    }

    /// Bodies of the inline scripts the browser executes.
    fn executable_inline(page: &str) -> Vec<&str> {
        scripts(page)
            .into_iter()
            .filter(|(tag, _)| !tag.contains(" src=") && !tag.contains("application/json"))
            .map(|(_, body)| body)
            .collect()
    }

    fn unescape(s: &str) -> String {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&#x27;", "'")
            .replace("&amp;", "&")
    }

    /// Text between `start` and the next `end`.
    fn between<'a>(page: &'a str, start: &str, end: &str) -> &'a str {
        let from = page
            .find(start)
            .unwrap_or_else(|| panic!("{start} not found"))
            + start.len();
        &page[from..from + page[from..].find(end).unwrap()]
    }

    fn assert_script_safe(raw: &str) {
        let bad: Vec<char> = raw
            .chars()
            .filter(|c| matches!(c, '<' | '>' | '&' | '\u{2028}' | '\u{2029}'))
            .collect();
        assert!(bad.is_empty(), "boot JSON has raw {bad:?}: {raw}");
    }

    const HOSTILE: [&str; 6] = [
        "don't.md",
        "x';alert(1);'.md",
        "</script><script>alert(1)",
        "<!--<script>.md",
        "a\u{2028}b\u{2029}c&amp;.md",
        "\"}]);alert(1);//.md",
    ];

    #[test]
    fn boot_block_is_json_in_head() {
        let theme = github_theme();
        let page = render_page(&base_opts(&theme));
        assert!(boot(&page).is_object());
        let inline = executable_inline(&page);
        assert!(
            inline.iter().any(|body| body
                .contains("JSON.parse(document.getElementById('birta-boot').textContent)")),
            "the viewer script must read BOOT from the data block"
        );
    }

    #[test]
    fn boot_block_carries_page_fields() {
        let theme = github_theme();
        let page = render_page(&PageOptions {
            static_mode: true,
            raw_mode: true,
            variant_explicit: true,
            current_path: Some("dir/a.md"),
            rev: Some("ce013625030ba8dba906f756967f9e9ca394464a"),
            keybindings_json: r#"{"toggle_dark":"d"}"#,
            ..base_opts(&theme)
        });
        assert_eq!(
            boot(&page),
            serde_json::json!({
                "staticMode": true,
                "themeMode": "toggle",
                "activeVariant": "dark",
                "variantExplicit": true,
                "variants": ["light", "dark"],
                "keybindings": {"toggle_dark": "d"},
                "currentPath": "dir/a.md",
                "rev": "ce013625030ba8dba906f756967f9e9ca394464a",
                "initialView": "raw",
                "extra": null,
                "review": {"enabled": false},
            })
        );
    }

    #[test]
    fn review_tags_and_boot_follow_the_token() {
        let theme = github_theme();
        let off = render_page(&base_opts(&theme));
        assert!(!off.contains("/_birta/review"));
        let on = render_page(&PageOptions {
            nonce: "N0nce",
            review_token: Some("t0ken"),
            review_dir: false,
            review_served: None,
            review_restart: None,
            ..base_opts(&theme)
        });
        assert_eq!(
            boot(&on)["review"],
            serde_json::json!({"enabled": true, "token": "t0ken", "dir": false})
        );
        assert!(on.contains("<script defer src=\"/_birta/review.js\" nonce=\"N0nce\"></script>"));
        assert!(on.contains("<link rel=\"stylesheet\" href=\"/_birta/review.css\">"));
    }

    #[test]
    fn boot_block_without_current_path_is_falsy() {
        let theme = github_theme();
        let page = render_page(&PageOptions {
            current_path: None,
            ..base_opts(&theme)
        });
        let path = &boot(&page)["currentPath"];
        assert!(path.is_null() || path == "", "got {path}");
    }

    #[test]
    fn boot_block_round_trips_hostile_paths() {
        let theme = github_theme();
        let plain = scripts(&render_page(&base_opts(&theme))).len();
        for path in HOSTILE {
            let page = render_page(&PageOptions {
                current_path: Some(path),
                ..base_opts(&theme)
            });
            assert_eq!(boot(&page)["currentPath"], path);
            assert_script_safe(boot_raw(&page));
            assert_eq!(scripts(&page).len(), plain, "{path:?} added a <script>");
            assert!(
                !page.contains("<script>alert(1)"),
                "{path:?} escaped the block"
            );
        }
    }

    #[test]
    fn hostile_keybindings_are_inert() {
        let theme = github_theme();
        let bindings = serde_json::json!({
            "toggle_dark": "</script><script>alert(1)</script>",
            "toggle_raw": "<!--<script>",
            "focus_theme": "\u{2028}&\u{2029}",
        });
        let json = bindings.to_string();
        let page = render_page(&PageOptions {
            keybindings_json: &json,
            ..base_opts(&theme)
        });
        assert_eq!(boot(&page)["keybindings"], bindings);
        assert_script_safe(boot_raw(&page));
        assert!(!page.contains("<script>alert(1)"));
    }

    #[test]
    fn extra_boot_is_embedded_and_escaped() {
        let theme = github_theme();
        let extra = serde_json::json!({"review": {"note": "</script><b>&'\u{2028}"}});
        let page = render_page(&PageOptions {
            extra_boot: Some(&extra),
            review_token: None,
            review_dir: false,
            review_served: None,
            review_restart: None,
            stops: None,
            rev: None,
            ..base_opts(&theme)
        });
        assert_eq!(boot(&page)["extra"], extra);
        assert_script_safe(boot_raw(&page));
    }

    #[test]
    fn inline_script_is_byte_constant_across_pages() {
        let github = github_theme();
        let light = ResolvedTheme {
            name: "custom-light".to_string(),
            active_variant: Variant::Light,
            ..dark_only_theme()
        };
        let extra = serde_json::json!({"k": 1});
        let a = render_page(&base_opts(&github));
        let b = render_page(&PageOptions {
            filename: "x';y\".md",
            file_stats: "2 lines",
            content_html: "<p>other</p>",
            source_html: Some("other"),
            reading_mode: true,
            raw_mode: true,
            theme: &light,
            theme_names: &["custom-light", "github"],
            variant_explicit: true,
            static_mode: true,
            keybindings_json: r#"{"toggle_dark":"x"}"#,
            current_path: Some("sub/x';y\".md"),
            nonce: "abc123",
            extra_boot: Some(&extra),
            review_token: None,
            review_dir: false,
            review_served: None,
            review_restart: None,
            stops: None,
            rev: None,
            ..base_opts(&github)
        });
        let inline_a = executable_inline(&a);
        assert!(!inline_a.is_empty());
        assert_eq!(inline_a, executable_inline(&b));
    }

    #[test]
    fn every_template_script_carries_the_nonce() {
        let theme = github_theme();
        let page = render_page(&PageOptions {
            nonce: "n0nce",
            ..base_opts(&theme)
        });
        let tags: Vec<&str> = scripts(&page).into_iter().map(|(tag, _)| tag).collect();
        let missing: Vec<&&str> = tags
            .iter()
            .filter(|t| !t.contains("nonce=\"n0nce\""))
            .collect();
        assert!(missing.is_empty(), "scripts without the nonce: {missing:?}");
        assert!(
            tags.len() >= 4,
            "expected CDN, boot and viewer scripts: {tags:?}"
        );
    }

    #[test]
    fn document_nonce_placeholder_stays_literal() {
        let theme = github_theme();
        let doc = "<script nonce=\"{{NONCE}}\">evil()</script>";
        let plain = render_page(&PageOptions {
            nonce: "n0nce",
            ..base_opts(&theme)
        });
        let page = render_page(&PageOptions {
            nonce: "n0nce",
            content_html: doc,
            source_html: Some(doc),
            ..base_opts(&theme)
        });
        assert_eq!(page.matches(doc).count(), 2);
        assert_eq!(
            page.matches("nonce=\"n0nce\"").count(),
            scripts(&plain).len(),
            "only template scripts may carry the nonce"
        );
    }

    #[test]
    fn document_scripts_do_not_get_the_nonce() {
        let theme = github_theme();
        let page = render_page(&PageOptions {
            nonce: "n0nce",
            content_html: "<script>evil()</script>",
            source_html: Some("<script>src()</script>"),
            ..base_opts(&theme)
        });
        assert!(page.contains("<script>evil()</script>"));
        assert!(page.contains("<script>src()</script>"));
    }

    #[test]
    fn hostile_filename_is_escaped_in_title_and_header() {
        let theme = github_theme();
        for name in [
            "<img src=x onerror=1>.md",
            "</title><script>x</script>.md",
            "a\"b'c&d.md",
            "&lt;b&gt;&amp;.md",
        ] {
            let page = render_page(&PageOptions {
                filename: name,
                ..base_opts(&theme)
            });
            let title = between(&page, "<title>", "</title>");
            assert!(!title.contains('<'), "raw < in <title>: {title}");
            assert_eq!(unescape(title), format!("{name} — birta"));
            let header = between(&page, "<span class=\"file-header-name\">", "</span>");
            assert!(
                !header.contains(['<', '"', '\'']),
                "raw markup in header: {header}"
            );
            assert_eq!(unescape(header), name);
            assert_eq!(page.matches("</title>").count(), 1);
        }
    }

    #[test]
    fn file_stats_are_escaped() {
        let theme = github_theme();
        let stats = "<b>1</b> & 'x' \"y\" &lt;x&gt;";
        let page = render_page(&PageOptions {
            file_stats: stats,
            ..base_opts(&theme)
        });
        let shown = between(
            &page,
            "id=\"file-stats\" style=\"display:none\">",
            "</span>",
        );
        assert!(
            !shown.contains(['<', '"', '\'']),
            "raw markup in stats: {shown}"
        );
        assert_eq!(unescape(shown), stats);
    }

    #[test]
    fn hostile_theme_name_is_escaped() {
        let name = "x\"><img src=x onerror=1>' &quot;t";
        let theme = ResolvedTheme {
            name: name.to_string(),
            ..dark_only_theme()
        };
        let page = render_page(&PageOptions {
            theme_names: &[name, "github"],
            ..base_opts(&theme)
        });
        assert!(!page.contains("<img src=x onerror=1>"));
        for (what, raw) in [
            ("THEME_ATTR", between(&page, "data-birta-theme=\"", "\"")),
            (
                "ACTIVE_THEME",
                between(&page, "id=\"theme-dropdown-label\">", "</span>"),
            ),
            (
                "option attr",
                between(&page, "theme-dropdown-item active\" data-theme=\"", "\""),
            ),
            (
                "option text",
                between(
                    &page,
                    "theme-dropdown-item active\" data-theme=\"",
                    "</button>",
                )
                .split_once("\">")
                .map_or("", |(_, text)| text),
            ),
        ] {
            assert!(!raw.contains(['<', '>', '"', '\'']), "{what} raw: {raw}");
            assert_eq!(unescape(raw), name, "{what}");
        }
    }

    #[test]
    fn inactive_theme_names_are_escaped() {
        let theme = github_theme();
        let name = "x\" autofocus onfocus=\"alert(1)";
        let page = render_page(&PageOptions {
            theme_names: &["github", name],
            ..base_opts(&theme)
        });
        assert!(!page.contains("\" autofocus"));
        let item = between(
            &page,
            "<button class=\"theme-dropdown-item\" data-theme=\"",
            "</button>",
        );
        let (attr, text) = item.split_once("\">").expect("dropdown item");
        assert_eq!(unescape(attr), name);
        assert_eq!(unescape(text), name);
    }

    #[test]
    fn boot_block_snapshot() {
        let theme = github_theme();
        let extra = serde_json::json!({"k": "v"});
        let page = render_page(&PageOptions {
            current_path: Some("dir/</script>&'\u{2028}.md"),
            keybindings_json: r#"{"toggle_dark":"<!--"}"#,
            extra_boot: Some(&extra),
            review_token: None,
            review_dir: false,
            review_served: None,
            review_restart: None,
            stops: None,
            rev: None,
            ..base_opts(&theme)
        });
        insta::assert_snapshot!(boot_raw(&page));
    }

    /// Value of `name="…"` in an open tag.
    fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        Some(&tag[start..start + tag[start..].find('"')?])
    }

    #[test]
    fn cdn_scripts_are_pinned_with_sri() {
        let theme = github_theme();
        let page = render_page(&base_opts(&theme));
        let external: Vec<&str> = scripts(&page)
            .into_iter()
            .map(|(tag, _)| tag)
            .filter(|tag| tag.contains(" src="))
            .collect();
        assert!(
            external.len() >= 2,
            "expected KaTeX and mermaid: {external:?}"
        );
        let mut names = Vec::new();
        for tag in external {
            let src = attr(tag, "src").unwrap();
            let package = src
                .strip_prefix("https://cdn.jsdelivr.net/npm/")
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_else(|| panic!("not a jsdelivr npm path: {src}"));
            let (name, version) = package.split_once('@').expect("unversioned package");
            let parts: Vec<&str> = version.split('.').collect();
            assert!(
                parts.len() == 3
                    && parts
                        .iter()
                        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
                "{src} is not pinned to an exact release"
            );
            let integrity = attr(tag, "integrity").unwrap_or_else(|| panic!("no SRI: {tag}"));
            let digest = integrity.strip_prefix("sha384-").unwrap_or("");
            assert!(
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/'),
                "not a sha384 SRI: {tag}"
            );
            assert_eq!(attr(tag, "crossorigin"), Some("anonymous"), "{tag}");
            names.push((name, parts[0]));
        }
        assert!(names.contains(&("katex", "0")), "{names:?}");
        assert!(names.contains(&("mermaid", "11")), "{names:?}");
    }

    #[test]
    fn cdn_stylesheets_are_pinned_with_sri() {
        let theme = github_theme();
        let page = render_page(&base_opts(&theme));
        let links: Vec<&str> = page
            .match_indices("<link")
            .map(|(i, _)| &page[i..i + page[i..].find('>').unwrap() + 1])
            .filter(|tag| attr(tag, "href").is_some_and(|h| h.starts_with("https:")))
            .collect();
        assert!(!links.is_empty(), "expected the KaTeX stylesheet");
        for tag in links {
            let href = attr(tag, "href").unwrap();
            assert!(
                href.starts_with("https://cdn.jsdelivr.net/npm/katex@0.16.21/"),
                "{href}"
            );
            let digest = attr(tag, "integrity")
                .and_then(|i| i.strip_prefix("sha384-"))
                .unwrap_or_else(|| panic!("no sha384 SRI: {tag}"));
            assert!(
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/'),
                "not a sha384 SRI: {tag}"
            );
            assert_eq!(attr(tag, "crossorigin"), Some("anonymous"), "{tag}");
        }
    }

    #[test]
    fn static_hash_covers_the_whole_filled_viewer_script() {
        assert!(
            !viewer_script().contains("{{"),
            "the hash would cover unfilled bytes"
        );
        let theme = github_theme();
        let page = render_page(&PageOptions {
            static_mode: true,
            ..base_opts(&theme)
        });
        assert_eq!(executable_inline(&page), [viewer_script()]);
    }

    #[test]
    fn viewer_script_inlines_the_patcher() {
        let theme = github_theme();
        for static_mode in [false, true] {
            let page = render_page(&PageOptions {
                static_mode,
                ..base_opts(&theme)
            });
            let inline = executable_inline(&page);
            assert_eq!(inline.len(), 1, "static_mode={static_mode}");
            assert!(
                inline[0].contains(PATCH_JS.trim()),
                "patcher not in the viewer script (static_mode={static_mode})"
            );
        }
    }

    #[test]
    fn mermaid_runs_at_strict_security_level() {
        let theme = github_theme();
        let page = render_page(&base_opts(&theme));
        let inline = executable_inline(&page).concat();
        let calls: Vec<&str> = inline
            .match_indices("mermaid.initialize(")
            .map(|(i, _)| {
                let args = &inline[i..];
                &args[..args.find(");").expect("unclosed initialize call")]
            })
            .collect();
        assert!(!calls.is_empty(), "mermaid.initialize is never called");
        for call in calls {
            assert!(call.contains("securityLevel: 'strict'"), "{call}");
        }
        for weaker in ["'loose'", "\"loose\"", "'antiscript'", "\"antiscript\""] {
            assert!(!page.contains(weaker), "{weaker} in the page");
        }
    }
}
