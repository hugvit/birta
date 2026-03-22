use std::sync::Arc;

use comrak::plugins::syntect::SyntectAdapter;
use comrak::{Options, markdown_to_html_with_plugins, options};

use crate::highlight;

pub fn render(markdown: &str) -> String {
    let options = options();
    let adapter = highlight::adapter();
    let plugins = plugins(&adapter);
    markdown_to_html_with_plugins(markdown, &options, &plugins)
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
