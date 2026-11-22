//! Theme lookup for the CLI: `birta_core::theme` plus the user's theme directories.

use std::path::{Path, PathBuf};

pub use birta_core::theme::*;

use crate::config::Config;

/// User config themes: `~/.config/birta/themes/`
fn config_themes_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".config").join("birta").join("themes"))
}

/// Bundled/installed themes: `~/.local/share/birta/themes/`
fn data_themes_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".local").join("share").join("birta").join("themes"))
}

/// Where the CLI looks for themes on this machine.
pub fn search() -> ThemeSearch {
    ThemeSearch {
        user_dir: config_themes_dir(),
        data_dir: data_themes_dir(),
    }
}

/// Write bundled themes to `~/.local/share/birta/themes/`.
/// Always overwrites bundled files to keep them in sync with the binary.
pub fn ensure_bundled_themes() {
    let Some(dir) = data_themes_dir() else {
        return;
    };
    if let Err(e) = install_bundled_themes(&dir) {
        eprintln!("birta: warning: failed to install bundled themes: {e}");
    }
}

/// List all discovered theme names from config dir and data dir.
pub fn list_installed() -> Vec<ThemeEntry> {
    search().list()
}

/// Resolve the active theme from CLI and config settings.
pub fn resolve(
    config: &Config,
    cli_theme: Option<&str>,
    cli_syntax: Option<&Path>,
) -> anyhow::Result<ResolvedTheme> {
    let theme_name = cli_theme
        .map(String::from)
        .or_else(|| config.theme.name.clone())
        .unwrap_or_else(|| "github".to_string());

    let mut resolved = search().resolve(&theme_name)?;

    // CLI --syntax-theme overrides the active variant's syntax
    if let Some(syntax_path) = cli_syntax {
        resolved.set_syntax_theme(load_syntax_theme(syntax_path)?);
    }

    Ok(resolved)
}

/// Resolve a single theme by name (for the registry).
pub fn resolve_by_name(name: &str) -> anyhow::Result<ResolvedTheme> {
    search().resolve(name)
}
