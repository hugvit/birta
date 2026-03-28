use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use syntect::highlighting::ThemeSet;

use crate::config::Config;

// ---------------------------------------------------------------------------
// Bundled theme data (embedded at compile time)
// ---------------------------------------------------------------------------

struct BundledTheme {
    name: &'static str,
    toml: &'static str,
    syntax_files: &'static [(&'static str, &'static [u8])],
}

static BUNDLED_THEMES: &[BundledTheme] = &[
    BundledTheme {
        name: "github",
        toml: include_str!("../assets/themes/github.toml"),
        syntax_files: &[],
    },
    BundledTheme {
        name: "catppuccin",
        toml: include_str!("../assets/themes/catppuccin.toml"),
        syntax_files: &[
            (
                "syntax/catppuccin-latte.tmTheme",
                include_bytes!("../assets/themes/syntax/catppuccin-latte.tmTheme"),
            ),
            (
                "syntax/catppuccin-mocha.tmTheme",
                include_bytes!("../assets/themes/syntax/catppuccin-mocha.tmTheme"),
            ),
        ],
    },
    BundledTheme {
        name: "dracula",
        toml: include_str!("../assets/themes/dracula.toml"),
        syntax_files: &[(
            "syntax/dracula.tmTheme",
            include_bytes!("../assets/themes/syntax/dracula.tmTheme"),
        )],
    },
];

// ---------------------------------------------------------------------------
// TOML theme format
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ThemeToml {
    theme: ThemeDef,
}

#[derive(Debug, Deserialize)]
struct ThemeDef {
    name: String,
    light: Option<VariantDef>,
    dark: Option<VariantDef>,
}

#[derive(Debug, Deserialize)]
pub struct VariantDef {
    pub fg_primary: Option<String>,
    pub fg_muted: Option<String>,
    pub fg_accent: Option<String>,
    pub bg_primary: Option<String>,
    pub bg_muted: Option<String>,
    pub bg_neutral_muted: Option<String>,
    pub bg_attention_muted: Option<String>,
    pub bg_secondary: Option<String>,
    pub border_primary: Option<String>,
    pub border_muted: Option<String>,
    pub fg_sheen: Option<String>,
    pub bg_sheen: Option<String>,
    pub border_sheen: Option<String>,
    pub alert_note: Option<String>,
    pub alert_tip: Option<String>,
    pub alert_important: Option<String>,
    pub alert_warning: Option<String>,
    pub alert_caution: Option<String>,
    pub alert_note_bg: Option<String>,
    pub alert_tip_bg: Option<String>,
    pub alert_important_bg: Option<String>,
    pub alert_warning_bg: Option<String>,
    pub alert_caution_bg: Option<String>,
    pub syntax: Option<SyntaxRef>,
}

#[derive(Debug, Deserialize)]
pub struct SyntaxRef {
    pub filepath: PathBuf,
}

/// CSS variable mapping: (toml_field, css_variable, warn_if_missing)
const CSS_VAR_MAP: &[(&str, &str, bool)] = &[
    ("fg_primary", "--fgColor-default", true),
    ("fg_muted", "--fgColor-muted", true),
    ("fg_accent", "--fgColor-accent", true),
    ("bg_primary", "--bgColor-default", true),
    ("bg_muted", "--bgColor-muted", true),
    ("bg_neutral_muted", "--bgColor-neutral-muted", true),
    ("bg_attention_muted", "--bgColor-attention-muted", true),
    ("bg_secondary", "--sheen-bg-secondary", true),
    ("border_primary", "--borderColor-default", true),
    ("border_muted", "--borderColor-muted", true),
    ("fg_sheen", "--sheen-fg", true),
    ("bg_sheen", "--sheen-bg", true),
    ("border_sheen", "--sheen-border", true),
    ("alert_note", "--alert-note", true),
    ("alert_tip", "--alert-tip", true),
    ("alert_important", "--alert-important", true),
    ("alert_warning", "--alert-warning", true),
    ("alert_caution", "--alert-caution", true),
    ("alert_note_bg", "--alert-note-bg", false),
    ("alert_tip_bg", "--alert-tip-bg", false),
    ("alert_important_bg", "--alert-important-bg", false),
    ("alert_warning_bg", "--alert-warning-bg", false),
    ("alert_caution_bg", "--alert-caution-bg", false),
];

