use std::path::PathBuf;

use crate::config::Config;

/// CLI-provided options, decoupled from clap for testability.
#[derive(Default)]
pub struct CliOptions {
    pub port: Option<u16>,
    pub no_open: bool,
    pub css: Option<PathBuf>,
    pub theme: Option<String>,
    pub syntax_theme: Option<PathBuf>,
    pub light: bool,
    pub dark: bool,
    pub font_body: Option<String>,
    pub font_mono: Option<String>,
    pub reading_mode: bool,
    pub no_header: bool,
    pub no_theme_swap: bool,
    pub no_toggle: bool,
}

/// Result of merging CLI options with config file settings.
pub struct MergedOptions {
    pub port: u16,
    pub no_open: bool,
    pub css_path: Option<PathBuf>,
    pub theme_name: Option<String>,
    pub syntax_theme: Option<PathBuf>,
    pub light: bool,
    pub dark: bool,
    pub font_body: Option<String>,
    pub font_mono: Option<String>,
    pub reading_mode: bool,
    pub enable_swap: bool,
    pub enable_toggle: bool,
    pub show_header: bool,
}

/// Merge CLI options with config, applying priority rules:
/// CLI flags > config > defaults.
///
/// Boolean display flags (`no_header`, `no_theme_swap`, `no_toggle`) use AND
/// logic: the feature is enabled only if both CLI and config agree. A `no_*`
/// flag from CLI disables it; `show_controls.x = false` in config disables it.
///
/// `no_open` and `reading_mode` use OR logic: either source being true
/// enables the behavior.
///
/// Variant preference: CLI `--light`/`--dark` flags take priority over
/// `theme.variant` in config. When neither is set, the theme default applies.
pub fn merge(cli: CliOptions, config: &Config) -> MergedOptions {
    let port = cli.port.or(config.port).unwrap_or(0);
    let no_open = cli.no_open || config.no_open.unwrap_or(false);

    let css_path = cli.css.or(config.css.clone());

    let theme_name = cli.theme.or(config.theme.name.clone());
    let syntax_theme = cli.syntax_theme.or(config.syntax_theme.clone());

    let font_body = cli.font_body.or(config.font.body.clone());
    let font_mono = cli.font_mono.or(config.font.mono.clone());

    let enable_swap = !cli.no_theme_swap && config.theme.controls.show_controls.theme_swap;
    let enable_toggle = !cli.no_toggle && config.theme.controls.show_controls.theme_toggle;
    let show_header = !cli.no_header && config.theme.controls.show_controls.header;

    let reading_mode = cli.reading_mode || config.reading_mode.unwrap_or(false);

    // CLI --light/--dark override config variant preference
    let (light, dark) = if cli.light || cli.dark {
        (cli.light, cli.dark)
    } else {
        match config.theme.variant.as_deref() {
            Some("light") => (true, false),
            Some("dark") => (false, true),
            Some(other) => {
                eprintln!(
                    "birta: warning: unknown theme variant '{other}', expected 'light' or 'dark'"
                );
                (false, false)
            }
            None => (false, false),
        }
    };

    MergedOptions {
        port,
        no_open,
        css_path,
        theme_name,
        syntax_theme,
        light,
        dark,
        font_body,
        font_mono,
        reading_mode,
        enable_swap,
        enable_toggle,
        show_header,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ControlFlags, FontConfig, ThemeConfig, ThemeControls};

    fn default_config() -> Config {
        Config::default()
    }

    fn config_with_port(port: u16) -> Config {
        Config {
            port: Some(port),
            ..Default::default()
        }
    }

    #[test]
    fn merge_port_cli_wins() {
        let cli = CliOptions {
            port: Some(8080),
            ..Default::default()
        };
        let config = config_with_port(3000);
        let merged = merge(cli, &config);
        assert_eq!(merged.port, 8080);
    }

    #[test]
    fn merge_port_config_fallback() {
        let cli = CliOptions::default();
        let config = config_with_port(3000);
        let merged = merge(cli, &config);
        assert_eq!(merged.port, 3000);
    }

    #[test]
    fn merge_port_default_when_both_none() {
        let cli = CliOptions::default();
        let config = default_config();
        let merged = merge(cli, &config);
        assert_eq!(merged.port, 0);
    }

    #[test]
    fn merge_no_open_cli_true() {
        let cli = CliOptions {
            no_open: true,
            ..Default::default()
        };
        let merged = merge(cli, &default_config());
        assert!(merged.no_open);
    }

    #[test]
    fn merge_no_open_config_true() {
        let config = Config {
            no_open: Some(true),
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert!(merged.no_open);
    }

    #[test]
    fn merge_no_open_both_false() {
        let merged = merge(CliOptions::default(), &default_config());
        assert!(!merged.no_open);
    }

    #[test]
    fn merge_css_cli_wins() {
        let cli = CliOptions {
            css: Some(PathBuf::from("/cli/style.css")),
            ..Default::default()
        };
        let config = Config {
            css: Some(PathBuf::from("/config/style.css")),
            ..Default::default()
        };
        let merged = merge(cli, &config);
        assert_eq!(merged.css_path, Some(PathBuf::from("/cli/style.css")));
    }

    #[test]
    fn merge_css_config_fallback() {
        let config = Config {
            css: Some(PathBuf::from("/config/style.css")),
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert_eq!(merged.css_path, Some(PathBuf::from("/config/style.css")));
    }

    #[test]
    fn merge_font_cli_wins() {
        let cli = CliOptions {
            font_body: Some("Georgia".to_string()),
            font_mono: Some("Fira Code".to_string()),
            ..Default::default()
        };
        let config = Config {
            font: FontConfig {
                body: Some("Arial".to_string()),
                mono: Some("Courier".to_string()),
            },
            ..Default::default()
        };
        let merged = merge(cli, &config);
        assert_eq!(merged.font_body.as_deref(), Some("Georgia"));
        assert_eq!(merged.font_mono.as_deref(), Some("Fira Code"));
    }

    #[test]
    fn merge_font_config_fallback() {
        let config = Config {
            font: FontConfig {
                body: Some("Arial".to_string()),
                mono: None,
            },
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert_eq!(merged.font_body.as_deref(), Some("Arial"));
        assert!(merged.font_mono.is_none());
    }

    #[test]
    fn merge_controls_cli_no_header_disables() {
        let cli = CliOptions {
            no_header: true,
            ..Default::default()
        };
        // Config defaults have header=true
        let merged = merge(cli, &default_config());
        assert!(!merged.show_header);
    }

    #[test]
    fn merge_controls_config_disables_header() {
        let config = Config {
            theme: ThemeConfig {
                controls: ThemeControls {
                    show_controls: ControlFlags {
                        header: false,
                        ..Default::default()
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };
        // CLI doesn't pass --no-header, but config says header=false
        let merged = merge(CliOptions::default(), &config);
        assert!(!merged.show_header);
    }

    #[test]
    fn merge_controls_both_enabled() {
        let merged = merge(CliOptions::default(), &default_config());
        assert!(merged.show_header);
        assert!(merged.enable_swap);
        assert!(merged.enable_toggle);
    }

    #[test]
    fn merge_controls_cli_no_toggle_disables() {
        let cli = CliOptions {
            no_toggle: true,
            ..Default::default()
        };
        let merged = merge(cli, &default_config());
        assert!(!merged.enable_toggle);
    }

    #[test]
    fn merge_controls_cli_no_theme_swap_disables() {
        let cli = CliOptions {
            no_theme_swap: true,
            ..Default::default()
        };
        let merged = merge(cli, &default_config());
        assert!(!merged.enable_swap);
    }

    #[test]
    fn merge_reading_mode_cli_enables() {
        let cli = CliOptions {
            reading_mode: true,
            ..Default::default()
        };
        let merged = merge(cli, &default_config());
        assert!(merged.reading_mode);
    }

    #[test]
    fn merge_reading_mode_config_enables() {
        let config = Config {
            reading_mode: Some(true),
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert!(merged.reading_mode);
    }

    #[test]
    fn merge_reading_mode_default_false() {
        let merged = merge(CliOptions::default(), &default_config());
        assert!(!merged.reading_mode);
    }

    #[test]
    fn merge_theme_name_cli_wins() {
        let cli = CliOptions {
            theme: Some("dracula".to_string()),
            ..Default::default()
        };
        let config = Config {
            theme: ThemeConfig {
                name: Some("github".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge(cli, &config);
        assert_eq!(merged.theme_name.as_deref(), Some("dracula"));
    }

    #[test]
    fn merge_theme_name_config_fallback() {
        let config = Config {
            theme: ThemeConfig {
                name: Some("catppuccin".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert_eq!(merged.theme_name.as_deref(), Some("catppuccin"));
    }

    #[test]
    fn merge_variant_cli_light_wins() {
        let cli = CliOptions {
            light: true,
            ..Default::default()
        };
        let config = Config {
            theme: ThemeConfig {
                variant: Some("dark".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge(cli, &config);
        assert!(merged.light);
        assert!(!merged.dark);
    }

    #[test]
    fn merge_variant_config_dark() {
        let config = Config {
            theme: ThemeConfig {
                variant: Some("dark".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert!(!merged.light);
        assert!(merged.dark);
    }

    #[test]
    fn merge_variant_config_light() {
        let config = Config {
            theme: ThemeConfig {
                variant: Some("light".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert!(merged.light);
        assert!(!merged.dark);
    }

    #[test]
    fn merge_variant_neither_set() {
        let merged = merge(CliOptions::default(), &default_config());
        assert!(!merged.light);
        assert!(!merged.dark);
    }

    #[test]
    fn merge_no_open_both_true() {
        let cli = CliOptions {
            no_open: true,
            ..Default::default()
        };
        let config = Config {
            no_open: Some(true),
            ..Default::default()
        };
        let merged = merge(cli, &config);
        assert!(merged.no_open);
    }

    #[test]
    fn merge_syntax_theme_cli_wins() {
        let cli = CliOptions {
            syntax_theme: Some(PathBuf::from("/cli/theme.tmTheme")),
            ..Default::default()
        };
        let config = Config {
            syntax_theme: Some(PathBuf::from("/config/theme.tmTheme")),
            ..Default::default()
        };
        let merged = merge(cli, &config);
        assert_eq!(
            merged.syntax_theme,
            Some(PathBuf::from("/cli/theme.tmTheme"))
        );
    }

    #[test]
    fn merge_syntax_theme_config_fallback() {
        let config = Config {
            syntax_theme: Some(PathBuf::from("/config/theme.tmTheme")),
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert_eq!(
            merged.syntax_theme,
            Some(PathBuf::from("/config/theme.tmTheme"))
        );
    }

    #[test]
    fn merge_syntax_theme_none_by_default() {
        let merged = merge(CliOptions::default(), &default_config());
        assert!(merged.syntax_theme.is_none());
    }

    #[test]
    fn merge_variant_unknown_treated_as_neither() {
        let config = Config {
            theme: ThemeConfig {
                variant: Some("blue".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge(CliOptions::default(), &config);
        assert!(!merged.light);
        assert!(!merged.dark);
    }

    #[test]
    fn merge_variant_cli_dark_overrides_config_light() {
        let cli = CliOptions {
            dark: true,
            ..Default::default()
        };
        let config = Config {
            theme: ThemeConfig {
                variant: Some("light".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge(cli, &config);
        assert!(!merged.light);
        assert!(merged.dark);
    }

    #[test]
    fn merge_reading_mode_both_true() {
        let cli = CliOptions {
            reading_mode: true,
            ..Default::default()
        };
        let config = Config {
            reading_mode: Some(true),
            ..Default::default()
        };
        let merged = merge(cli, &config);
        assert!(merged.reading_mode);
    }
}
