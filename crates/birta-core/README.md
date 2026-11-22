# birta-core

The rendering core of [birta](https://github.com/hugvit/birta): GitHub-flavoured markdown to HTML, syntax highlighting, themes, and the stylesheets that go with them. It has no server, no routes and no file watching; the host application supplies those.

The API follows birta's needs and may change between `0.x` releases. Pin a version or a git revision.

## Use

```rust
use birta_core::render::{RenderOptions, Resolver, render};
use birta_core::theme::ThemeSearch;

struct Notes;

impl Resolver for Notes {
    fn image(&self, url: &str) -> String {
        format!("/files/{url}")
    }
    fn wikilink(&self, target: &str) -> String {
        format!("/note/{target}")
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
    },
);
```

## Modules

| Module | What it gives you |
|---|---|
| `render` | `render` (markdown to an HTML fragment), `Resolver` (your URL scheme), `front_matter_table`, `render_source` (highlighted raw view), `format_file_stats` |
| `theme` | `ThemeSearch` (find and resolve themes), `ResolvedTheme`, `ThemeRegistry` (switch theme and light/dark at runtime), the embedded themes |
| `css` | The stylesheets as `&'static str`: `GITHUB_MARKDOWN`, `THEME_OVERRIDES`, `PAGE`, `SYNTAX`, `ALERTS` |
| `highlight` | The syntect adapters behind `render` |

## Building a page

1. Put the rendered fragment inside an element with class `markdown-body`.
2. Include the stylesheets in the order listed in `css`, then the active theme's `css_vars`.
3. Set `data-theme="light"` or `"dark"` on the root element. For any theme other than `github`, also set `data-birta-theme="<name>"`.
4. Math and Mermaid are left for the browser: load KaTeX for `[data-math-style]` elements and Mermaid for `pre.mermaid`.

## Opt-in syntax

- **`wikilinks`**: `[[target]]` and `[[target|alias]]` become links. `Resolver::wikilink` turns the target into an `href`.
- **`front_matter`**: a leading `---` block is left out of the HTML and returned raw in `Rendered::front_matter`. `front_matter_table` turns that block into a GitHub-style table to place above the body.
- **Not supported yet**: `![[embed]]` and `#tags` render as plain text.