fn variant_field<'a>(variant: &'a VariantDef, field: &str) -> Option<&'a str> {
    match field {
        "fg_primary" => variant.fg_primary.as_deref(),
        "fg_muted" => variant.fg_muted.as_deref(),
        "fg_accent" => variant.fg_accent.as_deref(),
        "bg_primary" => variant.bg_primary.as_deref(),
        "bg_muted" => variant.bg_muted.as_deref(),
        "bg_neutral_muted" => variant.bg_neutral_muted.as_deref(),
        "bg_attention_muted" => variant.bg_attention_muted.as_deref(),
        "bg_secondary" => variant.bg_secondary.as_deref(),
        "border_primary" => variant.border_primary.as_deref(),
        "border_muted" => variant.border_muted.as_deref(),
        "fg_sheen" => variant.fg_sheen.as_deref(),
        "bg_sheen" => variant.bg_sheen.as_deref(),
        "border_sheen" => variant.border_sheen.as_deref(),
        "alert_note" => variant.alert_note.as_deref(),
        "alert_tip" => variant.alert_tip.as_deref(),
        "alert_important" => variant.alert_important.as_deref(),
        "alert_warning" => variant.alert_warning.as_deref(),
        "alert_caution" => variant.alert_caution.as_deref(),
        "alert_note_bg" => variant.alert_note_bg.as_deref(),
        "alert_tip_bg" => variant.alert_tip_bg.as_deref(),
        "alert_important_bg" => variant.alert_important_bg.as_deref(),
        "alert_warning_bg" => variant.alert_warning_bg.as_deref(),
        "alert_caution_bg" => variant.alert_caution_bg.as_deref(),
        _ => None,
    }
}

/// Generate CSS variable declarations from a variant definition.
pub fn variant_to_css(variant: &VariantDef, theme_name: &str) -> String {
    let mut decls = Vec::new();
    for &(field, css_var, warn) in CSS_VAR_MAP {
        match variant_field(variant, field) {
            Some(value) => decls.push(format!("  {css_var}: {value};")),
            None if warn => {
                eprintln!("sheen: warning: theme '{theme_name}' missing '{field}'");
            }
            None => {}
        }
    }
    // Apply to :root, .markdown-body, and both data-theme selectors
    // so theme colors always take effect regardless of toggle state
    let block = decls.join("\n");
    format!(
        ":root,\n.markdown-body,\n[data-theme=\"dark\"],\n[data-theme=\"light\"] {{\n{block}\n}}"
    )
}

// ---------------------------------------------------------------------------
// Resolved theme types
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SyntaxTheme {
    pub theme: syntect::highlighting::Theme,
    pub theme_name: String,
}

pub struct VariantData {
    pub css_vars: String,
    pub syntax: Option<SyntaxTheme>,
}

pub enum ThemeVariants {
    Both {
        light: Box<VariantData>,
        dark: Box<VariantData>,
    },
    Single(Box<VariantData>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Light,
    Dark,
}

impl Variant {
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Light => "light",
            Variant::Dark => "dark",
        }
    }

    pub fn parse(s: &str) -> Option<Variant> {
        match s {
            "light" => Some(Variant::Light),
            "dark" => Some(Variant::Dark),
            _ => None,
        }
    }
}

pub struct ResolvedTheme {
    pub name: String,
    pub variants: ThemeVariants,
    pub active_variant: Variant,
}

impl ResolvedTheme {
    pub fn active_data(&self) -> &VariantData {
        match &self.variants {
            ThemeVariants::Both { light, dark } => match self.active_variant {
                Variant::Light => light,
                Variant::Dark => dark,
            },
            ThemeVariants::Single(data) => data,
        }
    }

    pub fn has_toggle(&self) -> bool {
        matches!(&self.variants, ThemeVariants::Both { .. })
    }

    pub fn variant_names(&self) -> Vec<&'static str> {
        match &self.variants {
            ThemeVariants::Both { .. } => vec!["light", "dark"],
            ThemeVariants::Single(_) => vec![self.active_variant.as_str()],
        }
    }

    pub fn is_github(&self) -> bool {
        self.name == "github"
    }
}

// ---------------------------------------------------------------------------
// Directory helpers
// ---------------------------------------------------------------------------

