use std::path::{Path, PathBuf};
use std::sync::Arc;

use comrak::nodes::NodeValue;
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{Arena, Options, format_html_with_plugins, options, parse_document};

use crate::highlight;
use crate::theme::SyntaxTheme;

/// Strategy for rewriting relative URLs (images and links).
#[derive(Clone)]
enum RewriteMode {
    /// Rewrite images to `/local/{path}`. No link rewriting.
    Server,
    /// Rewrite images to `file:///{base_dir}/{path}`. No link rewriting.
    Static(PathBuf),
    /// Rewrite images to `/local/{dir}/{path}` and `.md` links to `/view/{dir}/{path}`.
    /// `file_dir` is the directory of the current file relative to `base_dir`.
    Directory { file_dir: PathBuf },
}

impl RewriteMode {
    fn rewrite_image(&self, url: &str) -> String {
        let clean = url.strip_prefix("./").unwrap_or(url);
        match self {
            RewriteMode::Server => format!("/local/{clean}"),
            RewriteMode::Static(base) => format!("file://{}", base.join(clean).display()),
            RewriteMode::Directory { file_dir } => {
                let resolved = file_dir.join(clean);
                let normalized = normalize_path(&resolved);
                format!("/local/{}", normalized.display())
            }
        }
    }

    fn rewrite_link(&self, url: &str) -> String {
        if !should_rewrite_link(url) {
            return url.to_string();
        }

        let clean = url.strip_prefix("./").unwrap_or(url);
        let (path_part, fragment) = split_fragment(clean);

        match self {
            RewriteMode::Server | RewriteMode::Static(_) => url.to_string(),
            RewriteMode::Directory { file_dir } => {
                if path_part.ends_with(".md") || path_part.ends_with(".markdown") {
                    let resolved = file_dir.join(path_part);
                    let normalized = normalize_path(&resolved);
                    format!("/view/{}{fragment}", normalized.display())
                } else {
                    url.to_string()
                }
            }
        }
    }
}

/// Render markdown to HTML, rewriting relative image paths to `/local/` server URLs.
pub fn render(markdown: &str, syntax_theme: Option<&SyntaxTheme>) -> String {
    render_with_mode(markdown, syntax_theme, RewriteMode::Server)
}

/// Render markdown to HTML with directory navigation support.
/// Rewrites relative `.md` links to `/view/` routes and images to `/local/` routes,
/// resolving paths relative to the current file's directory.
pub fn render_dir(
    markdown: &str,
    syntax_theme: Option<&SyntaxTheme>,
    file_relpath: &Path,
) -> String {
    let file_dir = file_relpath.parent().unwrap_or(Path::new("")).to_path_buf();
    render_with_mode(markdown, syntax_theme, RewriteMode::Directory { file_dir })
}

/// Render markdown source as syntax-highlighted HTML for raw mode.
///
/// Returns a side-by-side layout with line numbers and highlighted source:
/// a flex container with a line-number column and a `<pre><code>` block.
pub fn render_source(markdown: &str, syntax_theme: Option<&SyntaxTheme>) -> String {
    let highlighted = crate::highlight::highlight_source(markdown, syntax_theme);
    let line_count = markdown.lines().count().max(1);

    let mut html = String::with_capacity(highlighted.len() + line_count * 60);
    html.push_str("<div class=\"source-lines\">");
    for i in 1..=line_count {
        html.push_str("<span data-line=\"");
        html.push_str(&i.to_string());
        html.push_str("\">");
        html.push_str(&i.to_string());
        html.push_str("</span>");
    }
    html.push_str("</div><pre class=\"source-code\"><code>");
    html.push_str(&highlighted);
    html.push_str("</code></pre>");
    html
}

/// Render markdown to HTML, rewriting relative image paths to absolute `file:///` URLs.
pub fn render_static(
    markdown: &str,
    syntax_theme: Option<&SyntaxTheme>,
    base_dir: &Path,
) -> String {
    render_with_mode(
        markdown,
        syntax_theme,
        RewriteMode::Static(base_dir.to_path_buf()),
    )
}

fn render_with_mode(
    markdown: &str,
    syntax_theme: Option<&SyntaxTheme>,
    mode: RewriteMode,
) -> String {
    let options = options(&mode);
    let adapter = match syntax_theme {
        Some(st) => highlight::adapter_with_theme(st),
        None => highlight::adapter(),
    };
    let plugins = plugins(&adapter);

    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &options);

    for node in root.descendants() {
        let mut data = node.data.borrow_mut();
        match &mut data.value {
            // Convert mermaid code blocks to raw HTML so syntect doesn't process them
            NodeValue::CodeBlock(code_block) if code_block.info == "mermaid" => {
                let html = format!(
                    "<pre class=\"mermaid\">{}</pre>\n",
                    html_escape(&code_block.literal)
                );
                data.value = NodeValue::HtmlBlock(comrak::nodes::NodeHtmlBlock {
                    block_type: 6,
                    literal: html,
                });
            }
            // Rewrite relative image src= in raw HTML (not handled by image_url_rewriter)
            NodeValue::HtmlBlock(block) => {
                block.literal = rewrite_html_img_srcs(&block.literal, &mode);
            }
            NodeValue::HtmlInline(raw) => {
                *raw = rewrite_html_img_srcs(raw, &mode);
            }
            _ => {}
        }
    }

    let mut html = String::new();
    format_html_with_plugins(root, &options, &mut html, &plugins).unwrap();
    html
}

fn plugins(adapter: &SyntectAdapter) -> options::Plugins<'_> {
    let mut plugins = options::Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(adapter);
    plugins
}

