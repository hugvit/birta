use comrak::plugins::syntect::SyntectAdapter;
use comrak::{Options, markdown_to_html_with_plugins, options};

use crate::highlight;

pub fn render(markdown: &str) -> String {
    let options = options();
    let adapter = highlight::adapter();
    let plugins = plugins(&adapter);
    let html = markdown_to_html_with_plugins(markdown, &options, &plugins);
    rewrite_local_image_paths(&html)
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
    options.extension.header_ids = Some("user-content-".to_string());

    // Parsing
    options.parse.smart = true;

    // Rendering
    options.render.github_pre_lang = true;
    options.render.r#unsafe = true;

    options
}

/// Rewrite relative image `src` attributes to go through `/local/`.
///
/// Skips absolute URLs, data URIs, and fragment-only references.
fn rewrite_local_image_paths(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut remaining = html;

    while let Some(img_pos) = remaining.find("<img") {
        result.push_str(&remaining[..img_pos]);
        remaining = &remaining[img_pos..];

        if let Some(src_start) = remaining.find("src=\"") {
            let before_src = &remaining[..src_start + 5]; // includes src="
            let after_src = &remaining[src_start + 5..];

            if let Some(src_end) = after_src.find('"') {
                let src_value = &after_src[..src_end];

                if should_rewrite(src_value) {
                    let clean = src_value.strip_prefix("./").unwrap_or(src_value);
                    result.push_str(before_src);
                    result.push_str("/local/");
                    result.push_str(clean);
                    result.push('"');
                    remaining = &after_src[src_end + 1..];
                } else {
                    result.push_str(&remaining[..src_start + 5 + src_end + 1]);
                    remaining = &after_src[src_end + 1..];
                }
            } else {
                result.push_str(&remaining[..4]);
                remaining = &remaining[4..];
            }
        } else {
            result.push_str(&remaining[..4]);
            remaining = &remaining[4..];
        }
    }

    result.push_str(remaining);
    result
}

fn should_rewrite(src: &str) -> bool {
    !src.is_empty()
        && !src.starts_with("http://")
        && !src.starts_with("https://")
        && !src.starts_with('/')
        && !src.starts_with("data:")
        && !src.starts_with('#')
}
