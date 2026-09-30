use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub port: Option<u16>,
    pub no_open: Option<bool>,
    pub css: Option<PathBuf>,
    pub reading_mode: Option<bool>,
    pub raw_mode: Option<bool>,
    pub syntax_theme: Option<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_theme_config")]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub font: FontConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
    #[serde(default)]
    pub review: ReviewConfig,
}

/// The `[review]` table. Review stays off unless enabled here or by `BIRTA_REVIEW=1`.
#[derive(Clone, Debug, Deserialize)]
pub struct ReviewConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Add `*.birta.jsonl` to the repo's `info/exclude` when a review file is created.
    #[serde(default = "default_true")]
    pub git_exclude: bool,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            git_exclude: true,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FontConfig {
    pub body: Option<String>,
    pub mono: Option<String>,
}

/// Key per action. Letters match case-blind; `Mod` is Meta or Ctrl; "" leaves an action unbound.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    pub toggle_reading: String,
    pub exit_reading: String,
    pub toggle_dark: String,
    pub focus_theme: String,
    pub toggle_raw: String,
    pub focus_next: String,
    pub focus_prev: String,
    pub note: String,
    pub ask: String,
    pub yes: String,
    pub no: String,
    pub later: String,
    pub your_call: String,
    pub next_item: String,
    pub prev_item: String,
    pub next_file: String,
    pub prev_file: String,
    pub undo: String,
    pub hide_marks: String,
    pub open: String,
    pub turn: String,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        let key = |k: &str| k.to_string();
        Self {
            toggle_reading: key("r"),
            exit_reading: key("Escape"),
            toggle_dark: key("d"),
            focus_theme: key("t"),
            toggle_raw: key("u"),
            focus_next: key("j"),
            focus_prev: key("k"),
            note: key("c"),
            ask: key("q"),
            yes: key("y"),
            no: key("x"),
            later: key("l"),
            your_call: key("."),
            next_item: key("n"),
            prev_item: key("p"),
            next_file: key("Shift+J"),
            prev_file: key("Shift+K"),
            undo: key("z"),
            hide_marks: key("m"),
            open: key("Enter"),
            turn: key("Mod+Enter"),
        }
    }
}

/// The key presses a binding answers to, spelled one way: modifiers sorted, a letter in lower
/// case, and `Mod` as both Ctrl and Meta. Empty for an unbound action.
fn presses(binding: &str) -> Vec<String> {
    if binding.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<&str> = binding.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let key = if key.chars().count() == 1 {
        key.to_lowercase()
    } else {
        key.to_string()
    };
    let mut sets: Vec<Vec<&str>> = vec![Vec::new()];
    for part in parts {
        if part == "Mod" {
            sets = sets
                .into_iter()
                .flat_map(|set| {
                    let mut ctrl = set.clone();
                    ctrl.push("Ctrl");
                    let mut meta = set;
                    meta.push("Meta");
                    [ctrl, meta]
                })
                .collect();
        } else {
            for set in &mut sets {
                set.push(part);
            }
        }
    }
    sets.into_iter()
        .map(|mut set| {
            set.sort_unstable();
            set.push(&key);
            set.join("+")
        })
        .collect()
}

fn clash(a: &str, b: &str) -> bool {
    let b = presses(b);
    presses(a).iter().any(|p| b.contains(p))
}

