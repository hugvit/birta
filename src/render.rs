//! The CLI's rendering modes: `birta_core::render` wired to this server's routes.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use birta_core::render::{RenderOptions, Resolver, front_matter_table};
pub use birta_core::render::{format_file_stats, render_source};

use crate::theme::SyntaxTheme;

/// Relative URLs discovered while rendering in `StaticBundle` mode, used by the
/// bundle crawler to know which pages to follow and which assets to copy.
/// All entries are raw relative URLs with any `#fragment`/`?query` stripped.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct References {
    /// Relative `.md`/`.markdown` links to other pages (to crawl).
    pub md_links: Vec<String>,
    /// Relative image/non-markdown links (to copy into the bundle).
    pub assets: Vec<String>,
}

/// True if `path` ends with a markdown extension, case-insensitively.
fn is_md_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// Swap a trailing `.md`/`.markdown` (case-insensitive) extension for `.html`.
/// `path` must already satisfy `is_md_path`.
fn swap_md_to_html(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let stem_len = if lower.ends_with(".markdown") {
        path.len() - ".markdown".len()
    } else {
        path.len() - ".md".len()
    };
    format!("{}.html", &path[..stem_len])
}

/// Split a URL at the first `#` or `?` into (path, fragment_with_delimiter).
fn split_fragment(url: &str) -> (&str, &str) {
    url.find(['#', '?'])
        .map(|pos| (&url[..pos], &url[pos..]))
        .unwrap_or((url, ""))
}

/// Normalize a relative path by collapsing `.` and `..` segments without filesystem access.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(seg) => parts.push(seg),
            _ => {}
        }
    }
    parts.iter().collect()
}

/// Single file: images are served from `/local/{path}`, links stay as written.
struct ServerResolver;

impl Resolver for ServerResolver {
    fn image(&self, url: &str) -> String {
        let clean = url.strip_prefix("./").unwrap_or(url);
        format!("/local/{clean}")
    }
}

/// Directory navigation: images go to `/local/{dir}/{path}` and `.md` links to
/// `/view/{dir}/{path}`. `file_dir` is the current file's directory relative to
/// the served root.
struct DirectoryResolver {
    file_dir: PathBuf,
}

impl Resolver for DirectoryResolver {
    fn image(&self, url: &str) -> String {
        let clean = url.strip_prefix("./").unwrap_or(url);
        let normalized = normalize_path(&self.file_dir.join(clean));
        format!("/local/{}", normalized.display())
    }

    fn link(&self, url: &str) -> String {
        let clean = url.strip_prefix("./").unwrap_or(url);
        let (path_part, fragment) = split_fragment(clean);
        if path_part.ends_with(".md") || path_part.ends_with(".markdown") {
            let normalized = normalize_path(&self.file_dir.join(path_part));
            format!("/view/{}{fragment}", normalized.display())
        } else {
            url.to_string()
        }
    }
}

/// Static bundle on a mirrored output tree: `.md` links become relative
/// `.html`, images stay relative, and every target is recorded for the crawler.
#[derive(Default)]
struct BundleResolver {
    refs: RefCell<References>,
}

impl Resolver for BundleResolver {
    fn image(&self, url: &str) -> String {
        let clean = url.strip_prefix("./").unwrap_or(url);
        let path_part = split_fragment(clean).0;
        if !path_part.is_empty() {
            self.refs.borrow_mut().assets.push(path_part.to_string());
        }
        // The asset is copied to the mirrored location, so the relative src still resolves
        path_part.to_string()
    }

    fn link(&self, url: &str) -> String {
        let clean = url.strip_prefix("./").unwrap_or(url);
        let (path_part, fragment) = split_fragment(clean);
        if path_part.is_empty() {
            return url.to_string();
        }
        if is_md_path(path_part) {
            self.refs.borrow_mut().md_links.push(path_part.to_string());
            format!("{}{fragment}", swap_md_to_html(path_part))
        } else {
            self.refs.borrow_mut().assets.push(path_part.to_string());
            url.to_string()
        }
    }
}

