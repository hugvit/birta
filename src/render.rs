use comrak::plugins::syntect::SyntectAdapter;
use comrak::{Options, markdown_to_html_with_plugins, options};

use crate::highlight;

pub fn render(markdown: &str) -> String {
    let options = options();
    let adapter = highlight::adapter();
    let plugins = plugins(&adapter);
    let html = markdown_to_html_with_plugins(markdown, &options, &plugins);
    let html = postprocess_alerts(&html);
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

/// Post-process blockquotes to convert GitHub-style alerts.
///
/// Detects `<blockquote>\n<p>[!TYPE]\n` and transforms into styled alert divs.
fn postprocess_alerts(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut remaining = html;

    while let Some(bq_pos) = remaining.find("<blockquote>\n<p>[!") {
        result.push_str(&remaining[..bq_pos]);
        let after_marker = &remaining[bq_pos + "<blockquote>\n<p>[!".len()..];

        if let Some(end_bracket) = after_marker.find("]\n") {
            let alert_type_raw = &after_marker[..end_bracket];
            let alert_type_lower = alert_type_raw.to_ascii_lowercase();

            if matches!(
                alert_type_lower.as_str(),
                "note" | "tip" | "important" | "warning" | "caution"
            ) {
                let icon = alert_icon(&alert_type_lower);
                let title = alert_title(&alert_type_lower);

                result.push_str(&format!(
                    "<div class=\"markdown-alert markdown-alert-{alert_type_lower}\">\n\
                     <p class=\"markdown-alert-title\">{icon} {title}</p>\n<p>"
                ));

                // Skip past the `[!TYPE]\n` and continue with rest of blockquote content
                remaining = &after_marker[end_bracket + 2..];

                // Find the closing </blockquote> and replace it with </div>
                if let Some(close_pos) = remaining.find("</blockquote>") {
                    result.push_str(&remaining[..close_pos]);
                    result.push_str("</div>");
                    remaining = &remaining[close_pos + "</blockquote>".len()..];
                } else {
                    // No closing tag found, just continue
                }
            } else {
                // Not a recognized alert type, pass through
                result.push_str("<blockquote>\n<p>[!");
                remaining = after_marker;
            }
        } else {
            result.push_str("<blockquote>\n<p>[!");
            remaining = after_marker;
        }
    }

    result.push_str(remaining);
    result
}

fn alert_icon(alert_type: &str) -> &'static str {
    match alert_type {
        "note" => {
            "<svg class=\"octicon\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\"><path fill=\"currentColor\" d=\"M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z\"></path></svg>"
        }
        "tip" => {
            "<svg class=\"octicon\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\"><path fill=\"currentColor\" d=\"M8 1.5c-2.363 0-4 1.69-4 3.75 0 .984.424 1.625.984 2.304l.214.253c.223.264.47.556.673.848.284.411.537.896.621 1.49a.75.75 0 0 1-1.484.211c-.04-.282-.163-.547-.37-.847a8.456 8.456 0 0 0-.542-.68c-.084-.1-.173-.205-.268-.32C3.201 7.75 2.5 6.766 2.5 5.25 2.5 2.31 4.863 0 8 0s5.5 2.31 5.5 5.25c0 1.516-.701 2.5-1.328 3.259-.095.115-.184.22-.268.319-.207.245-.383.453-.541.681-.208.3-.33.565-.37.847a.751.751 0 0 1-1.485-.212c.084-.593.337-1.078.621-1.489.203-.292.45-.584.673-.848.075-.088.147-.173.213-.253.561-.679.985-1.32.985-2.304 0-2.06-1.637-3.75-4-3.75ZM5.75 12h4.5a.75.75 0 0 1 0 1.5h-4.5a.75.75 0 0 1 0-1.5ZM6 15.25a.75.75 0 0 1 .75-.75h2.5a.75.75 0 0 1 0 1.5h-2.5a.75.75 0 0 1-.75-.75Z\"></path></svg>"
        }
        "important" => {
            "<svg class=\"octicon\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\"><path fill=\"currentColor\" d=\"M0 1.75C0 .784.784 0 1.75 0h12.5C15.216 0 16 .784 16 1.75v9.5A1.75 1.75 0 0 1 14.25 13H8.06l-2.573 2.573A1.458 1.458 0 0 1 3 14.543V13H1.75A1.75 1.75 0 0 1 0 11.25Zm1.75-.25a.25.25 0 0 0-.25.25v9.5c0 .138.112.25.25.25h2a.75.75 0 0 1 .75.75v2.19l2.72-2.72a.749.749 0 0 1 .53-.22h6.5a.25.25 0 0 0 .25-.25v-9.5a.25.25 0 0 0-.25-.25Zm7 2.25v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 9a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z\"></path></svg>"
        }
        "warning" => {
            "<svg class=\"octicon\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\"><path fill=\"currentColor\" d=\"M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z\"></path></svg>"
        }
        "caution" => {
            "<svg class=\"octicon\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\"><path fill=\"currentColor\" d=\"M4.47.22A.749.749 0 0 1 5 0h6c.199 0 .389.079.53.22l4.25 4.25c.141.14.22.331.22.53v6a.749.749 0 0 1-.22.53l-4.25 4.25A.749.749 0 0 1 11 16H5a.749.749 0 0 1-.53-.22L.22 11.53A.749.749 0 0 1 0 11V5c0-.199.079-.389.22-.53Zm.84 1.28L1.5 5.31v5.38l3.81 3.81h5.38l3.81-3.81V5.31L10.69 1.5ZM8 4a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 8 4Zm0 8a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z\"></path></svg>"
        }
        _ => "",
    }
}

fn alert_title(alert_type: &str) -> &'static str {
    match alert_type {
        "note" => "Note",
        "tip" => "Tip",
        "important" => "Important",
        "warning" => "Warning",
        "caution" => "Caution",
        _ => "",
    }
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
