# birta-core

The rendering core of [birta](https://github.com/hugvit/birta): GitHub-flavoured markdown to HTML, syntax highlighting, themes, and the stylesheets that go with them. It has no server, no routes and no file watching; the host application supplies those.

The API follows birta's needs and may change between `0.x` releases. Pin a version or a git revision.

## Use

```rust
use birta_core::render::{RenderOptions, Resolved, Resolver, render};
use birta_core::theme::ThemeSearch;

struct Notes;

impl Resolver for Notes {
    fn image(&self, url: &str) -> String {
        format!("/files/{url}")
    }
    fn wikilink(&self, target: &str) -> Resolved {
        match lookup(target) {
            Some(path) => Resolved::Found(format!("/note/{path}")),
            None => Resolved::Missing(format!("/new?title={target}")),
        }
    }
}

let theme = ThemeSearch::default().resolve("catppuccin")?;
let page = render(
    markdown,
    &RenderOptions {
        syntax_theme: theme.active_data().syntax.as_ref(),
        resolver: Some(&Notes),
        wikilinks: true,
        front_matter: true,
        embeds: true,
        tags: true,
    },
);
```

## Modules

| Module | What it gives you |
|---|---|
| `render` | `render` (markdown to an HTML fragment), `Resolver` (your URL scheme), `front_matter_table`, `render_source` (highlighted raw view), `format_file_stats` |
| `theme` | `ThemeSearch` (find and resolve themes), `ResolvedTheme`, `ThemeRegistry` (switch theme and light/dark at runtime), the embedded themes |
| `diff` | `diff` (two texts to line hunks, rows and themed HTML), `DiffOptions`, `Layout` |
| `css` | The stylesheets as `&'static str`: `GITHUB_MARKDOWN`, `THEME_OVERRIDES`, `PAGE`, `SYNTAX`, `ALERTS`, `DIFF` |
| `highlight` | The syntect adapters behind `render` |

## Building a page

1. Put the rendered fragment inside an element with class `markdown-body`.
2. Include the stylesheets in the order listed in `css`, then the active theme's `css_vars`.
3. Set `data-theme="light"` or `"dark"` on the root element. For any theme other than `github`, also set `data-birta-theme="<name>"`.
4. Math and Mermaid are left for the browser: load KaTeX for `[data-math-style]` elements and Mermaid for `pre.mermaid`.

## Showing a diff

`diff::diff(old, new, &opts)` compares two versions of a document as highlighted source:

```rust
use birta_core::diff::{diff, DiffOptions};

let d = diff(theirs, mine, &DiffOptions {
    labels: Some(["Theirs", "Yours"]),
    ..Default::default()
});
// d.html: the view; d.hunks / d.rows / d.old_lines / d.new_lines: the same diff as data
```

- **Styles**: include `css::DIFF` (and `css::SYNTAX` in class mode) after the stylesheets above. Colours come from the syntax theme's `markup.inserted` / `markup.deleted` / `markup.changed` scopes when the theme has them, otherwise from its alert colours.
- **Layout**: one HTML renders unified or side by side. `data-layout="auto"` (the default) is side by side when the diff is at least 48rem wide. `auto` measures its own width, so give the diff a parent with a definite width (a block, not an inline-block or a shrink-to-fit flex item). Set `data-layout` to `unified` or `split` on the `.birta-diff` element to force one; no new diff is needed.
- **Data**: hunks are 0-based, half-open line ranges into `old_lines` and `new_lines`. In the HTML, each `.diff-hunk` wrapper carries `data-hunk`; removed cells carry a 1-based `data-old`, added cells a `data-new`, and context rows (`.diff-ctx`) both. A host can wire actions without parsing the HTML.
- **Line endings**: by default CRLF vs LF, a byte order mark and a missing final newline are ignored, each with its own flag and a note in the HTML. `LineEndings::Exact` compares bytes.
- **Text**: `DiffOptions::strings` replaces the fold and note text; `labels` names the two columns.
- **Scoped syntax CSS**: class-mode token classes in a diff are not under `pre.syntax-highlighting`. A host that scopes its syntax CSS there must add `.birta-diff` to that scope.

## Opt-in syntax

- **`wikilinks`**: `[[target]]` and `[[target|alias]]` become links. `Resolver::wikilink` turns the target into an `href`.
- **Unresolved targets**: `Resolver::wikilink` and `Resolver::embed` return `Resolved::Found(url)` or `Resolved::Missing(url)`. A missing wiki link keeps its `href` and gains `data-unresolved="true"`; a missing image embed becomes a `<span data-unresolved="true">` holding its target.
- **`front_matter`**: a leading `---` block is left out of the HTML and returned raw in `Rendered::front_matter`. `front_matter_table` turns that block into a GitHub-style table to place above the body.
- **`embeds`**: `![[image.png]]` becomes an `<img data-embed="image">`. `|300` sets the width, `|300x200` both sides, any other text after the pipe is the alt text. `Resolver::embed` turns the target into a `src`; by default it goes through `Resolver::image`. A paragraph that is only `![[note]]` or `![[note#Heading]]` becomes a `div.markdown-embed` holding that note, when `Resolver::embed_note` returns its markdown. Embedded notes render without `data-sourcepos`, heading ids and their front matter, nest at most three deep, and stop at cycles. Inline note embeds and `#^block` targets stay plain text. A host that writes checkboxes back by their `data-sourcepos` must skip inputs inside `.markdown-embed`, or `closest('li[data-sourcepos]')` finds a line of the host file.
- **`tags`**: `#tag` becomes `<span class="tag" data-tag="tag">`, or an `<a class="tag">` when `Resolver::tag` returns a URL. A tag starts after whitespace or at the start of a line, and must contain something other than digits, so `#1`, `C#` and URL fragments stay text.
- **Escapes**: `\![[x]]` and `\#tag` stay plain text, as do embeds and tags written with entities (`&#33;[[x]]`, `&#35;tag`).
- **`highlight`**: `==text==` becomes `<mark>`.
- **`current_note`**: the name of the note being rendered, so `![[itself]]` stays text instead of expanding once.
- **Found targets**: `Rendered::wikilinks`, `Rendered::embeds` and `Rendered::tags` list what each enabled syntax found, so an index needs no second parse.
- **Known limitation**: relative links and images inside an embedded note go through the host's `Resolver`, as if they were written in the host note. `embed_note` has no way to name the embedded note's own location yet.
- **Not supported yet**: inline note embeds, `^block` ids, and Obsidian callouts render as plain text.
