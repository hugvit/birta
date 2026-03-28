use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub port: Option<u16>,
    pub no_open: Option<bool>,
    pub css: Option<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_theme_config")]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub font: FontConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FontConfig {
    pub body: Option<String>,
    pub mono: Option<String>,
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
    #[serde(default)]
    pub theme_swap: bool,
    #[serde(default = "default_true")]
    pub theme_toggle: bool,
    #[serde(default = "default_true")]
    pub header: bool,
}

impl Default for ControlFlags {
    fn default() -> Self {
        Self {
            theme_swap: false,
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
                "sheen: warning: `theme = \"...\"` is deprecated, use `[theme]` table with `name = \"...\"`"
            );
            Ok(ThemeConfig {
                name: Some(name),
                controls: ThemeControls::default(),
            })
        }
        Ok(ThemeValue::Config(config)) => Ok(config),
        Err(_) => Err(de::Error::custom(
            "expected theme as string or [theme] table",
        )),
    }
}

/// Load config from `~/.config/sheen/config.toml` if it exists.
pub fn load() -> Config {
    config_path()
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .and_then(|contents| match toml::from_str(&contents) {
            Ok(config) => Some(config),
            Err(e) => {
                eprintln!("sheen: warning: failed to parse config: {e}");
                None
            }
        })
        .unwrap_or_default()
}

fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".config").join("sheen").join("config.toml"))
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
        assert!(!config.theme.controls.show_controls.theme_swap);
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
        assert!(!config.theme.controls.show_controls.theme_swap);
        assert!(config.theme.controls.show_controls.theme_toggle);
    }
}