fn options(mode: &RewriteMode) -> Options<'static> {
    let mut options = Options::default();

    // GFM extensions
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.description_lists = true;
    options.extension.shortcodes = true;
    options.extension.header_ids = Some(String::new());
    options.extension.alerts = true;
    options.extension.math_dollars = true;
    options.extension.math_code = true;

    // Rewrite relative image paths using the selected strategy
    let img_mode = mode.clone();
    options.extension.image_url_rewriter = Some(Arc::new(move |url: &str| {
        if should_rewrite(url) {
            img_mode.rewrite_image(url)
        } else {
            url.to_string()
        }
    }));

    // Rewrite relative link hrefs for directory navigation
    if matches!(mode, RewriteMode::Directory { .. }) {
        let link_mode = mode.clone();
        options.extension.link_url_rewriter =
            Some(Arc::new(move |url: &str| link_mode.rewrite_link(url)));
    }

    // Parsing
    options.parse.smart = true;

    // Rendering
    options.render.github_pre_lang = true;
    options.render.r#unsafe = true;
    options.render.sourcepos = true;

    options
}

fn should_rewrite(src: &str) -> bool {
    !src.is_empty()
        && !src.starts_with("http://")
        && !src.starts_with("https://")
        && !src.starts_with('/')
        && !src.starts_with("data:")
        && !src.starts_with('#')
}

/// Rewrite `src="..."` attributes in raw HTML `<img>` tags.
fn rewrite_html_img_srcs(html: &str, mode: &RewriteMode) -> String {
    let mut result = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(pos) = rest.find("src=\"") {
        result.push_str(&rest[..pos]);
        let after_src = &rest[pos + 5..]; // skip past src="
        if let Some(end) = after_src.find('"') {
            let url = &after_src[..end];
            if should_rewrite(url) {
                let rewritten = mode.rewrite_image(url);
                result.push_str(&format!("src=\"{rewritten}\""));
            } else {
                result.push_str(&format!("src=\"{url}\""));
            }
            rest = &after_src[end + 1..];
        } else {
            result.push_str(&rest[pos..]);
            rest = "";
            break;
        }
    }
    result.push_str(rest);
    result
}

fn should_rewrite_link(url: &str) -> bool {
    should_rewrite(url) && !url.starts_with("mailto:") && !url.starts_with("tel:")
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

pub(crate) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
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
            None,
            Path::new("README.md"),
        );
        assert!(
            html.contains("href=\"/view/docs/guide.md\""),
            "should rewrite .md link to /view/ route, got: {html}"
        );
    }

    #[test]
    fn render_dir_preserves_fragment() {
        let html = render_dir("[link](other.md#section)", None, Path::new("README.md"));
        assert!(
            html.contains("href=\"/view/other.md#section\""),
            "should preserve fragment, got: {html}"
        );
    }

    #[test]
    fn render_dir_non_md_to_local() {
        let html = render_dir("[dl](file.zip)", None, Path::new("README.md"));
        assert!(
            html.contains("href=\"file.zip\""),
            "non-md links should pass through unchanged, got: {html}"
        );
    }

    #[test]
    fn render_dir_external_unchanged() {
        let html = render_dir("[ext](https://example.com)", None, Path::new("README.md"));
        assert!(
            html.contains("href=\"https://example.com\""),
            "external links should be unchanged, got: {html}"
        );
    }

    #[test]
    fn render_dir_anchor_unchanged() {
        let html = render_dir("[section](#heading)", None, Path::new("README.md"));
        assert!(
            html.contains("href=\"#heading\""),
            "anchor links should be unchanged, got: {html}"
        );
    }

    #[test]
    fn render_dir_parent_traversal() {
        let html = render_dir("[back](../README.md)", None, Path::new("docs/guide.md"));
        assert!(
            html.contains("href=\"/view/README.md\""),
            "../ should resolve relative to file dir, got: {html}"
        );
    }

    #[test]
    fn render_dir_images_resolve_relative_to_file() {
        let html = render_dir("![img](img/photo.png)", None, Path::new("docs/guide.md"));
        assert!(
            html.contains("src=\"/local/docs/img/photo.png\""),
            "images should resolve relative to file dir, got: {html}"
        );
    }

    #[test]
    fn render_without_dir_does_not_rewrite_links() {
        let html = render("[link](other.md)", None);
        assert!(
            html.contains("href=\"other.md\""),
            "plain render should not rewrite links, got: {html}"
        );
    }

    #[test]
    fn render_source_contains_line_numbers() {
        let source = render_source("# Hello\nworld\n", None);
        assert!(
            source.contains("data-line=\"1\""),
            "should contain line number 1, got: {source}"
        );
        assert!(
            source.contains("data-line=\"2\""),
            "should contain line number 2, got: {source}"
        );
    }

    #[test]
    fn render_source_contains_pre_code_block() {
        let source = render_source("# Hello", None);
        assert!(
            source.contains("<pre class=\"source-code\"><code>"),
            "should contain source code pre/code block, got: {source}"
        );
        assert!(
            source.contains("</code></pre>"),
            "should close pre/code block, got: {source}"
        );
    }

    #[test]
    fn render_source_contains_highlighted_content() {
        let source = render_source("# Hello\n\n**bold**\n", None);
        // Syntect should produce span elements for Markdown tokens
        assert!(
            source.contains("<span"),
            "highlighted source should contain span elements, got: {source}"
        );
    }

    #[test]
    fn render_source_empty_input() {
        let source = render_source("", None);
        assert!(
            source.contains("data-line=\"1\""),
            "empty input should still have at least line 1"
        );
    }

    #[test]
    fn render_source_single_line_no_newline() {
        let source = render_source("hello", None);
        assert!(
            source.contains("data-line=\"1\""),
            "single line without newline should have line 1"
        );
        assert!(
            !source.contains("data-line=\"2\""),
            "single line should not have line 2"
        );
    }
}
