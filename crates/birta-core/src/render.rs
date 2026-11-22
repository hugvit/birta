//! Markdown to HTML, with hooks for the host application's URL scheme.

use comrak::nodes::NodeValue;
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{Arena, Options, format_html_with_plugins, options, parse_document};

use yaml_rust2::{Yaml, YamlLoader};

use crate::theme::SyntaxTheme;
use crate::{highlight, obsidian};

/// Maps the URLs found in a document onto the host application's routes.
///
/// `image` and `link` are only called for relative URLs: absolute URLs
/// (`http:`, `https:`, `data:`, a leading `/`) and `#fragment` links pass
/// through untouched, and so do `mailto:` and `tel:` links. Every method
/// defaults to returning its input, so implement only what the host needs.
pub trait Resolver {
    /// The `src` for a relative image, from markdown or a raw `<img>` tag.
    fn image(&self, url: &str) -> String {
        url.to_string()
    }

    /// The `href` for a relative link. `url` may carry a `?query` or `#fragment`.
    fn link(&self, url: &str) -> String {
        url.to_string()
    }

    /// The `href` for a `[[target]]` wiki link. `target` is the text before
    /// any `|alias`. Only called when [`RenderOptions::wikilinks`] is set.
    fn wikilink(&self, target: &str) -> String {
        target.to_string()
    }

    /// The `src` for an `![[image.png]]` embed. `target` is the text before
    /// any `|size`. Only called when [`RenderOptions::embeds`] is set. The
    /// default treats it like a relative image and calls [`Resolver::image`].
    fn embed(&self, target: &str) -> String {
        if is_relative(target) {
            self.image(target)
        } else {
            target.to_string()
        }
    }

    /// The markdown of the note an `![[note]]` or `![[note#Heading]]` embed
    /// points at, or `None` to leave the embed as text. `name` is the text
    /// before any `#` or `|`. Only called when [`RenderOptions::embeds`] is set.
    fn embed_note(&self, _name: &str) -> Option<String> {
        None
    }

    /// The `href` for a `#tag`, or `None` to render it as a plain
    /// `<span class="tag">`. Only called when [`RenderOptions::tags`] is set.
    fn tag(&self, _name: &str) -> Option<String> {
        None
    }
}

/// How to render a document. The default is GitHub-flavoured markdown with
/// class-based code highlighting and every URL left as written.
#[derive(Clone, Copy, Default)]
pub struct RenderOptions<'a> {
    /// Highlight code with this theme's inline colours instead of CSS classes.
    pub syntax_theme: Option<&'a SyntaxTheme>,
    /// Rewrites image, link and wiki-link targets.
    pub resolver: Option<&'a dyn Resolver>,
    /// Parse `[[target]]` and `[[target|alias]]` as links.
    pub wikilinks: bool,
    /// Treat a leading `---` block as front matter: it is left out of the
    /// HTML and returned in [`Rendered::front_matter`].
    pub front_matter: bool,
    /// Render Obsidian image embeds: `![[image.png]]`, `![[image.png|300]]`
    /// (width) and `![[image.png|300x200]]`. A paragraph that is only a note
    /// embed (`![[note]]`, `![[note#Heading]]`) is replaced by the note when
    /// [`Resolver::embed_note`] returns it; other embeds stay plain text.
    pub embeds: bool,
    /// Render Obsidian `#tags` as `<span class="tag">`, or as links when
    /// [`Resolver::tag`] returns a URL.
    pub tags: bool,
}

/// The result of [`render`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// HTML fragment for the document body, to place inside `.markdown-body`.
    pub html: String,
    /// The raw front matter block including its `---` delimiters, when
    /// [`RenderOptions::front_matter`] is set and the document has one.
    pub front_matter: Option<String>,
    /// `[[wiki link]]` targets, before resolving, in document order without
    /// duplicates. Empty unless [`RenderOptions::wikilinks`] is set.
    pub wikilinks: Vec<String>,
    /// `![[embed]]` targets, images or not, in document order without
    /// duplicates. Empty unless [`RenderOptions::embeds`] is set.
    pub embeds: Vec<String>,
    /// `#tag` names without the `#`, in document order without duplicates.
    /// Empty unless [`RenderOptions::tags`] is set.
    pub tags: Vec<String>,
}