/// Which optional markdown syntax the CLI renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    /// Show a leading `---` block as a table instead of parsing it as markdown.
    pub front_matter: bool,
    /// Render Obsidian `![[image.png]]` embeds.
    pub embeds: bool,
    /// Render Obsidian `#tags`.
    pub tags: bool,
}

impl Features {
    /// What the CLI renders when no flag or config key says otherwise.
    pub const CLI: Features = Features {
        front_matter: true,
        embeds: true,
        tags: false,
    };
}

/// What the CLI's settings ask of every render.
#[derive(Clone, Copy, Default)]
pub struct Style<'a> {
    /// Highlight code with this theme's inline colours instead of CSS classes.
    pub syntax_theme: Option<&'a SyntaxTheme>,
    pub features: Features,
}

impl<'a> Style<'a> {
    pub fn new(syntax_theme: Option<&'a SyntaxTheme>, features: Features) -> Self {
        Self {
            syntax_theme,
            features,
        }
    }
}

fn render_with(markdown: &str, style: Style<'_>, resolver: &dyn Resolver) -> String {
    let opts = RenderOptions {
        syntax_theme: style.syntax_theme,
        resolver: Some(resolver),
        front_matter: style.features.front_matter,
        embeds: style.features.embeds,
        tags: style.features.tags,
        ..Default::default()
    };
    let rendered = birta_core::render::render(markdown, &opts);
    match rendered.front_matter {
        Some(raw) => front_matter_table(&raw) + &rendered.html,
        None => rendered.html,
    }
}

/// Render markdown to HTML, rewriting relative image paths to `/local/` server URLs.
pub fn render(markdown: &str, style: Style<'_>) -> String {
    render_with(markdown, style, &ServerResolver)
}

/// Render markdown to HTML with directory navigation support.
/// Rewrites relative `.md` links to `/view/` routes and images to `/local/` routes,
/// resolving paths relative to the current file's directory.
pub fn render_dir(markdown: &str, style: Style<'_>, file_relpath: &Path) -> String {
    let file_dir = file_relpath.parent().unwrap_or(Path::new("")).to_path_buf();
    render_with(markdown, style, &DirectoryResolver { file_dir })
}

