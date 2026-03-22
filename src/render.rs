use std::sync::Arc;

use comrak::nodes::NodeValue;
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{Arena, Options, format_html_with_plugins, options, parse_document};

use crate::highlight;

pub fn render(markdown: &str) -> String {
    let options = options();
    let adapter = highlight::adapter();
    let plugins = plugins(&adapter);

    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &options);

    // Convert mermaid code blocks to raw HTML so syntect doesn't process them
    for node in root.descendants() {
        let mut data = node.data.borrow_mut();
        if let NodeValue::CodeBlock(ref code_block) = data.value {
            if code_block.info == "mermaid" {
                let html = format!(
                    "<pre class=\"mermaid\">{}</pre>\n",
                    html_escape(&code_block.literal)
                );
                data.value = NodeValue::HtmlBlock(comrak::nodes::NodeHtmlBlock {
                    block_type: 6,
                    literal: html,
                });
            }
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

fn options() -> Options<'static> {
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

    // Rewrite relative image paths to go through /local/
    options.extension.image_url_rewriter = Some(Arc::new(|url: &str| {
        if should_rewrite(url) {
            let clean = url.strip_prefix("./").unwrap_or(url);
            format!("/local/{clean}")
        } else {
            url.to_string()
        }
    }));

    // Parsing
    options.parse.smart = true;

    // Rendering
    options.render.github_pre_lang = true;
    options.render.r#unsafe = true;

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
