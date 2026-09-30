use std::path::{Path, PathBuf};
use std::sync::Arc;

use comrak::nodes::NodeValue;
use comrak::{Arena, Options, format_html_with_plugins, options, parse_document};

use crate::anchor::{self, BlockIndex};
use crate::highlight::{self, Highlighter};
use crate::theme::SyntaxTheme;

/// Strategy for rewriting relative URLs (images and links).
#[derive(Clone)]
enum RewriteMode {
    /// Rewrite images to `/local/{path}`. No link rewriting.
    Server,
    /// Rewrite images to `/local/{dir}/{path}` and `.md` links to `/view/{dir}/{path}`.
    /// `file_dir` is the directory of the current file relative to `base_dir`.
    Directory { file_dir: PathBuf },
    /// Multi-file static bundle: rewrite `.md`/`.markdown` links to relative `.html`
    /// (position-invariant on a mirrored output tree) and leave images/assets as
    /// relative URLs. Referenced paths are collected into `References` by the caller.
    StaticBundle,
}

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

impl RewriteMode {
    fn rewrite_image(&self, url: &str) -> String {
        let clean = url.strip_prefix("./").unwrap_or(url);
        match self {
            RewriteMode::Server => format!("/local/{clean}"),
            RewriteMode::Directory { file_dir } => match normalize_path(&file_dir.join(clean)) {
                Some(normalized) => format!("/local/{}", normalized.display()),
                None => url.to_string(),
            },
            // Keep the relative URL (query stripped); the asset is copied to the
            // mirrored location in the bundle, so the relative src still resolves.
            RewriteMode::StaticBundle => split_fragment(clean).0.to_string(),
        }
    }

    fn rewrite_link(&self, url: &str) -> String {
        if !should_rewrite_link(url) {
            return url.to_string();
        }

        let clean = url.strip_prefix("./").unwrap_or(url);
        let (path_part, fragment) = split_fragment(clean);

        match self {
            RewriteMode::Server => url.to_string(),
            RewriteMode::Directory { file_dir } => {
                let normalized = (path_part.ends_with(".md") || path_part.ends_with(".markdown"))
                    .then(|| normalize_path(&file_dir.join(path_part)))
                    .flatten();
                match normalized {
                    Some(normalized) => format!("/view/{}{fragment}", normalized.display()),
                    None => url.to_string(),
                }
            }
            // Mirrored tree: keep the relative link, swap `.md`→`.html`. Non-md
            // links pass through (the file is copied as an asset).
            RewriteMode::StaticBundle => {
                if is_md_path(path_part) {
                    format!("{}{fragment}", swap_md_to_html(path_part))
                } else {
                    url.to_string()
                }
            }
        }
    }
}

/// Render markdown to HTML, rewriting relative image paths to `/local/` server URLs.
pub fn render(markdown: &str, syntax_theme: Option<&SyntaxTheme>) -> String {
    render_indexed(markdown, syntax_theme).0
}

/// [`render`], plus the block index of the same parse.
pub fn render_indexed(markdown: &str, syntax_theme: Option<&SyntaxTheme>) -> (String, BlockIndex) {
    let (html, _, index) = render_with_mode(markdown, syntax_theme, RewriteMode::Server);
    (html, index)
}

/// Render markdown to HTML with directory navigation support.
/// Rewrites relative `.md` links to `/view/` routes and images to `/local/` routes,
/// resolving paths relative to the current file's directory.
pub fn render_dir(
    markdown: &str,
    syntax_theme: Option<&SyntaxTheme>,
    file_relpath: &Path,
) -> String {
    render_dir_indexed(markdown, syntax_theme, file_relpath).0
}

/// [`render_dir`], plus the block index of the same parse.
pub fn render_dir_indexed(
    markdown: &str,
    syntax_theme: Option<&SyntaxTheme>,
    file_relpath: &Path,
) -> (String, BlockIndex) {
    let file_dir = file_relpath.parent().unwrap_or(Path::new("")).to_path_buf();
    let mode = RewriteMode::Directory { file_dir };
    let (html, _, index) = render_with_mode(markdown, syntax_theme, mode);
    (html, index)
}

