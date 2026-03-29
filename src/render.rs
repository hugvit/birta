use std::path::{Path, PathBuf};
use std::sync::Arc;

use comrak::nodes::NodeValue;
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{Arena, Options, format_html_with_plugins, options, parse_document};

use crate::highlight;
use crate::theme::SyntaxTheme;

/// Strategy for rewriting relative image URLs.
#[derive(Clone)]
enum RewriteMode {
    /// Rewrite to `/local/{path}` for the server to serve.
    Server,
    /// Rewrite to `file:///{base_dir}/{path}` for self-contained HTML.
    Static(PathBuf),
}

impl RewriteMode {
    fn rewrite(&self, url: &str) -> String {
        let clean = url.strip_prefix("./").unwrap_or(url);
        match self {
            RewriteMode::Server => format!("/local/{clean}"),
            RewriteMode::Static(base) => format!("file://{}", base.join(clean).display()),
        }
    }
}

/// Render markdown to HTML, rewriting relative image paths to `/local/` server URLs.
pub fn render(markdown: &str, syntax_theme: Option<&SyntaxTheme>) -> String {
    render_with_mode(markdown, syntax_theme, RewriteMode::Server)
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
    let mode = mode.clone();
    options.extension.image_url_rewriter = Some(Arc::new(move |url: &str| {
        if should_rewrite(url) {
            mode.rewrite(url)
        } else {
            url.to_string()
        }
    }));

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
                let rewritten = mode.rewrite(url);
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

fn html_escape(s: &str) -> String {
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