/// User config themes: `~/.config/sheen/themes/`
fn config_themes_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".config").join("sheen").join("themes"))
}

/// Bundled/installed themes: `~/.local/share/sheen/themes/`
fn data_themes_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".local").join("share").join("sheen").join("themes"))
}

// ---------------------------------------------------------------------------
// Auto-install bundled themes
// ---------------------------------------------------------------------------

/// Write bundled themes to `~/.local/share/sheen/themes/`.
/// Always overwrites bundled files to keep them in sync with the binary.
pub fn ensure_bundled_themes() {
    let Some(dir) = data_themes_dir() else {
        return;
    };
    if let Err(e) = write_bundled_themes(&dir) {
        eprintln!("sheen: warning: failed to install bundled themes: {e}");
    }
}

fn write_bundled_themes(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir.join("syntax"))?;
    for bundled in BUNDLED_THEMES {
        let toml_path = dir.join(format!("{}.toml", bundled.name));
        std::fs::write(&toml_path, bundled.toml)?;
        for &(rel_path, data) in bundled.syntax_files {
            let dest = dir.join(rel_path);
            std::fs::write(&dest, data)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Theme discovery
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ThemeSource {
    User,
    Bundled,
}

#[derive(Debug)]
pub struct ThemeEntry {
    pub name: String,
    pub source: ThemeSource,
}

/// List all discovered theme names from config dir and data dir.
pub fn list_installed() -> Vec<ThemeEntry> {
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if let Some(dir) = config_themes_dir() {
        collect_toml_themes(&dir, ThemeSource::User, &mut entries, &mut seen);
    }
    if let Some(dir) = data_themes_dir() {
        collect_toml_themes(&dir, ThemeSource::Bundled, &mut entries, &mut seen);
    }

    // Also include bundled themes not yet on disk
    for bundled in BUNDLED_THEMES {
        if seen.insert(bundled.name.to_string()) {
            entries.push(ThemeEntry {
                name: bundled.name.to_string(),
                source: ThemeSource::Bundled,
            });
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

fn collect_toml_themes(
    dir: &Path,
    source: ThemeSource,
    entries: &mut Vec<ThemeEntry>,
    seen: &mut std::collections::HashSet<String>,
) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if seen.insert(stem.to_string()) {
                    entries.push(ThemeEntry {
                        name: stem.to_string(),
                        source: match &source {
                            ThemeSource::User => ThemeSource::User,
                            ThemeSource::Bundled => ThemeSource::Bundled,
                        },
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Three-tier theme resolution
// ---------------------------------------------------------------------------

/// Resolve a theme name to a parsed ThemeToml + base directory for relative paths.
fn find_theme_toml(name: &str) -> anyhow::Result<(ThemeToml, Option<PathBuf>)> {
    // 1. User config dir
    if let Some(dir) = config_themes_dir() {
        let path = dir.join(format!("{name}.toml"));
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let theme_toml: ThemeToml = toml::from_str(&contents)
                .map_err(|e| anyhow::anyhow!("failed to parse theme '{name}': {e}"))?;
            return Ok((theme_toml, Some(dir)));
        }
    }

    // 2. Absolute path (or relative to cwd)
    let as_path = Path::new(name);
    if as_path.is_absolute() || as_path.exists() {
        // Could be path to .toml file or directory containing one
        let toml_path = if as_path.extension().is_some_and(|e| e == "toml") {
            as_path.to_path_buf()
        } else {
            // Try <path>.toml
            as_path.with_extension("toml")
        };
        if toml_path.exists() {
            let contents = std::fs::read_to_string(&toml_path)?;
            let theme_toml: ThemeToml = toml::from_str(&contents).map_err(|e| {
                anyhow::anyhow!("failed to parse theme '{}': {e}", toml_path.display())
            })?;
            let base = toml_path.parent().map(Path::to_path_buf);
            return Ok((theme_toml, base));
        }
    }

    // 3. Data dir (bundled)
    if let Some(dir) = data_themes_dir() {
        let path = dir.join(format!("{name}.toml"));
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let theme_toml: ThemeToml = toml::from_str(&contents)
                .map_err(|e| anyhow::anyhow!("failed to parse theme '{name}': {e}"))?;
            return Ok((theme_toml, Some(dir)));
        }
    }

    // 4. Embedded fallback
    for bundled in BUNDLED_THEMES {
        if bundled.name == name {
            let theme_toml: ThemeToml = toml::from_str(bundled.toml)
                .map_err(|e| anyhow::anyhow!("failed to parse bundled theme '{name}': {e}"))?;
            return Ok((theme_toml, None));
        }
    }

    let installed = list_installed();
    if installed.is_empty() {
        anyhow::bail!("theme '{name}' not found. No themes installed.");
    }
    let names: Vec<&str> = installed.iter().map(|e| e.name.as_str()).collect();
    anyhow::bail!("theme '{name}' not found. Available: {}", names.join(", "));
}

fn load_tmtheme(path: &Path) -> anyhow::Result<SyntaxTheme> {
    let theme = ThemeSet::get_theme(path)
        .map_err(|e| anyhow::anyhow!("failed to load tmTheme '{}': {e}", path.display()))?;
    let name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "custom".to_string());
    Ok(SyntaxTheme {
        theme,
        theme_name: name,
    })
}

/// Load a tmTheme from a bundled theme's embedded data.
fn load_bundled_tmtheme(theme_name: &str, rel_path: &str) -> anyhow::Result<SyntaxTheme> {
    // Normalize: strip leading "./" for comparison
    let normalized = rel_path.strip_prefix("./").unwrap_or(rel_path);
    for bundled in BUNDLED_THEMES {
        if bundled.name == theme_name {
            for &(path, data) in bundled.syntax_files {
                if path == normalized {
                    let cursor = std::io::Cursor::new(data);
                    let theme = ThemeSet::load_from_reader(&mut std::io::BufReader::new(cursor))
                        .map_err(|e| {
                            anyhow::anyhow!("failed to load bundled tmTheme '{rel_path}': {e}")
                        })?;
                    let name = Path::new(rel_path)
                        .file_stem()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "bundled".to_string());
                    return Ok(SyntaxTheme {
                        theme,
                        theme_name: name,
                    });
                }
            }
        }
    }
    anyhow::bail!("bundled tmTheme '{rel_path}' not found for theme '{theme_name}'");
}

fn resolve_variant(
    variant_def: &VariantDef,
    theme_name: &str,
    base_dir: &Option<PathBuf>,
) -> anyhow::Result<VariantData> {
    let css_vars = variant_to_css(variant_def, theme_name);
    let syntax = match &variant_def.syntax {
        Some(syntax_ref) => {
            let rel = &syntax_ref.filepath;
            // Try filesystem first, fall back to embedded
            if let Some(base) = base_dir {
                let full_path = base.join(rel);
                if full_path.exists() {
                    Some(load_tmtheme(&full_path)?)
                } else {
                    Some(load_bundled_tmtheme(theme_name, &rel.to_string_lossy())?)
                }
            } else {
                Some(load_bundled_tmtheme(theme_name, &rel.to_string_lossy())?)
            }
        }
        None => None,
    };
    Ok(VariantData { css_vars, syntax })
}

fn build_resolved_theme(
    theme_toml: ThemeToml,
    base_dir: Option<PathBuf>,
) -> anyhow::Result<ResolvedTheme> {
    let name = theme_toml.theme.name;
    match (theme_toml.theme.light, theme_toml.theme.dark) {
        (Some(light_def), Some(dark_def)) => {
            let light = Box::new(resolve_variant(&light_def, &name, &base_dir)?);
            let dark = Box::new(resolve_variant(&dark_def, &name, &base_dir)?);
            Ok(ResolvedTheme {
                name,
                variants: ThemeVariants::Both { light, dark },
                active_variant: Variant::Dark,
            })
        }
        (Some(light_def), None) => {
            let data = Box::new(resolve_variant(&light_def, &name, &base_dir)?);
            Ok(ResolvedTheme {
                name,
                variants: ThemeVariants::Single(data),
                active_variant: Variant::Light,
            })
        }
        (None, Some(dark_def)) => {
            let data = Box::new(resolve_variant(&dark_def, &name, &base_dir)?);
            Ok(ResolvedTheme {
                name,
                variants: ThemeVariants::Single(data),
                active_variant: Variant::Dark,
            })
        }
        (None, None) => {
            anyhow::bail!("theme '{name}' has no light or dark variant defined");
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

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

    let (theme_toml, base_dir) = find_theme_toml(&theme_name)?;
    let mut resolved = build_resolved_theme(theme_toml, base_dir)?;

    // CLI --syntax-theme overrides the active variant's syntax
    if let Some(syntax_path) = cli_syntax {
        let syntax = load_tmtheme(syntax_path)?;
        match &mut resolved.variants {
            ThemeVariants::Both { light, dark } => match resolved.active_variant {
                Variant::Light => light.syntax = Some(syntax),
                Variant::Dark => dark.syntax = Some(syntax),
            },
            ThemeVariants::Single(data) => data.syntax = Some(syntax),
        }
    }

    Ok(resolved)
}

/// Resolve a single theme by name (for the registry).
pub fn resolve_by_name(name: &str) -> anyhow::Result<ResolvedTheme> {
    let (theme_toml, base_dir) = find_theme_toml(name)?;
    build_resolved_theme(theme_toml, base_dir)
}

// ---------------------------------------------------------------------------
// Theme registry (for hot-swap)
// ---------------------------------------------------------------------------

pub struct ThemeRegistry {
    themes: HashMap<String, ResolvedTheme>,
    active: String,
    preferred_variant: Variant,
}

impl ThemeRegistry {
    pub fn new(initial: ResolvedTheme) -> Self {
        let active = initial.name.clone();
        let preferred_variant = initial.active_variant;
        let mut themes = HashMap::new();
        themes.insert(initial.name.clone(), initial);
        Self {
            themes,
            active,
            preferred_variant,
        }
    }

    /// Discover and load all themes from config + data dirs.
    pub fn discover_all(&mut self) {
        for entry in list_installed() {
            if self.themes.contains_key(&entry.name) {
                continue;
            }
            match resolve_by_name(&entry.name) {
                Ok(theme) => {
                    self.themes.insert(entry.name, theme);
                }
                Err(e) => {
                    eprintln!("sheen: warning: failed to load theme '{}': {e}", entry.name);
                }
            }
        }
    }

    pub fn active(&self) -> &ResolvedTheme {
        &self.themes[&self.active]
    }

    pub fn active_mut(&mut self) -> &mut ResolvedTheme {
        self.themes
            .get_mut(&self.active)
            .expect("active theme missing")
    }

    pub fn set_active(&mut self, name: &str) -> anyhow::Result<()> {
        let theme = self
            .themes
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("theme '{name}' not found in registry"))?;
        // Apply preferred variant if the theme supports it
        if theme.has_toggle() {
            theme.active_variant = self.preferred_variant;
        }
        self.active = name.to_string();
        Ok(())
    }

    pub fn set_variant(&mut self, variant: Variant) {
        self.preferred_variant = variant;
        let theme = self.active_mut();
        if theme.has_toggle() {
            theme.active_variant = variant;
        }
    }

    pub fn theme_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.themes.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_toml() {
        let toml_str = include_str!("../assets/themes/github.toml");
        let theme: ThemeToml = toml::from_str(toml_str).unwrap();
        assert_eq!(theme.theme.name, "github");
        assert!(theme.theme.light.is_some());
        assert!(theme.theme.dark.is_some());
        assert!(theme.theme.light.unwrap().syntax.is_none());
    }

    #[test]
    fn parse_catppuccin_toml() {
        let toml_str = include_str!("../assets/themes/catppuccin.toml");
        let theme: ThemeToml = toml::from_str(toml_str).unwrap();
        assert_eq!(theme.theme.name, "catppuccin");
        assert!(theme.theme.light.is_some());
        assert!(theme.theme.dark.is_some());
        let light = theme.theme.light.unwrap();
        assert!(light.syntax.is_some());
        assert_eq!(
            light.syntax.unwrap().filepath.to_str().unwrap(),
            "./syntax/catppuccin-latte.tmTheme"
        );
    }

    #[test]
    fn parse_dracula_toml() {
        let toml_str = include_str!("../assets/themes/dracula.toml");
        let theme: ThemeToml = toml::from_str(toml_str).unwrap();
        assert_eq!(theme.theme.name, "dracula");
        assert!(theme.theme.light.is_none());
        assert!(theme.theme.dark.is_some());
    }

    #[test]
    fn variant_to_css_generates_all_vars() {
        let variant = VariantDef {
            fg_primary: Some("#111".to_string()),
            fg_muted: Some("#222".to_string()),
            fg_accent: Some("#333".to_string()),
            bg_primary: Some("#444".to_string()),
            bg_muted: Some("#555".to_string()),
            bg_neutral_muted: Some("#666".to_string()),
            bg_attention_muted: Some("#777".to_string()),
            bg_secondary: Some("#888".to_string()),
            border_primary: Some("#999".to_string()),
            border_muted: Some("#aaa".to_string()),
            fg_sheen: Some("#bbb".to_string()),
            bg_sheen: Some("#ccc".to_string()),
            border_sheen: Some("#ddd".to_string()),
            alert_note: Some("#e01".to_string()),
            alert_tip: Some("#e02".to_string()),
            alert_important: Some("#e03".to_string()),
            alert_warning: Some("#e04".to_string()),
            alert_caution: Some("#e05".to_string()),
            alert_note_bg: Some("#e0133".to_string()),
            alert_tip_bg: Some("#e0233".to_string()),
            alert_important_bg: Some("#e0333".to_string()),
            alert_warning_bg: Some("#e0433".to_string()),
            alert_caution_bg: Some("#e0533".to_string()),
            syntax: None,
        };
        let css = variant_to_css(&variant, "test");
        assert!(css.contains("--fgColor-default: #111;"));
        assert!(css.contains("--bgColor-default: #444;"));
        assert!(css.contains("--sheen-fg: #bbb;"));
        assert!(css.contains("--alert-note: #e01;"));
        assert!(css.contains("--alert-note-bg: #e0133;"));
        assert!(css.contains(":root,"));
        assert!(css.contains(".markdown-body,"));
    }

    #[test]
    fn variant_to_css_warns_on_missing_fields() {
        let variant = VariantDef {
            fg_primary: Some("#111".to_string()),
            fg_muted: None,
            fg_accent: None,
            bg_primary: None,
            bg_muted: None,
            bg_neutral_muted: None,
            bg_attention_muted: None,
            bg_secondary: None,
            border_primary: None,
            border_muted: None,
            fg_sheen: None,
            bg_sheen: None,
            border_sheen: None,
            alert_note: None,
            alert_tip: None,
            alert_important: None,
            alert_warning: None,
            alert_caution: None,
            alert_note_bg: None,
            alert_tip_bg: None,
            alert_important_bg: None,
            alert_warning_bg: None,
            alert_caution_bg: None,
            syntax: None,
        };
        let css = variant_to_css(&variant, "partial");
        assert!(css.contains("--fgColor-default: #111;"));
        // Missing fields should not appear
        assert!(!css.contains("--fgColor-muted"));
    }

    #[test]
    fn resolved_theme_github_has_toggle() {
        let toml_str = include_str!("../assets/themes/github.toml");
        let theme_toml: ThemeToml = toml::from_str(toml_str).unwrap();
        let resolved = build_resolved_theme(theme_toml, None).unwrap();
        assert!(resolved.has_toggle());
        assert_eq!(resolved.variant_names(), vec!["light", "dark"]);
        assert!(resolved.is_github());
    }

    #[test]
    fn resolved_theme_dracula_no_toggle() {
        let toml_str = include_str!("../assets/themes/dracula.toml");
        let theme_toml: ThemeToml = toml::from_str(toml_str).unwrap();
        let resolved = build_resolved_theme(theme_toml, None).unwrap();
        assert!(!resolved.has_toggle());
        assert_eq!(resolved.variant_names(), vec!["dark"]);
        assert_eq!(resolved.active_variant, Variant::Dark);
    }

    #[test]
    fn theme_registry_basics() {
        let toml_str = include_str!("../assets/themes/github.toml");
        let theme_toml: ThemeToml = toml::from_str(toml_str).unwrap();
        let resolved = build_resolved_theme(theme_toml, None).unwrap();

        let mut registry = ThemeRegistry::new(resolved);
        assert_eq!(registry.active().name, "github");
        assert_eq!(registry.theme_names(), vec!["github"]);

        // Add another theme
        let dracula_toml: ThemeToml =
            toml::from_str(include_str!("../assets/themes/dracula.toml")).unwrap();
        let dracula = build_resolved_theme(dracula_toml, None).unwrap();
        registry.themes.insert("dracula".to_string(), dracula);

        registry.set_active("dracula").unwrap();
        assert_eq!(registry.active().name, "dracula");
    }
}