/// Render markdown to HTML for a multi-file static bundle: `.md` links become
/// relative `.html`, images/assets stay relative. Returns the HTML and the set of
/// relative URLs referenced (pages to crawl + assets to copy).
pub fn render_bundle(markdown: &str, style: Style<'_>) -> (String, References) {
    let resolver = BundleResolver::default();
    let html = render_with(markdown, style, &resolver);
    (html, resolver.refs.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_collapses_parent() {
        assert_eq!(normalize_path(Path::new("a/b/../c")), PathBuf::from("a/c"));
    }

    #[test]
    fn normalize_path_collapses_multiple_parents() {
        assert_eq!(normalize_path(Path::new("a/b/../../c")), PathBuf::from("c"));
    }

    #[test]
    fn normalize_path_strips_current_dir() {
        assert_eq!(normalize_path(Path::new("./a/./b")), PathBuf::from("a/b"));
    }

    #[test]
    fn normalize_path_empty_result() {
        assert_eq!(normalize_path(Path::new("")), PathBuf::from(""));
    }

    #[test]
    fn split_fragment_with_hash() {
        assert_eq!(split_fragment("file.md#section"), ("file.md", "#section"));
    }

    #[test]
    fn split_fragment_with_query() {
        assert_eq!(split_fragment("file.md?q=1"), ("file.md", "?q=1"));
    }

    #[test]
    fn split_fragment_no_fragment() {
        assert_eq!(split_fragment("file.md"), ("file.md", ""));
    }

    #[test]
    fn render_dir_rewrites_md_links() {
        let html = render_dir(
            "# Hello\n\n[guide](docs/guide.md)",
            Style::default(),
            Path::new("README.md"),
        );
        assert!(
            html.contains("href=\"/view/docs/guide.md\""),
            "should rewrite .md link to /view/ route, got: {html}"
        );
    }

    #[test]
    fn render_dir_preserves_fragment() {
        let html = render_dir(
            "[link](other.md#section)",
            Style::default(),
            Path::new("README.md"),
        );
        assert!(
            html.contains("href=\"/view/other.md#section\""),
            "should preserve fragment, got: {html}"
        );
    }

    #[test]
    fn render_dir_non_md_to_local() {
        let html = render_dir("[dl](file.zip)", Style::default(), Path::new("README.md"));
        assert!(
            html.contains("href=\"file.zip\""),
            "non-md links should pass through unchanged, got: {html}"
        );
    }

    #[test]
    fn render_dir_external_unchanged() {
        let html = render_dir(
            "[ext](https://example.com)",
            Style::default(),
            Path::new("README.md"),
        );
        assert!(
            html.contains("href=\"https://example.com\""),
            "external links should be unchanged, got: {html}"
        );
    }

    #[test]
    fn render_dir_anchor_unchanged() {
        let html = render_dir(
            "[section](#heading)",
            Style::default(),
            Path::new("README.md"),
        );
        assert!(
            html.contains("href=\"#heading\""),
            "anchor links should be unchanged, got: {html}"
        );
    }

    #[test]
    fn render_dir_parent_traversal() {
        let html = render_dir(
            "[back](../README.md)",
            Style::default(),
            Path::new("docs/guide.md"),
        );
        assert!(
            html.contains("href=\"/view/README.md\""),
            "../ should resolve relative to file dir, got: {html}"
        );
    }

    #[test]
    fn render_dir_images_resolve_relative_to_file() {
        let html = render_dir(
            "![img](img/photo.png)",
            Style::default(),
            Path::new("docs/guide.md"),
        );
        assert!(
            html.contains("src=\"/local/docs/img/photo.png\""),
            "images should resolve relative to file dir, got: {html}"
        );
    }

    #[test]
    fn render_without_dir_does_not_rewrite_links() {
        let html = render("[link](other.md)", Style::default());
        assert!(
            html.contains("href=\"other.md\""),
            "plain render should not rewrite links, got: {html}"
        );
    }

    #[test]
    fn render_bundle_swaps_md_to_html() {
        let (html, refs) = render_bundle("[guide](docs/guide.md)", Style::default());
        assert!(
            html.contains("href=\"docs/guide.html\""),
            "should swap .md link to relative .html, got: {html}"
        );
        assert_eq!(refs.md_links, vec!["docs/guide.md".to_string()]);
        assert!(refs.assets.is_empty());
    }

    #[test]
    fn render_bundle_preserves_fragment_in_href() {
        let (html, refs) = render_bundle("[s](../README.md#sec)", Style::default());
        assert!(
            html.contains("href=\"../README.html#sec\""),
            "should swap ext and keep fragment, got: {html}"
        );
        // Collected target has the fragment stripped.
        assert_eq!(refs.md_links, vec!["../README.md".to_string()]);
    }

    #[test]
    fn render_bundle_collects_image_assets() {
        let (html, refs) = render_bundle("![img](img/photo.png)", Style::default());
        assert!(
            html.contains("src=\"img/photo.png\""),
            "image src should stay relative, got: {html}"
        );
        assert_eq!(refs.assets, vec!["img/photo.png".to_string()]);
    }

    #[test]
    fn render_bundle_strips_image_query() {
        let (html, refs) = render_bundle("![img](photo.png?v=2)", Style::default());
        assert!(
            html.contains("src=\"photo.png\""),
            "image src query should be stripped, got: {html}"
        );
        assert_eq!(refs.assets, vec!["photo.png".to_string()]);
    }

    #[test]
    fn render_bundle_uppercase_md_extension_is_a_page() {
        let (html, refs) = render_bundle("[g](Guide.MD)", Style::default());
        assert_eq!(
            refs.md_links,
            vec!["Guide.MD".to_string()],
            "uppercase .MD should be treated as a page, not an asset"
        );
        assert!(refs.assets.is_empty());
        assert!(
            html.contains("href=\"Guide.html\""),
            "uppercase .MD href should swap to .html, got: {html}"
        );
    }

    #[test]
    fn render_bundle_uppercase_markdown_extension_swaps() {
        let (html, refs) = render_bundle("[g](Guide.MARKDOWN)", Style::default());
        assert_eq!(refs.md_links, vec!["Guide.MARKDOWN".to_string()]);
        assert!(
            html.contains("href=\"Guide.html\""),
            "uppercase .MARKDOWN href should swap to .html, got: {html}"
        );
    }

    #[test]
    fn render_bundle_link_strips_query_and_fragment_in_target() {
        let (html, refs) = render_bundle("[x](p.md?q=1#s)", Style::default());
        assert_eq!(
            refs.md_links,
            vec!["p.md".to_string()],
            "collected target drops query/fragment"
        );
        assert!(
            html.contains("href=\"p.html?q=1#s\""),
            "href keeps query+fragment after ext swap, got: {html}"
        );
    }

    #[test]
    fn render_bundle_non_md_link_is_asset() {
        let (html, refs) = render_bundle("[dl](file.zip)", Style::default());
        assert!(
            html.contains("href=\"file.zip\""),
            "non-md link href should be unchanged, got: {html}"
        );
        assert_eq!(refs.assets, vec!["file.zip".to_string()]);
        assert!(refs.md_links.is_empty());
    }

    #[test]
    fn render_bundle_collects_raw_html_img() {
        let (_html, refs) = render_bundle("<img src=\"logo.png\">", Style::default());
        assert_eq!(refs.assets, vec!["logo.png".to_string()]);
    }

    #[test]
    fn render_bundle_ignores_external_and_anchor() {
        let (_html, refs) = render_bundle(
            "[a](https://x.com) [b](#h) [c](mailto:a@b.c) ![d](data:image/png;base64,AAAA)",
            Style::default(),
        );
        assert!(refs.md_links.is_empty(), "no md links expected");
        assert!(
            refs.assets.is_empty(),
            "external/anchor/mailto/data must not be collected, got: {:?}",
            refs.assets
        );
    }

    const EMBEDS: Style<'static> = Style {
        syntax_theme: None,
        features: Features {
            front_matter: false,
            embeds: true,
            tags: false,
        },
    };

    #[test]
    fn embeds_and_tags_follow_the_features() {
        let md = "![[a.png]] #idea";
        let off = render(md, Style::default());
        assert!(off.contains("![[a.png]] #idea"), "got: {off}");
        let on = render(
            md,
            Style::new(
                None,
                Features {
                    embeds: true,
                    tags: true,
                    ..Default::default()
                },
            ),
        );
        assert!(on.contains("src=\"/local/a.png\""), "got: {on}");
        assert!(on.contains("class=\"tag\""), "got: {on}");
    }

    #[test]
    fn render_dir_image_embed_resolves_relative_to_file() {
        let html = render_dir(
            "![[img/photo 1.png|200]]",
            EMBEDS,
            Path::new("docs/guide.md"),
        );
        assert!(
            html.contains(
                "src=\"/local/docs/img/photo%201.png\" alt=\"img/photo 1.png\" width=\"200\""
            ),
            "got: {html}"
        );
    }

    #[test]
    fn render_bundle_collects_image_embeds() {
        let (html, refs) = render_bundle("![[img/photo.png]]", EMBEDS);
        assert!(html.contains("src=\"img/photo.png\""), "got: {html}");
        assert_eq!(refs.assets, vec!["img/photo.png".to_string()]);
    }

    #[test]
    fn front_matter_is_markdown_when_off() {
        let html = render("---\ntitle: x\n---\nbody", Style::default());
        assert!(html.contains("<hr"), "got: {html}");
        assert!(!html.contains("front-matter"), "got: {html}");
    }

    #[test]
    fn front_matter_becomes_a_table_above_the_body_when_on() {
        let style = Style::new(
            None,
            Features {
                front_matter: true,
                ..Default::default()
            },
        );
        let html = render("---\ntitle: x\n---\nbody", style);
        assert!(
            html.starts_with("<table class=\"front-matter\"><thead><tr><th>title</th>"),
            "got: {html}"
        );
        assert!(!html.contains("<hr"), "got: {html}");
        assert!(html.contains(">body</p>"), "got: {html}");
    }
}