/// Render markdown to an HTML fragment.
///
/// Block elements carry `data-sourcepos` attributes with their source lines,
/// counted from the top of the file even when front matter is stripped.
pub fn render(markdown: &str, opts: &RenderOptions<'_>) -> Rendered {
    render_nested(markdown, opts, &[])
}

/// How deep note embeds may nest inside one another.
const MAX_EMBED_DEPTH: usize = 3;

/// `embedding` names the notes being embedded around this one, outermost first.
fn render_nested(markdown: &str, opts: &RenderOptions<'_>, embedding: &[&str]) -> Rendered {
    let mut options = comrak_options(opts);
    // Line numbers in an embedded note point into another file
    options.render.sourcepos = embedding.is_empty();
    let adapter = match opts.syntax_theme {
        Some(st) => highlight::adapter_with_theme(st),
        None => highlight::adapter(),
    };
    let plugins = plugins(&adapter);

    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &options);
    let mut front_matter = None;
    let mut wikilinks: Vec<String> = Vec::new();

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
            NodeValue::FrontMatter(raw) => front_matter = Some(raw.clone()),
            value => {
                if let NodeValue::WikiLink(link) = value
                    && !wikilinks.contains(&link.url)
                {
                    wikilinks.push(link.url.clone());
                }
                if let Some(resolver) = opts.resolver {
                    resolve_urls(value, resolver);
                }
            }
        }
    }

    // After resolving, so the embeds it emits as raw HTML aren't resolved twice
    let found = obsidian::rewrite(
        &arena,
        root,
        markdown,
        &obsidian::Syntax {
            embeds: opts.embeds,
            tags: opts.tags,
            resolver: opts.resolver,
            note: &|target| embed_note(target, opts, embedding),
        },
    );

    let mut html = String::new();
    format_html_with_plugins(root, &options, &mut html, &plugins).unwrap();
    Rendered {
        html,
        front_matter,
        wikilinks,
        embeds: found.embeds,
        tags: found.tags,
    }
}

/// Render the note behind a `![[note#Heading]]` target, wrapped in a
/// `div.markdown-embed`. `None` on a cycle, past [`MAX_EMBED_DEPTH`], for a
/// missing note or heading, and for `#^block` targets.
fn embed_note(target: &str, opts: &RenderOptions<'_>, embedding: &[&str]) -> Option<String> {
    let resolver = opts.resolver?;
    let (name, heading) = match target.split_once('#') {
        Some((name, heading)) => (name.trim(), Some(heading.trim())),
        None => (target.trim(), None),
    };
    if name.is_empty()
        || embedding.contains(&name)
        || embedding.len() >= MAX_EMBED_DEPTH
        || heading.is_some_and(|h| h.starts_with('^'))
    {
        return None;
    }

    let markdown = resolver.embed_note(name)?;
    let markdown = match heading {
        Some(heading) => heading_section(&markdown, heading)?,
        None => markdown,
    };
    let mut stack = embedding.to_vec();
    stack.push(name);
    let nested = RenderOptions {
        front_matter: true,
        ..*opts
    };
    let html = render_nested(&markdown, &nested, &stack).html;
    Some(format!(
        "<div class=\"markdown-embed\" data-embed=\"note\" data-embed-target=\"{}\">\n{html}</div>\n",
        html_escape(target)
    ))
}

/// The lines from the heading whose text is `heading` up to the next heading
/// of the same or a higher level.
fn heading_section(markdown: &str, heading: &str) -> Option<String> {
    let mut options = Options::default();
    options.extension.front_matter_delimiter = Some("---".to_string());
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &options);

    let headings: Vec<(u8, usize, String)> = root
        .children()
        .filter_map(|node| {
            let data = node.data.borrow();
            let NodeValue::Heading(h) = &data.value else {
                return None;
            };
            let text: String = node
                .descendants()
                .filter_map(|n| match &n.data.borrow().value {
                    NodeValue::Text(t) => Some(t.to_string()),
                    NodeValue::Code(c) => Some(c.literal.clone()),
                    _ => None,
                })
                .collect();
            Some((h.level, data.sourcepos.start.line, text))
        })
        .collect();

    let index = headings
        .iter()
        .position(|(_, _, text)| text.trim().to_lowercase() == heading.to_lowercase())?;
    let (level, start, _) = headings[index];
    let end = headings[index + 1..]
        .iter()
        .find(|(l, _, _)| *l <= level)
        .map(|(_, line, _)| line - 1);

    let lines: Vec<&str> = markdown.lines().collect();
    let end = end.unwrap_or(lines.len()).min(lines.len());
    Some(lines.get(start.checked_sub(1)?..end)?.join("\n"))
}