/// Render markdown to HTML for a multi-file static bundle: `.md` links become
/// relative `.html`, images/assets stay relative. Returns the HTML and the set of
/// relative URLs referenced (pages to crawl + assets to copy).
pub fn render_bundle(markdown: &str, syntax_theme: Option<&SyntaxTheme>) -> (String, References) {
    let (html, refs, _) = render_with_mode(markdown, syntax_theme, RewriteMode::StaticBundle);
    (html, refs)
}

/// Format file stats for the raw-mode file header, e.g. "215 lines (154 loc) · 5.25 KB".
///
/// - `lines`: total line count via `str::lines` (matches `render_source` and GitHub)
/// - `loc`: non-empty lines (lines of "code")
/// - size: UTF-8 byte length of the markdown, formatted as B/KB/MB
pub fn format_file_stats(markdown: &str) -> String {
    let lines = markdown.lines().count();
    let loc = markdown.lines().filter(|l| !l.trim().is_empty()).count();
    let size = format_size(markdown.len());
    format!("{lines} lines ({loc} loc) · {size}")
}

fn format_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
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

fn render_with_mode(
    markdown: &str,
    syntax_theme: Option<&SyntaxTheme>,
    mode: RewriteMode,
) -> (String, References, BlockIndex) {
    let options = options(&mode);
    let adapter = match syntax_theme {
        Some(st) => highlight::adapter_with_theme(st),
        None => highlight::adapter(),
    };
    let plugins = plugins(&adapter);

    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &options);
    // Before the mermaid rewrite below, which turns diagrams into raw HTML
    let index = anchor::index(root, markdown);

    // Collect referenced relative URLs only for bundle export.
    let collect = matches!(mode, RewriteMode::StaticBundle);
    let mut refs = References::default();

    for node in root.descendants() {
        let mut data = node.data.borrow_mut();
        let sourcepos = data.sourcepos;
        match &mut data.value {
            // Convert mermaid code blocks to raw HTML so syntect doesn't process them
            NodeValue::CodeBlock(code_block) if code_block.info == "mermaid" => {
                let html = format!(
                    "<pre class=\"mermaid\" data-sourcepos=\"{}\">{}</pre>\n",
                    sourcepos,
                    html_escape(&code_block.literal)
                );
                data.value = NodeValue::HtmlBlock(comrak::nodes::NodeHtmlBlock {
                    block_type: 6,
                    literal: html,
                });
            }
            // Rewrite relative image src= in raw HTML (not handled by image_url_rewriter)
            NodeValue::HtmlBlock(block) => {
                let html = rewrite_html_img_srcs(&block.literal, &mode, collect, &mut refs);
                block.literal = neutralize_reserved(&html);
            }
            NodeValue::HtmlInline(raw) => {
                let html = rewrite_html_img_srcs(raw, &mode, collect, &mut refs);
                *raw = neutralize_reserved(&html);
            }
            // Collect markdown link/image targets for the bundle crawler.
            NodeValue::Link(link) if collect => collect_link(&link.url, &mut refs),
            NodeValue::Image(link) if collect => collect_asset(&link.url, &mut refs),
            _ => {}
        }
    }

    let mut html = String::new();
    format_html_with_plugins(root, &options, &mut html, &plugins).unwrap();
    (html, refs, index)
}

/// Record a markdown link target: `.md`/`.markdown` → page to crawl, else asset.
fn collect_link(url: &str, refs: &mut References) {
    if !should_rewrite_link(url) {
        return;
    }
    let clean = url.strip_prefix("./").unwrap_or(url);
    let path_part = split_fragment(clean).0;
    if path_part.is_empty() {
        return;
    }
    if is_md_path(path_part) {
        refs.md_links.push(path_part.to_string());
    } else {
        refs.assets.push(path_part.to_string());
    }
}

