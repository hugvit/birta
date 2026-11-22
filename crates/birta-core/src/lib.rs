//! The rendering core of [birta](https://github.com/hugvit/birta): markdown to
//! HTML, syntax highlighting, themes, and the stylesheets that go with them.
//!
//! It knows nothing about servers, routes or files being watched. A host
//! application renders with [`render::render`], maps URLs onto its own routes
//! through a [`render::Resolver`], and builds its page from [`css`] plus a
//! [`theme::ResolvedTheme`].
//!
//! ```
//! use birta_core::render::{RenderOptions, Resolver, render};
//! use birta_core::theme::ThemeSearch;
//!
//! struct Notes;
//!
//! impl Resolver for Notes {
//!     fn wikilink(&self, target: &str) -> String {
//!         format!("/note/{target}")
//!     }
//! }
//!
//! let theme = ThemeSearch::default().resolve("catppuccin")?;
//! let opts = RenderOptions {
//!     syntax_theme: theme.active_data().syntax.as_ref(),
//!     resolver: Some(&Notes),
//!     wikilinks: true,
//!     front_matter: true,
//!     ..Default::default()
//! };
//! let page = render("---\ntitle: Hi\n---\nSee [[ideas]].", &opts);
//!
//! assert!(page.html.contains(r#"href="/note/ideas""#));
//! assert!(page.front_matter.is_some());
//! # Ok::<(), anyhow::Error>(())
//! ```

pub mod css;
pub mod highlight;
mod obsidian;
pub mod render;
pub mod theme;