/// Render a front matter block as a GitHub-style table: one column per key,
/// one row of values. Takes the raw block from [`Rendered::front_matter`].
///
/// Nested mappings become nested tables and lists of plain values are joined
/// with commas. A block that is not a YAML mapping is shown as written in a
/// `<pre>`, and an empty block renders nothing.
pub fn front_matter_table(raw: &str) -> String {
    match YamlLoader::load_from_str(raw).ok().as_deref() {
        Some([Yaml::Hash(map), ..]) if map.is_empty() => String::new(),
        Some([Yaml::Hash(map), ..]) => yaml_table(map, " class=\"front-matter\""),
        Some([] | [Yaml::Null, ..]) => String::new(),
        _ => {
            let body: Vec<&str> = raw.lines().filter(|l| l.trim_end() != "---").collect();
            format!(
                "<pre class=\"front-matter\"><code>{}</code></pre>\n",
                html_escape(&body.join("\n"))
            )
        }
    }
}

fn yaml_table(map: &yaml_rust2::yaml::Hash, attrs: &str) -> String {
    let mut head = String::new();
    let mut row = String::new();
    for (key, value) in map {
        head.push_str(&format!("<th>{}</th>", yaml_cell(key)));
        row.push_str(&format!("<td>{}</td>", yaml_cell(value)));
    }
    format!("<table{attrs}><thead><tr>{head}</tr></thead><tbody><tr>{row}</tr></tbody></table>\n")
}

fn yaml_cell(value: &Yaml) -> String {
    match value {
        Yaml::Hash(map) => yaml_table(map, ""),
        Yaml::Array(items)
            if items
                .iter()
                .any(|i| matches!(i, Yaml::Hash(_) | Yaml::Array(_))) =>
        {
            let rows: String = items
                .iter()
                .map(|i| format!("<tr><td>{}</td></tr>", yaml_cell(i)))
                .collect();
            format!("<table><tbody>{rows}</tbody></table>")
        }
        Yaml::Array(items) => items.iter().map(yaml_cell).collect::<Vec<_>>().join(", "),
        Yaml::String(s) | Yaml::Real(s) => html_escape(s),
        Yaml::Integer(n) => n.to_string(),
        Yaml::Boolean(b) => b.to_string(),
        _ => String::new(),
    }
}

fn resolve_urls(value: &mut NodeValue, resolver: &dyn Resolver) {
    match value {
        NodeValue::HtmlBlock(block) => {
            block.literal = rewrite_html_img_srcs(&block.literal, resolver);
        }
        NodeValue::HtmlInline(raw) => *raw = rewrite_html_img_srcs(raw, resolver),
        NodeValue::Link(link) if is_relative_link(&link.url) => {
            link.url = resolver.link(&link.url);
        }
        NodeValue::Image(image) if is_relative(&image.url) => {
            image.url = resolver.image(&image.url);
        }
        NodeValue::WikiLink(link) => link.url = resolver.wikilink(&link.url),
        _ => {}
    }
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

fn plugins(adapter: &SyntectAdapter) -> options::Plugins<'_> {
    let mut plugins = options::Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(adapter);
    plugins
}

