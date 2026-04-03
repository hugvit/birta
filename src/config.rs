use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub port: Option<u16>,
    pub no_open: Option<bool>,
    pub css: Option<PathBuf>,
    pub reading_mode: Option<bool>,
    pub syntax_theme: Option<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_theme_config")]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub font: FontConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FontConfig {
    pub body: Option<String>,
    pub mono: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KeybindingsConfig {
    #[serde(default = "default_toggle_reading")]
    pub toggle_reading: String,
    #[serde(default = "default_exit_reading")]
    pub exit_reading: String,
    #[serde(default = "default_toggle_dark")]
    pub toggle_dark: String,
    #[serde(default = "default_focus_theme")]
    pub focus_theme: String,
}

fn default_toggle_reading() -> String {
    "r".to_string()
}
fn default_exit_reading() -> String {
    "Escape".to_string()
}
fn default_toggle_dark() -> String {
    "d".to_string()
}
fn default_focus_theme() -> String {
    "t".to_string()
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            toggle_reading: default_toggle_reading(),
            exit_reading: default_exit_reading(),
            toggle_dark: default_toggle_dark(),
            focus_theme: default_focus_theme(),
        }
    }
}

impl KeybindingsConfig {
    /// Serialize to JSON for injection into a `<script>` block in the viewer
    /// template. Safe because serde_json escapes special characters and the
    /// source data is local (config file / CLI args, not network input).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Apply CLI `--bind action=key` overrides. Unknown actions are warned about.
    pub fn apply_overrides(&mut self, overrides: &[String]) {
        for entry in overrides {
            let Some((action, key)) = entry.split_once('=') else {
                eprintln!("birta: warning: invalid --bind format '{entry}', expected ACTION=KEY");
                continue;
            };
            let key = if key == "none" {
                String::new()
            } else {
                key.to_string()
            };
            match action {
                "toggle_reading" => self.toggle_reading = key,
                "exit_reading" => self.exit_reading = key,
                "toggle_dark" => self.toggle_dark = key,
                "focus_theme" => self.focus_theme = key,
                _ => {
                    eprintln!("birta: warning: unknown keybinding action '{action}'");
                }
            }
        }
    }
}