impl KeybindingsConfig {
    fn slots(&self) -> [(&'static str, &String); 21] {
        [
            ("toggle_reading", &self.toggle_reading),
            ("exit_reading", &self.exit_reading),
            ("toggle_dark", &self.toggle_dark),
            ("focus_theme", &self.focus_theme),
            ("toggle_raw", &self.toggle_raw),
            ("focus_next", &self.focus_next),
            ("focus_prev", &self.focus_prev),
            ("note", &self.note),
            ("ask", &self.ask),
            ("yes", &self.yes),
            ("no", &self.no),
            ("later", &self.later),
            ("your_call", &self.your_call),
            ("next_item", &self.next_item),
            ("prev_item", &self.prev_item),
            ("next_file", &self.next_file),
            ("prev_file", &self.prev_file),
            ("undo", &self.undo),
            ("hide_marks", &self.hide_marks),
            ("open", &self.open),
            ("turn", &self.turn),
        ]
    }

    fn slot(&mut self, action: &str) -> Option<&mut String> {
        Some(match action {
            "toggle_reading" => &mut self.toggle_reading,
            "exit_reading" => &mut self.exit_reading,
            "toggle_dark" => &mut self.toggle_dark,
            "focus_theme" => &mut self.focus_theme,
            "toggle_raw" => &mut self.toggle_raw,
            "focus_next" => &mut self.focus_next,
            "focus_prev" => &mut self.focus_prev,
            "note" => &mut self.note,
            "ask" => &mut self.ask,
            "yes" => &mut self.yes,
            "no" => &mut self.no,
            "later" => &mut self.later,
            "your_call" => &mut self.your_call,
            "next_item" => &mut self.next_item,
            "prev_item" => &mut self.prev_item,
            "next_file" => &mut self.next_file,
            "prev_file" => &mut self.prev_file,
            "undo" => &mut self.undo,
            "hide_marks" => &mut self.hide_marks,
            "open" => &mut self.open,
            "turn" => &mut self.turn,
            _ => return None,
        })
    }

    /// The first two actions sharing a key press, if any.
    fn first_clash(&self) -> Option<(&'static str, &'static str, String)> {
        let slots = self.slots();
        for (i, (a, key)) in slots.iter().enumerate() {
            for (b, other) in &slots[i + 1..] {
                if clash(key, other) {
                    return Some((a, b, (*key).clone()));
                }
            }
        }
        None
    }

    /// Serialize to JSON for injection into a `<script>` block in the viewer
    /// template. Safe because serde_json escapes special characters and the
    /// source data is local (config file / CLI args, not network input).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Apply CLI `--bind action=key` overrides. Unknown actions are warned about.
    pub fn apply_overrides(&mut self, overrides: &[String]) {
        for (action, key) in parse_binds(overrides) {
            match self.slot(action) {
                Some(slot) => *slot = key,
                None => eprintln!("birta: warning: unknown keybinding action '{action}'"),
            }
        }
    }

    /// These bindings from the config file with the `--bind` flags over them. Two actions on
    /// one key within the config, or within the flags, is an error naming both. A flag that
    /// takes a key the config gave another action wins: that action falls back to its default
    /// key if it is free, else it is left unbound, with a warning.
    pub fn with_binds(&self, overrides: &[String]) -> Result<Self, String> {
        if let Some((a, b, key)) = self.first_clash() {
            return Err(format!(
                "keybindings: {a} and {b} are both on \"{key}\" in config.toml"
            ));
        }
        let binds: Vec<(&str, String)> = parse_binds(overrides)
            .into_iter()
            .filter(|(action, _)| {
                let known = KeybindingsConfig::default()
                    .slots()
                    .iter()
                    .any(|(a, _)| a == action);
                if !known {
                    eprintln!("birta: warning: unknown keybinding action '{action}'");
                }
                known
            })
            .collect();
        for (i, (a, key)) in binds.iter().enumerate() {
            if let Some((b, _)) = binds[i + 1..]
                .iter()
                .find(|(b, other)| b != a && clash(key, other))
            {
                return Err(format!(
                    "keybindings: --bind {a} and --bind {b} are both on \"{key}\""
                ));
            }
        }
        let mut out = self.clone();
        for (action, key) in &binds {
            if let Some(slot) = out.slot(action) {
                slot.clone_from(key);
            }
        }
        let bound = |name: &str| binds.iter().any(|(a, _)| *a == name);
        let defaults = KeybindingsConfig::default();
        for (action, key) in &binds {
            let losers: Vec<&'static str> = out
                .slots()
                .iter()
                .filter(|(other, held)| !bound(other) && clash(key, held))
                .map(|(other, _)| *other)
                .collect();
            for loser in losers {
                let fallback = defaults
                    .slots()
                    .iter()
                    .find(|(a, _)| *a == loser)
                    .map(|(_, k)| (*k).clone())
                    .unwrap_or_default();
                let free = out
                    .slots()
                    .iter()
                    .all(|(other, held)| *other == loser || !clash(&fallback, held));
                let now = if free { fallback } else { String::new() };
                let shown = if now.is_empty() {
                    "unbound".to_string()
                } else {
                    format!("on \"{now}\"")
                };
                eprintln!(
                    "birta: warning: --bind {action}={key} takes the key from {loser}, which is now {shown}"
                );
                if let Some(slot) = out.slot(loser) {
                    *slot = now;
                }
            }
        }
        Ok(out)
    }
}

/// `action=key` pairs, with `none` as the empty (unbound) key. Malformed entries are warned about.
fn parse_binds(overrides: &[String]) -> Vec<(&str, String)> {
    overrides
        .iter()
        .filter_map(|entry| {
            let Some((action, key)) = entry.split_once('=') else {
                eprintln!("birta: warning: invalid --bind format '{entry}', expected ACTION=KEY");
                return None;
            };
            let key = if key == "none" {
                String::new()
            } else {
                key.to_string()
            };
            Some((action, key))
        })
        .collect()
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
        assert_eq!(config.keybindings.toggle_raw, "u");
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
        assert!(json.contains("\"toggle_raw\":\"u\""));
    }