fn comrak_options(opts: &RenderOptions<'_>) -> Options<'static> {
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

    // Obsidian-style `[[target|alias]]`: the alias comes after the pipe
    options.extension.wikilinks_title_after_pipe = opts.wikilinks;
    if opts.front_matter {
        options.extension.front_matter_delimiter = Some("---".to_string());
    }

    // Parsing
    options.parse.smart = true;

    // Rendering
    options.render.github_pre_lang = true;
    options.render.r#unsafe = true;
    options.render.sourcepos = true;

    options
}

pub(crate) fn is_relative(src: &str) -> bool {
    !src.is_empty()
        && !src.starts_with("http://")
        && !src.starts_with("https://")
        && !src.starts_with('/')
        && !src.starts_with("data:")
        && !src.starts_with('#')
}

fn is_relative_link(url: &str) -> bool {
    is_relative(url) && !url.starts_with("mailto:") && !url.starts_with("tel:")
}

/// Rewrite relative `src="..."` attributes in raw HTML `<img>` tags.
fn rewrite_html_img_srcs(html: &str, resolver: &dyn Resolver) -> String {
    let mut result = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(pos) = rest.find("src=\"") {
        result.push_str(&rest[..pos]);
        let after_src = &rest[pos + 5..]; // skip past src="
        if let Some(end) = after_src.find('"') {
            let url = &after_src[..end];
            if is_relative(url) {
                let rewritten = resolver.image(url);
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

    /// Prefixes every hook's input so tests can see which hook ran.
    struct Tagging;

    impl Resolver for Tagging {
        fn image(&self, url: &str) -> String {
            format!("/img/{url}")
        }
        fn link(&self, url: &str) -> String {
            format!("/doc/{url}")
        }
        fn wikilink(&self, target: &str) -> String {
            format!("/note/{target}")
        }
    }

    fn with_resolver(markdown: &str) -> String {
        let opts = RenderOptions {
            resolver: Some(&Tagging),
            wikilinks: true,
            ..Default::default()
        };
        render(markdown, &opts).html
    }

    #[test]
    fn default_options_leave_urls_as_written() {
        let html = render("[a](b.md) ![c](d.png)", &RenderOptions::default()).html;
        assert!(html.contains("href=\"b.md\""), "got: {html}");
        assert!(html.contains("src=\"d.png\""), "got: {html}");
    }

    #[test]
    fn resolver_rewrites_relative_links_and_images() {
        let html = with_resolver("[a](b.md#s) ![c](d.png)");
        assert!(html.contains("href=\"/doc/b.md#s\""), "got: {html}");
        assert!(html.contains("src=\"/img/d.png\""), "got: {html}");
    }

    #[test]
    fn resolver_rewrites_raw_html_images() {
        let html = with_resolver("<img src=\"logo.png\">");
        assert!(html.contains("src=\"/img/logo.png\""), "got: {html}");
    }

    #[test]
    fn resolver_skips_absolute_anchor_and_mailto_urls() {
        let html = with_resolver(
            "[a](https://x.com) [b](#h) [c](mailto:a@b.c) [d](/abs) ![e](https://x.com/e.png)",
        );
        assert!(!html.contains("/doc/"), "got: {html}");
        assert!(!html.contains("/img/"), "got: {html}");
    }

    #[test]
    fn wikilinks_are_literal_text_by_default() {
        let html = render("See [[note]].", &RenderOptions::default()).html;
        assert!(html.contains("[[note]]"), "got: {html}");
        assert!(!html.contains("<a "), "got: {html}");
    }

    #[test]
    fn wikilink_target_goes_through_the_resolver() {
        let html = with_resolver("See [[world/note]].");
        assert!(
            html.contains("href=\"/note/world/note\" data-wikilink=\"true\">world/note</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn wikilink_alias_comes_after_the_pipe() {
        let html = with_resolver("[[note|an alias]]");
        assert!(
            html.contains("href=\"/note/note\" data-wikilink=\"true\">an alias</a>"),
            "got: {html}"
        );
    }

    fn obsidian(markdown: &str) -> Rendered {
        let opts = RenderOptions {
            resolver: Some(&Tagging),
            wikilinks: true,
            embeds: true,
            tags: true,
            ..Default::default()
        };
        render(markdown, &opts)
    }

    #[test]
    fn embeds_and_tags_are_literal_text_by_default() {
        let rendered = render("![[a.png]] #idea", &RenderOptions::default());
        assert!(
            rendered.html.contains("![[a.png]] #idea"),
            "got: {}",
            rendered.html
        );
        assert!(rendered.embeds.is_empty() && rendered.tags.is_empty());
    }

    #[test]
    fn image_embed_goes_through_the_resolver() {
        let html = obsidian("See ![[pics/a.png]] here").html;
        assert!(
            html.contains(
                "See <img src=\"/img/pics/a.png\" alt=\"pics/a.png\" data-embed=\"image\" /> here"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn image_embed_works_without_wikilinks_or_a_resolver() {
        let opts = RenderOptions {
            embeds: true,
            ..Default::default()
        };
        let html = render("![[a.PNG]]", &opts).html;
        assert!(html.contains("<img src=\"a.PNG\""), "got: {html}");
    }

    #[test]
    fn image_embed_size_and_alt() {
        let html = obsidian("![[a.png|300]] ![[b.png|30x20]] ![[c.png|A cat]]").html;
        assert!(html.contains("alt=\"a.png\" width=\"300\" "), "got: {html}");
        assert!(
            html.contains("alt=\"b.png\" width=\"30\" height=\"20\" "),
            "got: {html}"
        );
        assert!(html.contains("alt=\"A cat\" data-embed"), "got: {html}");
    }

    #[test]
    fn image_embed_encodes_and_escapes() {
        let html = obsidian("![[Pasted image 1.png|a \"b\" & c]]").html;
        assert!(
            html.contains("src=\"/img/Pasted%20image%201.png\" alt=\"a &quot;b&quot; &amp; c\""),
            "got: {html}"
        );
    }

    #[test]
    fn image_embed_target_ignores_smart_punctuation() {
        let html = obsidian("> - ![[it's -- \"x\"....png]]").html;
        assert!(
            html.contains("src=\"/img/it&#x27;s%20--%20%22x%22....png\""),
            "got: {html}"
        );
    }

    #[test]
    fn image_embed_in_a_table_cell_takes_an_escaped_pipe() {
        let html = obsidian("| x |\n|---|\n| ![[a.png\\|300]] |").html;
        assert!(
            html.contains("src=\"/img/a.png\" alt=\"a.png\" width=\"300\""),
            "got: {html}"
        );
    }

    #[test]
    fn image_embed_target_survives_non_ascii_before_it() {
        let html = obsidian("> - Þú sérð ![[it's.png]]").html;
        assert!(html.contains("src=\"/img/it&#x27;s.png\""), "got: {html}");
    }

    #[test]
    fn image_embed_leaves_absolute_urls_alone() {
        let html = obsidian("![[https://x.com/a.png]] ![[/abs.png]]").html;
        assert!(html.contains("src=\"/abs.png\""), "got: {html}");
        assert!(!html.contains("/img/"), "got: {html}");
    }

    #[test]
    fn embeds_in_code_links_and_escapes_stay_literal() {
        let html =
            obsidian("`![[a.png]]`\n\n```\n![[b.png]]\n```\n\n[![[c.png]]](x.md) \\![[d.png]]")
                .html;
        assert!(html.contains(">![[a.png]]</code>"), "got: {html}");
        assert!(html.contains("![[b.png]]\n</span>"), "got: {html}");
        assert!(!html.contains("data-embed"), "got: {html}");
        // An escaped bang leaves an ordinary wiki link
        assert!(html.contains("href=\"/note/d.png\""), "got: {html}");
    }

    #[test]
    fn note_embeds_stay_literal_but_are_recorded() {
        let rendered = obsidian("![[Ideas#Next]] ![[a.png|2]] ![[a.png]]");
        assert!(
            rendered.html.contains("![[Ideas#Next]]"),
            "got: {}",
            rendered.html
        );
        assert_eq!(rendered.embeds, vec!["Ideas#Next", "a.png"]);
    }

    /// Serves notes from a fixed list, for note embeds.
    struct Vault(&'static [(&'static str, &'static str)]);

    impl Resolver for Vault {
        fn embed_note(&self, name: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, md)| md.to_string())
        }
    }

    fn with_vault(vault: &'static Vault, markdown: &str) -> Rendered {
        let opts = RenderOptions {
            resolver: Some(vault),
            embeds: true,
            tags: true,
            wikilinks: true,
            front_matter: true,
            ..Default::default()
        };
        render(markdown, &opts)
    }

    #[test]
    fn note_embed_replaces_its_paragraph() {
        static VAULT: Vault = Vault(&[("Other", "---\ntitle: x\n---\n- [ ] task #t [[x]]\n")]);
        let rendered = with_vault(&VAULT, "# Host\n\n![[Other]]\n\nafter #host");
        let html = &rendered.html;
        assert!(
            html.contains("<div class=\"markdown-embed\" data-embed=\"note\" data-embed-target=\"Other\">\n<ul>"),
            "got: {html}"
        );
        assert!(
            !html.contains("title: x"),
            "front matter is stripped, got: {html}"
        );
        assert!(!html.contains("<p data-sourcepos=\"3"), "got: {html}");
        // Embedded checkboxes must not point at lines of the host file
        assert!(html.contains("<li><input type=\"checkbox\""), "got: {html}");
        assert_eq!(rendered.embeds, vec!["Other"]);
        assert_eq!(
            rendered.tags,
            vec!["host"],
            "embedded tags belong to the other note"
        );
        assert!(rendered.wikilinks.is_empty());
    }

    #[test]
    fn note_embed_takes_a_heading_section() {
        static VAULT: Vault = Vault(&[(
            "N",
            "# Top\nintro\n## Plan\nin plan\n### Sub\nin sub\n## Next\nnot this\n",
        )]);
        let html = with_vault(&VAULT, "![[N#plan]]").html;
        assert!(
            html.contains("in plan") && html.contains("in sub"),
            "got: {html}"
        );
        assert!(
            !html.contains("intro") && !html.contains("not this"),
            "got: {html}"
        );
    }

    #[test]
    fn note_embed_heading_matches_non_ascii_case() {
        static VAULT: Vault = Vault(&[("N", "## Þróun\nþetta\n")]);
        let html = with_vault(&VAULT, "![[N#þróun]]").html;
        assert!(html.contains("þetta"), "got: {html}");
    }

    #[test]
    fn note_embed_with_carriage_returns_does_not_panic() {
        static VAULT: Vault = Vault(&[("N", "# A\rintro\r## B\rx")]);
        with_vault(&VAULT, "![[N#B]]");
    }

    #[test]
    fn note_embeds_that_cannot_render_stay_text() {
        static VAULT: Vault = Vault(&[("N", "# A\n")]);
        let html = with_vault(
            &VAULT,
            "![[Missing]]\n\n![[N#Nope]]\n\n![[N#^block]]\n\nInline ![[N]] here",
        )
        .html;
        for literal in [
            "![[Missing]]",
            "![[N#Nope]]",
            "![[N#^block]]",
            "Inline ![[N]] here",
        ] {
            assert!(html.contains(literal), "{literal} should stay, got: {html}");
        }
        assert!(!html.contains("markdown-embed"), "got: {html}");
    }

    #[test]
    fn note_embed_cycle_stops() {
        static VAULT: Vault = Vault(&[("A", "in a\n\n![[B]]"), ("B", "in b\n\n![[A]]")]);
        let html = with_vault(&VAULT, "![[A]]").html;
        assert_eq!(html.matches("in a").count(), 1, "got: {html}");
        assert_eq!(html.matches("in b").count(), 1, "got: {html}");
        assert!(html.contains("<p>![[A]]</p>"), "got: {html}");
    }

    #[test]
    fn note_embed_depth_is_limited() {
        static VAULT: Vault = Vault(&[
            ("1", "one\n\n![[2]]"),
            ("2", "two\n\n![[3]]"),
            ("3", "three\n\n![[4]]"),
            ("4", "four"),
        ]);
        let html = with_vault(&VAULT, "![[1]]").html;
        assert!(html.contains("three"), "got: {html}");
        assert!(!html.contains("four"), "got: {html}");
        assert!(html.contains("<p>![[4]]</p>"), "got: {html}");
    }

    #[test]
    fn tags_render_as_spans() {
        let html = obsidian("Note #idea and #a/b-c_d.").html;
        assert!(
            html.contains(
                "Note <span class=\"tag\" data-tag=\"idea\">#idea</span> and \
                 <span class=\"tag\" data-tag=\"a/b-c_d\">#a/b-c_d</span>."
            ),
            "got: {html}"
        );
    }

    #[test]
    fn tags_link_when_the_resolver_gives_a_url() {
        struct Tags;
        impl Resolver for Tags {
            fn tag(&self, name: &str) -> Option<String> {
                Some(format!("/tags/{name}"))
            }
        }
        let opts = RenderOptions {
            resolver: Some(&Tags),
            tags: true,
            ..Default::default()
        };
        let html = render("#idea", &opts).html;
        assert!(
            html.contains("<a class=\"tag\" href=\"/tags/idea\" data-tag=\"idea\">#idea</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn things_that_are_not_tags() {
        let html = obsidian(
            "#1 #2024 C#x a#b # x `#code` [#l](u) <https://x.com/#frag> **b**#x\n\n```\n#c\n```",
        )
        .html;
        assert!(!html.contains("class=\"tag\""), "got: {html}");
    }

    #[test]
    fn tags_after_line_breaks_and_unicode() {
        let rendered = obsidian("one\n#tvö (#þrjú) **x** #y2024");
        assert_eq!(rendered.tags, vec!["tvö", "þrjú", "y2024"]);
    }

    #[test]
    fn tag_in_heading_keeps_the_heading_id() {
        let plain = render("# Plan #draft", &RenderOptions::default()).html;
        let tagged = obsidian("# Plan #draft").html;
        let id = |h: &str| {
            h.split("id=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(id(&plain), id(&tagged), "got: {tagged}");
        assert!(tagged.contains("class=\"tag\""), "got: {tagged}");
    }

    #[test]
    fn rendered_lists_wikilinks() {
        let rendered = obsidian("[[a]] [[b|B]] [[a]]");
        assert_eq!(rendered.wikilinks, vec!["a", "b"]);
    }

    #[test]
    fn front_matter_renders_as_content_by_default() {
        let rendered = render("---\ntitle: x\n---\n# T\n", &RenderOptions::default());
        assert_eq!(rendered.front_matter, None);
        assert!(rendered.html.contains("title: x"), "got: {}", rendered.html);
    }

    #[test]
    fn front_matter_is_stripped_and_returned_when_enabled() {
        let opts = RenderOptions {
            front_matter: true,
            ..Default::default()
        };
        let rendered = render("---\ntitle: x\ntags: [a]\n---\n# T\n", &opts);
        assert!(
            !rendered.html.contains("title: x"),
            "got: {}",
            rendered.html
        );
        let raw = rendered
            .front_matter
            .expect("front matter should be returned");
        assert!(raw.contains("title: x\ntags: [a]"), "got: {raw}");
    }

    #[test]
    fn sourcepos_counts_lines_from_the_top_of_the_file() {
        let opts = RenderOptions {
            front_matter: true,
            ..Default::default()
        };
        let html = render("---\ntitle: x\n---\n# T\n", &opts).html;
        assert!(html.contains("data-sourcepos=\"4:1-4:3\""), "got: {html}");
    }

    #[test]
    fn front_matter_table_has_a_column_per_key() {
        let html = front_matter_table(
            "---\ntitle: Ideas\ntags: [self, bench]\ncreated: 2026-01-01\n---\n",
        );
        assert_eq!(
            html,
            "<table class=\"front-matter\"><thead><tr><th>title</th><th>tags</th><th>created</th></tr></thead>\
             <tbody><tr><td>Ideas</td><td>self, bench</td><td>2026-01-01</td></tr></tbody></table>\n"
        );
    }

    #[test]
    fn front_matter_table_escapes_html() {
        let html = front_matter_table("---\ntitle: \"<b>x</b> & y\"\n---\n");
        assert!(
            html.contains("<td>&lt;b&gt;x&lt;/b&gt; &amp; y</td>"),
            "got: {html}"
        );
    }

    #[test]
    fn front_matter_table_nests_mappings_and_joins_block_lists() {
        let html = front_matter_table("---\nmeta:\n  a: 1\ntags:\n  - x\n  - y\n---\n");
        assert!(
            html.contains("<td><table><thead><tr><th>a</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table>\n</td>"),
            "got: {html}"
        );
        assert!(html.contains("<td>x, y</td>"), "got: {html}");
    }

    #[test]
    fn front_matter_table_shows_non_mapping_blocks_as_written() {
        let html = front_matter_table("---\njust <text>\n---\n");
        assert_eq!(
            html,
            "<pre class=\"front-matter\"><code>just &lt;text&gt;</code></pre>\n"
        );
        let html = front_matter_table("---\nkey: [unclosed\n---\n");
        assert!(
            html.starts_with("<pre class=\"front-matter\">"),
            "got: {html}"
        );
    }

    #[test]
    fn front_matter_table_is_empty_for_an_empty_block() {
        assert_eq!(front_matter_table("---\n---\n"), "");
    }

    #[test]
    fn document_without_front_matter_returns_none() {
        let opts = RenderOptions {
            front_matter: true,
            ..Default::default()
        };
        assert_eq!(render("# T\n", &opts).front_matter, None);
    }

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