/// Record a relative asset reference (image), with query/fragment stripped.
fn collect_asset(url: &str, refs: &mut References) {
    if !should_rewrite(url) {
        return;
    }
    let clean = url.strip_prefix("./").unwrap_or(url);
    let path_part = split_fragment(clean).0;
    if !path_part.is_empty() {
        refs.assets.push(path_part.to_string());
    }
}

fn plugins(adapter: &Highlighter) -> options::Plugins<'_> {
    let mut plugins = options::Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(adapter);
    plugins
}

fn options(mode: &RewriteMode) -> Options<'static> {
    let mut options = anchor::parse_options();

    // Rewrite relative image paths using the selected strategy
    let img_mode = mode.clone();
    options.extension.image_url_rewriter = Some(Arc::new(move |url: &str| {
        if should_rewrite(url) {
            img_mode.rewrite_image(url)
        } else {
            url.to_string()
        }
    }));

    // Rewrite relative link hrefs for directory navigation and bundle export
    if matches!(
        mode,
        RewriteMode::Directory { .. } | RewriteMode::StaticBundle
    ) {
        let link_mode = mode.clone();
        options.extension.link_url_rewriter =
            Some(Arc::new(move |url: &str| link_mode.rewrite_link(url)));
    }

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

/// Rewrite `src="..."` attributes in raw HTML `<img>` tags. When `collect` is set
/// (bundle mode), rewritten relative srcs are also recorded as assets to copy.
fn rewrite_html_img_srcs(
    html: &str,
    mode: &RewriteMode,
    collect: bool,
    refs: &mut References,
) -> String {
    let mut result = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(pos) = rest.find("src=\"") {
        result.push_str(&rest[..pos]);
        let after_src = &rest[pos + 5..]; // skip past src="
        if let Some(end) = after_src.find('"') {
            let url = &after_src[..end];
            if should_rewrite(url) {
                if collect {
                    collect_asset(url, refs);
                }
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

/// Keeps raw HTML from forging the viewer's own markup: `birta-*` tags render as text
/// and `data-sourcepos` / `data-br-*` attributes become `data-x-*`. `http-equiv` becomes
/// `data-x-http-equiv`, since a meta refresh or policy is beyond the page CSP's reach.
fn neutralize_reserved(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len());
    let mut rest = literal;
    // A tag left open would absorb whatever the renderer emits next, so its `<` becomes text.
    while let Some(open) = neutralize_tags(rest, &mut out) {
        out.push_str("&lt;");
        rest = &rest[open + 1..];
    }
    out
}

/// Appends `s` to `out`, rewritten. If `s` ends inside a tag, `out` is cut back to
/// that tag and its offset in `s` is returned.
fn neutralize_tags(s: &str, out: &mut String) -> Option<usize> {
    let b = s.as_bytes();
    let mut copied = 0;
    let mut i = 0;
    while let Some(lt) = s[i..].find('<').map(|p| p + i) {
        let after = &s[lt + 1..];
        if is_reserved_tag(after) {
            out.push_str(&s[copied..lt]);
            out.push_str("&lt;");
            copied = lt + 1;
            i = lt + 1;
            continue;
        }
        let end_tag = after.starts_with('/');
        let name_start = lt + 1 + usize::from(end_tag);
        if !b.get(name_start).is_some_and(u8::is_ascii_alphabetic) {
            i = match skip_markup(s, lt) {
                Some(end) => end,
                // Unterminated comments swallow what follows, which can't forge an attribute
                None => break,
            };
            continue;
        }
        out.push_str(&s[copied..lt]);
        copied = lt;
        let mark = out.len();
        match scan_tag(s, name_start, out, &mut copied) {
            Some(end) => i = end,
            None => {
                out.truncate(mark);
                return Some(lt);
            }
        }
    }
    out.push_str(&s[copied..]);
    None
}

/// End of a comment, `<!…>`, `<?…>` or `</…>` starting at `lt`, or `lt + 1` for a literal `<`.
fn skip_markup(s: &str, lt: usize) -> Option<usize> {
    let after = &s[lt + 1..];
    if let Some(body) = after.strip_prefix("!--") {
        let start = lt + 4;
        if body.starts_with('>') {
            return Some(start + 1);
        }
        if body.starts_with("->") {
            return Some(start + 2);
        }
        let close = ["-->", "--!>"]
            .iter()
            .filter_map(|end| body.find(end).map(|p| p + end.len()))
            .min()?;
        return Some(start + close);
    }
    if after.starts_with(['!', '?', '/']) {
        return after.find('>').map(|p| lt + 1 + p + 1);
    }
    Some(lt + 1)
}

/// Walks a tag from its name as a browser tokenizer would, renaming reserved attributes.
/// Returns the offset past `>`, or `None` if the tag is still open at the end of `s`.
fn scan_tag(s: &str, name_start: usize, out: &mut String, copied: &mut usize) -> Option<usize> {
    let b = s.as_bytes();
    let is_space = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c');
    let mut i = name_start;
    while i < b.len() && !is_space(b[i]) && !matches!(b[i], b'/' | b'>') {
        i += 1;
    }
    loop {
        while i < b.len() && (is_space(b[i]) || b[i] == b'/') {
            i += 1;
        }
        match b.get(i) {
            None => return None,
            Some(b'>') => return Some(i + 1),
            Some(_) => {}
        }
        let attr_start = i;
        i += 1;
        while i < b.len() && !is_space(b[i]) && !matches!(b[i], b'/' | b'>' | b'=') {
            i += 1;
        }
        if let Some(kept) = reserved_attr_kept(&s[attr_start..i]) {
            out.push_str(&s[*copied..attr_start]);
            out.push_str("data-x-");
            *copied = attr_start + kept;
        }
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        if b.get(i) != Some(&b'=') {
            continue;
        }
        i += 1;
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        match b.get(i) {
            None => return None,
            Some(&quote @ (b'"' | b'\'')) => {
                i += 1 + s[i + 1..].find(char::from(quote))? + 1;
            }
            Some(_) => {
                while i < b.len() && !is_space(b[i]) && b[i] != b'>' {
                    i += 1;
                }
            }
        }
    }
}

/// `<birta-…` or `</birta-…`, tolerating whitespace around the `/`.
fn is_reserved_tag(after_lt: &str) -> bool {
    let rest = after_lt.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
    rest.get(..6)
        .is_some_and(|name| name.eq_ignore_ascii_case("birta-"))
}

/// For a reserved attribute name, the offset from which the original name follows `data-x-`.
fn reserved_attr_kept(name: &str) -> Option<usize> {
    let data = name.eq_ignore_ascii_case("data-sourcepos")
        || name
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data-br-"));
    if data {
        Some("data-".len())
    } else {
        name.eq_ignore_ascii_case("http-equiv").then_some(0)
    }
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
/// `None` when a `..` climbs above the root, which no served path can name.
pub(crate) fn normalize_path(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(seg) => parts.push(seg),
            _ => {}
        }
    }
    Some(parts.iter().collect())
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
    fn format_file_stats_empty() {
        assert_eq!(format_file_stats(""), "0 lines (0 loc) · 0 B");
    }

    #[test]
    fn format_file_stats_small_file() {
        assert_eq!(format_file_stats("hello"), "1 lines (1 loc) · 5 B");
    }

    #[test]
    fn format_file_stats_trailing_newline_matches_render_source() {
        // Rust's lines() strips one trailing newline: "foo\n" -> 1 line.
        // render_source uses the same logic, so the stats line count matches
        // what the user sees in the gutter.
        assert_eq!(format_file_stats("foo\n"), "1 lines (1 loc) · 4 B");
    }

    #[test]
    fn format_file_stats_counts_blank_lines_in_total_but_not_loc() {
        let md = "a\n\nb\n\n\nc\n";
        // 6 newline-terminated lines, 3 non-empty.
        assert_eq!(format_file_stats(md), "6 lines (3 loc) · 9 B");
    }

    #[test]
    fn format_file_stats_kb_boundary() {
        let md = "a".repeat(2048);
        assert_eq!(format_file_stats(&md), "1 lines (1 loc) · 2.00 KB");
    }

    #[test]
    fn format_file_stats_mb_threshold() {
        let md = "a".repeat(2 * 1024 * 1024);
        let s = format_file_stats(&md);
        assert!(s.ends_with("2.00 MB"), "got: {s}");
    }

    #[test]
    fn format_file_stats_whitespace_only_line_not_counted_as_loc() {
        assert_eq!(format_file_stats("a\n   \nb"), "3 lines (2 loc) · 7 B");
    }

    #[test]
    fn normalize_path_collapses_parent() {
        assert_eq!(
            normalize_path(Path::new("a/b/../c")),
            Some(PathBuf::from("a/c"))
        );
    }

    #[test]
    fn normalize_path_collapses_multiple_parents() {
        assert_eq!(
            normalize_path(Path::new("a/b/../../c")),
            Some(PathBuf::from("c"))
        );
    }

    #[test]
    fn normalize_path_strips_current_dir() {
        assert_eq!(
            normalize_path(Path::new("./a/./b")),
            Some(PathBuf::from("a/b"))
        );
    }

    #[test]
    fn normalize_path_refuses_to_climb_above_the_root() {
        for path in [
            "..",
            "../x.md",
            "a/../../x.md",
            "./../x.md",
            "a/b/../../../x.md",
        ] {
            assert_eq!(normalize_path(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn normalize_path_empty_result() {
        assert_eq!(normalize_path(Path::new("")), Some(PathBuf::from("")));
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
    fn render_dir_leaves_links_escaping_the_root_unrewritten() {
        let mut bad = Vec::new();
        for (url, file) in [
            ("../x.md", "README.md"),
            ("a/../../x.md", "README.md"),
            ("../x.md#sec", "README.md"),
            ("./../x.md", "README.md"),
            ("../../x.md", "docs/guide.md"),
            ("../../../x.markdown", "docs/sub/guide.md"),
        ] {
            let html = render_dir(&format!("[x]({url})"), None, Path::new(file));
            if !html.contains(&format!("href=\"{url}\"")) || html.contains("/view/") {
                bad.push(format!("{url} from {file}: {html}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn render_dir_rewrites_parent_links_that_stay_inside_the_root() {
        for (url, file, want) in [
            ("sub/../../README.md", "docs/guide.md", "/view/README.md"),
            ("../docs/../README.md", "docs/guide.md", "/view/README.md"),
            ("../../a.md#top", "docs/sub/guide.md", "/view/a.md#top"),
        ] {
            let html = render_dir(&format!("[x]({url})"), None, Path::new(file));
            assert!(
                html.contains(&format!("href=\"{want}\"")),
                "{url} from {file}: {html}"
            );
        }
    }

    #[test]
    fn render_dir_leaves_images_escaping_the_root_unrewritten() {
        let mut bad = Vec::new();
        for (md, file, src) in [
            ("![p](../p.png)", "README.md", "../p.png"),
            ("![p](../../p.png)", "docs/guide.md", "../../p.png"),
            ("<img src=\"../p.png\">", "README.md", "../p.png"),
        ] {
            let html = render_dir(md, None, Path::new(file));
            if !html.contains(&format!("src=\"{src}\"")) || html.contains("/local/") {
                bad.push(format!("{md} from {file}: {html}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
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

    // --- bundle mode ---------------------------------------------------------

    #[test]
    fn render_bundle_swaps_md_to_html() {
        let (html, refs) = render_bundle("[guide](docs/guide.md)", None);
        assert!(
            html.contains("href=\"docs/guide.html\""),
            "should swap .md link to relative .html, got: {html}"
        );
        assert_eq!(refs.md_links, vec!["docs/guide.md".to_string()]);
        assert!(refs.assets.is_empty());
    }

    #[test]
    fn render_bundle_preserves_fragment_in_href() {
        let (html, refs) = render_bundle("[s](../README.md#sec)", None);
        assert!(
            html.contains("href=\"../README.html#sec\""),
            "should swap ext and keep fragment, got: {html}"
        );
        // Collected target has the fragment stripped.
        assert_eq!(refs.md_links, vec!["../README.md".to_string()]);
    }

    #[test]
    fn render_bundle_collects_image_assets() {
        let (html, refs) = render_bundle("![img](img/photo.png)", None);
        assert!(
            html.contains("src=\"img/photo.png\""),
            "image src should stay relative, got: {html}"
        );
        assert_eq!(refs.assets, vec!["img/photo.png".to_string()]);
    }

    #[test]
    fn render_bundle_strips_image_query() {
        let (html, refs) = render_bundle("![img](photo.png?v=2)", None);
        assert!(
            html.contains("src=\"photo.png\""),
            "image src query should be stripped, got: {html}"
        );
        assert_eq!(refs.assets, vec!["photo.png".to_string()]);
    }

    #[test]
    fn render_bundle_uppercase_md_extension_is_a_page() {
        let (html, refs) = render_bundle("[g](Guide.MD)", None);
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
        let (html, refs) = render_bundle("[g](Guide.MARKDOWN)", None);
        assert_eq!(refs.md_links, vec!["Guide.MARKDOWN".to_string()]);
        assert!(
            html.contains("href=\"Guide.html\""),
            "uppercase .MARKDOWN href should swap to .html, got: {html}"
        );
    }

    #[test]
    fn render_bundle_link_strips_query_and_fragment_in_target() {
        let (html, refs) = render_bundle("[x](p.md?q=1#s)", None);
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
        let (html, refs) = render_bundle("[dl](file.zip)", None);
        assert!(
            html.contains("href=\"file.zip\""),
            "non-md link href should be unchanged, got: {html}"
        );
        assert_eq!(refs.assets, vec!["file.zip".to_string()]);
        assert!(refs.md_links.is_empty());
    }

    #[test]
    fn render_bundle_collects_raw_html_img() {
        let (_html, refs) = render_bundle("<img src=\"logo.png\">", None);
        assert_eq!(refs.assets, vec!["logo.png".to_string()]);
    }

    #[test]
    fn render_bundle_ignores_external_and_anchor() {
        let (_html, refs) = render_bundle(
            "[a](https://x.com) [b](#h) [c](mailto:a@b.c) ![d](data:image/png;base64,AAAA)",
            None,
        );
        assert!(refs.md_links.is_empty(), "no md links expected");
        assert!(
            refs.assets.is_empty(),
            "external/anchor/mailto/data must not be collected, got: {:?}",
            refs.assets
        );
    }

    /// Names of every attribute whose value is `value`, lowercased, as a browser would read them.
    fn attr_names_with_value(html: &str, value: &str) -> Vec<String> {
        html.match_indices(value)
            .map(|(i, _)| {
                let before = html[..i].trim_end_matches(['"', '\'']);
                let before = before.trim_end().strip_suffix('=').unwrap_or(before);
                let before = before.trim_end();
                let start = before
                    .rfind(|c: char| c.is_whitespace() || "/<>\"'".contains(c))
                    .map_or(0, |j| j + 1);
                before[start..].to_ascii_lowercase()
            })
            .collect()
    }

    /// True if `html` opens or closes a `birta-*` element, tolerating `< /` spacing.
    fn has_reserved_tag(html: &str) -> bool {
        html.match_indices('<').any(|(i, _)| {
            let rest = html[i + 1..].trim_start();
            let rest = rest.strip_prefix('/').unwrap_or(rest).trim_start();
            rest.get(..6)
                .is_some_and(|name| name.eq_ignore_ascii_case("birta-"))
        })
    }

    /// Output of every render path that emits raw HTML.
    fn all_renders(md: &str) -> [(&'static str, String); 3] {
        [
            ("render", render(md, None)),
            ("render_dir", render_dir(md, None, Path::new("a.md"))),
            ("render_bundle", render_bundle(md, None).0),
        ]
    }

    #[test]
    fn raw_html_reserved_attrs_are_renamed() {
        let cases = [
            (
                "<div data-sourcepos=\"40:1-90:1\">x</div>",
                "40:1-90:1",
                "data-x-sourcepos",
            ),
            (
                "<div DATA-SOURCEPOS=\"1:1-1:1\">x</div>",
                "1:1-1:1",
                "data-x-sourcepos",
            ),
            (
                "<div data-sourcepos = '2:2-2:2'>x</div>",
                "2:2-2:2",
                "data-x-sourcepos",
            ),
            (
                "<div data-sourcepos=3:3-3:3>x</div>",
                "3:3-3:3",
                "data-x-sourcepos",
            ),
            (
                "<div Data-Br-x=\"4:4-4:4\">x</div>",
                "4:4-4:4",
                "data-x-br-x",
            ),
            (
                "<div data-br-id=\"5:5-5:5\">x</div>",
                "5:5-5:5",
                "data-x-br-id",
            ),
            (
                "a <span data-sourcepos=\"6:6-6:6\">b</span>",
                "6:6-6:6",
                "data-x-sourcepos",
            ),
            (
                "a <span DATA-BR-id=\"7:7-7:7\">b</span>",
                "7:7-7:7",
                "data-x-br-id",
            ),
            (
                "<section>\n<div/data-sourcepos=\"8:8-8:8\">x</div>\n</section>",
                "8:8-8:8",
                "data-x-sourcepos",
            ),
            (
                "<div title=\"a>b\" data-sourcepos=\"10:10-10:10\">q</div>",
                "10:10-10:10",
                "data-x-sourcepos",
            ),
            (
                "<div\n  data-sourcepos=\"11:11-11:11\">x</div>",
                "11:11-11:11",
                "data-x-sourcepos",
            ),
            (
                "<div\tdata-sourcepos=\"12:12-12:12\">x</div>",
                "12:12-12:12",
                "data-x-sourcepos",
            ),
            (
                "<div class=x\tdata-br-y=\"14:14-14:14\">x</div>",
                "14:14-14:14",
                "data-x-br-y",
            ),
        ];
        let mut bad = Vec::new();
        for (md, value, want) in cases {
            for (name, html) in all_renders(md) {
                let names = attr_names_with_value(&html, value);
                if names.is_empty() || names.iter().any(|n| n != want) {
                    bad.push(format!(
                        "{name}({md:?}): got {names:?}, want {want}\n  {html}"
                    ));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "reserved attrs not renamed:\n{}",
            bad.join("\n")
        );
    }

    #[test]
    fn raw_html_http_equiv_is_renamed() {
        let cases = [
            "<meta http-equiv=\"refresh\" content=\"5\">",
            "<META HTTP-EQUIV=refresh content=5>",
            "<meta http-equiv = 'refresh' content='5'>",
            "<meta\tHttp-Equiv=\"refresh\">",
            "<div>\n<meta/http-equiv=\"refresh\">\n</div>",
            "a <meta http-equiv=\"refresh\" content=\"5\"> b",
            "<meta content=\"5\" http-equiv=\"refresh\">",
        ];
        let mut bad = Vec::new();
        for md in cases {
            for (name, html) in all_renders(md) {
                let names = attr_names_with_value(&html, "refresh");
                if names.is_empty() || names.iter().any(|n| n != "data-x-http-equiv") {
                    bad.push(format!("{name}({md:?}): got {names:?}\n  {html}"));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "http-equiv not renamed:\n{}",
            bad.join("\n")
        );

        for (name, html) in all_renders("<meta http-equiv>") {
            assert!(html.contains("<meta data-x-http-equiv>"), "{name}: {html}");
        }
        // Only the exact attribute name is reserved
        for (name, html) in all_renders("<meta xhttp-equiv=\"refresh\" data-http-equiv=\"r\">") {
            assert!(
                html.contains("<meta xhttp-equiv=\"refresh\" data-http-equiv=\"r\">"),
                "{name}: {html}"
            );
        }
    }

    #[test]
    fn raw_html_bare_reserved_attrs_are_renamed() {
        for (name, html) in all_renders("<div data-sourcepos data-BR-x>x</div>") {
            assert!(
                html.to_ascii_lowercase()
                    .contains("<div data-x-sourcepos data-x-br-x>"),
                "{name}: {html}"
            );
        }
    }

    #[test]
    fn raw_html_open_tag_is_escaped() {
        // (markdown, text that must appear, attribute value that must not be reserved)
        let cases = [
            // The attribute would land on the first block's `<div`
            (
                "<div title=\"\n\n<p>\" data-sourcepos=\"9:9-9:9\">x</p>",
                "&lt;div title=\"",
                None,
            ),
            // comrak decodes `&#39;`, closing the raw block's open attribute from paragraph text
            (
                "<div title='\n\n&#39; data-sourcepos=50:1-60:1 data-br-id=zz x=&#39;\n\n&#39;",
                "&lt;div title='",
                None,
            ),
            ("text\n\n<div data-a=\"1\"", "&lt;div data-a=\"1\"", None),
            // Once the `<div` is text, the tag inside its old attribute value is live
            (
                "<div title=\"<span data-sourcepos='13:13-13:13'>\"",
                "&lt;div title=\"<span data-x-sourcepos='13:13-13:13'>",
                Some("13:13-13:13"),
            ),
        ];
        let mut bad = Vec::new();
        for (md, want, value) in cases {
            for (name, html) in all_renders(md) {
                let renamed = value.is_none_or(|v| {
                    attr_names_with_value(&html, v)
                        .iter()
                        .all(|n| n == "data-x-sourcepos")
                });
                if !html.contains(want) || !renamed {
                    bad.push(format!("{name}({md:?}):\n  {html}"));
                }
            }
        }
        assert!(bad.is_empty(), "open tags not escaped:\n{}", bad.join("\n"));
    }

    #[test]
    fn raw_html_reserved_tags_render_as_text() {
        let cases = [
            "<birta-flow>hi</birta-flow>",
            "<BIRTA-flow>hi</BIRTA-flow>",
            "<div>\n< /birta-flow>\n<birta-flow data-x=\"1\">\n</div>",
            "<section>\n<Birta-Note>a</Birta-Note>\n</section>",
            "x <birta-note>y</birta-note> z",
        ];
        let mut bad = Vec::new();
        for md in cases {
            for (name, html) in all_renders(md) {
                if has_reserved_tag(&html) || !html.to_ascii_lowercase().contains("&lt;birta-") {
                    bad.push(format!("{name}({md:?}):\n  {html}"));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "reserved tags not neutralized:\n{}",
            bad.join("\n")
        );
    }

    #[test]
    fn neutralizer_leaves_rendered_markup_alone() {
        let md = "para\n\n\
                  <div title=\"data-sourcepos=1 <birta-x>\">data-sourcepos=\"1:2\" data-br-x=1 in text <!-- <birta-y> --></div>\n\n\
                  `data-sourcepos=\"1:1\" data-br-x <birta-x>`\n\n\
                  plain data-sourcepos= text\n\n\
                  ```mermaid\nA[data-br-x=1] --> B[data-sourcepos=2]\n```\n";
        for (name, html) in all_renders(md) {
            for want in [
                "<p data-sourcepos=\"1:1-1:4\">para</p>",
                "<div title=\"data-sourcepos=1 <birta-x>\">data-sourcepos=\"1:2\" data-br-x=1 in text <!-- <birta-y> --></div>",
                ">data-sourcepos=&quot;1:1&quot; data-br-x &lt;birta-x&gt;</code>",
                ">plain data-sourcepos= text</p>",
                "A[data-br-x=1] --&gt; B[data-sourcepos=2]",
            ] {
                assert!(html.contains(want), "{name} lost {want:?}:\n{html}");
            }
        }
    }
}