    #[test]
    fn keybindings_apply_overrides() {
        let mut kb = KeybindingsConfig::default();
        kb.apply_overrides(&[
            "toggle_reading=Alt+r".to_string(),
            "toggle_dark=Alt+d".to_string(),
            "toggle_raw=s".to_string(),
        ]);
        assert_eq!(kb.toggle_reading, "Alt+r");
        assert_eq!(kb.toggle_dark, "Alt+d");
        assert_eq!(kb.toggle_raw, "s");
        // Unaffected bindings keep defaults
        assert_eq!(kb.exit_reading, "Escape");
    }

    fn keys_json(kb: &KeybindingsConfig) -> serde_json::Value {
        serde_json::from_str(&kb.to_json()).unwrap()
    }

    #[test]
    fn keybindings_review_defaults() {
        let json = keys_json(&KeybindingsConfig::default());
        let want = [
            ("focus_next", "j"),
            ("focus_prev", "k"),
            ("note", "c"),
            ("ask", "q"),
            ("yes", "y"),
            ("no", "x"),
            ("later", "l"),
            ("your_call", "."),
            ("next_item", "n"),
            ("prev_item", "p"),
            ("next_file", "Shift+J"),
            ("prev_file", "Shift+K"),
            ("undo", "z"),
            ("hide_marks", "m"),
            ("open", "Enter"),
            ("turn", "Mod+Enter"),
        ];
        for (action, key) in want {
            assert_eq!(json[action], key, "{action} in {json}");
        }
        // An empty table keeps every default
        let config: Config = toml::from_str(
            "[keybindings]
",
        )
        .unwrap();
        assert_eq!(keys_json(&config.keybindings), json);
    }

    #[test]
    fn keybindings_review_keys_rebind_in_config() {
        let toml_str = r#"
[keybindings]
yes = "g"
turn = "Ctrl+Enter"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let json = keys_json(&config.keybindings);
        assert_eq!(json["yes"], "g");
        assert_eq!(json["turn"], "Ctrl+Enter");
        assert_eq!(json["no"], "x");
        assert_eq!(json["toggle_reading"], "r");
    }

    #[test]
    fn keybindings_apply_overrides_rebinds_review_keys() {
        let mut kb = KeybindingsConfig::default();
        kb.apply_overrides(&["yes=g".to_string(), "hide_marks=none".to_string()]);
        let json = keys_json(&kb);
        assert_eq!(json["yes"], "g");
        assert_eq!(json["hide_marks"], "");
        assert_eq!(json["focus_next"], "j");
    }

    #[test]
    fn keybindings_apply_overrides_none_disables() {
        let mut kb = KeybindingsConfig::default();
        kb.apply_overrides(&["toggle_dark=none".to_string()]);
        assert_eq!(kb.toggle_dark, "");
    }
}