impl FontConfig {
    /// Generate CSS overrides for custom fonts. Returns `None` if no fonts configured.
    pub fn to_css(&self) -> Option<String> {
        let mut rules = Vec::new();
        if let Some(body) = &self.body {
            rules.push(format!(
                ".markdown-body {{ font-family: {body} !important; }}\n\
                 .header {{ font-family: {body}; }}"
            ));
        }
        if let Some(mono) = &self.mono {
            rules.push(format!(
                ".markdown-body code, .markdown-body pre {{ font-family: {mono} !important; }}\n\
                 .file-header {{ font-family: {mono}; }}"
            ));
        }
        if rules.is_empty() {
            None
        } else {
            Some(rules.join("\n"))
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ThemeConfig {
    pub name: Option<String>,
    pub variant: Option<String>,
    #[serde(default)]
    pub controls: ThemeControls,
}

#[derive(Debug, Default, Deserialize)]
pub struct ThemeControls {
    #[serde(default)]
    pub show_controls: ControlFlags,
}

#[derive(Debug, Deserialize)]
pub struct ControlFlags {
    #[serde(default = "default_true")]
    pub theme_swap: bool,
    #[serde(default = "default_true")]
    pub theme_toggle: bool,
    #[serde(default = "default_true")]
    pub header: bool,
}

impl Default for ControlFlags {
    fn default() -> Self {
        Self {
            theme_swap: true,
            theme_toggle: true,
            header: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Support both old `theme = "name"` (string) and new `[theme]` (table) formats.
fn deserialize_theme_config<'de, D>(deserializer: D) -> Result<ThemeConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ThemeValue {
        Name(String),
        Config(ThemeConfig),
    }

    match ThemeValue::deserialize(deserializer) {
        Ok(ThemeValue::Name(name)) => {
            eprintln!(
                "birta: warning: `theme = \"...\"` is deprecated, use `[theme]` table with `name = \"...\"`"
            );
            Ok(ThemeConfig {
                name: Some(name),
                variant: None,
                controls: ThemeControls::default(),
            })
        }
        Ok(ThemeValue::Config(config)) => Ok(config),
        Err(_) => Err(de::Error::custom(
            "expected theme as string or [theme] table",
        )),
    }
}

/// Load config from `~/.config/birta/config.toml` if it exists.
pub fn load() -> Config {
    config_path()
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .and_then(|contents| match toml::from_str(&contents) {
            Ok(config) => Some(config),
            Err(e) => {
                eprintln!("birta: warning: failed to parse config: {e}");
                None
            }
        })
        .unwrap_or_default()
}

fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".config").join("birta").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_theme_config() {
        let toml_str = r#"
[theme]
name = "catppuccin"

[theme.controls]
show_controls = { theme_swap = true, theme_toggle = true }
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.theme.name.as_deref(), Some("catppuccin"));
        assert!(config.theme.controls.show_controls.theme_swap);
        assert!(config.theme.controls.show_controls.theme_toggle);
    }

    #[test]
    fn parse_old_string_theme_config() {
        let toml_str = r#"theme = "dracula""#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.theme.name.as_deref(), Some("dracula"));
    }

    #[test]
    fn parse_empty_config() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.theme.name.is_none());
        assert!(config.theme.controls.show_controls.theme_swap);
        assert!(config.theme.controls.show_controls.theme_toggle);
    }

    #[test]
    fn parse_font_config() {
        let toml_str = r#"
[font]
body = "Georgia, serif"
mono = "JetBrains Mono, monospace"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.font.body.as_deref(), Some("Georgia, serif"));
        assert_eq!(
            config.font.mono.as_deref(),
            Some("JetBrains Mono, monospace")
        );
        let css = config.font.to_css().unwrap();
        assert!(css.contains("Georgia, serif"));
        assert!(css.contains("JetBrains Mono, monospace"));
    }

    #[test]
    fn font_config_empty_returns_none() {
        let config = FontConfig::default();
        assert!(config.to_css().is_none());
    }

    #[test]
    fn parse_theme_name_only() {
        let toml_str = r#"
[theme]
name = "github"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.theme.name.as_deref(), Some("github"));
        assert!(config.theme.controls.show_controls.theme_swap);
        assert!(config.theme.controls.show_controls.theme_toggle);
    }

    #[test]
    fn parse_reading_mode() {
        let config: Config = toml::from_str("reading_mode = true").unwrap();
        assert_eq!(config.reading_mode, Some(true));
    }

    #[test]
    fn parse_reading_mode_default_none() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.reading_mode.is_none());
    }

    #[test]
    fn parse_syntax_theme() {
        let config: Config =
            toml::from_str(r#"syntax_theme = "/path/to/monokai.tmTheme""#).unwrap();
        assert_eq!(
            config.syntax_theme,
            Some(PathBuf::from("/path/to/monokai.tmTheme"))
        );
    }

    #[test]
    fn parse_theme_variant() {
        let toml_str = r#"
[theme]
name = "catppuccin"
variant = "dark"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.theme.variant.as_deref(), Some("dark"));
    }

    #[test]
    fn parse_theme_variant_default_none() {
        let toml_str = r#"
[theme]
name = "github"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.theme.variant.is_none());
    }

    #[test]
    fn keybindings_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.keybindings.toggle_reading, "r");
        assert_eq!(config.keybindings.exit_reading, "Escape");
        assert_eq!(config.keybindings.toggle_dark, "d");
        assert_eq!(config.keybindings.focus_theme, "t");
    }

    #[test]
    fn keybindings_partial_override() {
        let toml_str = r#"
[keybindings]
toggle_reading = "Alt+r"
toggle_dark = "Alt+d"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.keybindings.toggle_reading, "Alt+r");
        assert_eq!(config.keybindings.toggle_dark, "Alt+d");
        // Unset fields keep defaults
        assert_eq!(config.keybindings.exit_reading, "Escape");
        assert_eq!(config.keybindings.focus_theme, "t");
    }

    #[test]
    fn keybindings_disabled_binding() {
        let toml_str = r#"
[keybindings]
toggle_dark = ""
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.keybindings.toggle_dark, "");
    }

    #[test]
    fn keybindings_to_json() {
        let kb = KeybindingsConfig::default();
        let json = kb.to_json();
        assert!(json.contains("\"toggle_reading\":\"r\""));
        assert!(json.contains("\"exit_reading\":\"Escape\""));
    }

    #[test]
    fn keybindings_apply_overrides() {
        let mut kb = KeybindingsConfig::default();
        kb.apply_overrides(&[
            "toggle_reading=Alt+r".to_string(),
            "toggle_dark=Alt+d".to_string(),
        ]);
        assert_eq!(kb.toggle_reading, "Alt+r");
        assert_eq!(kb.toggle_dark, "Alt+d");
        // Unaffected bindings keep defaults
        assert_eq!(kb.exit_reading, "Escape");
    }

    #[test]
    fn keybindings_apply_overrides_none_disables() {
        let mut kb = KeybindingsConfig::default();
        kb.apply_overrides(&["toggle_dark=none".to_string()]);
        assert_eq!(kb.toggle_dark, "");
    }
}
