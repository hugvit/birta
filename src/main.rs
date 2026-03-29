use std::io::Read;
use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    version,
    about = "Preview markdown files in the browser with GitHub-style rendering",
    max_term_width = 98,
    after_long_help = "\x1b[1;4mExamples:\x1b[0m
  birta README.md                        Preview with live reload
  birta --theme catppuccin README.md     Use a specific theme
  birta --light --no-header README.md    Light mode, no chrome
  birta --reading-mode README.md         Distraction-free reading
  birta --list-themes                    Show available themes
  cat notes.md | birta -                 Preview from stdin

\x1b[1;4mConfig:\x1b[0m
  ~/.config/birta/config.toml            Persistent settings
  ~/.config/birta/themes/<name>.toml     Custom themes"
)]
struct Cli {
    /// Markdown file to preview, or "-" for stdin
    file: Option<PathBuf>,

    // -- Server ---------------------------------------------------------------
    /// Port to serve on [default: auto-assign]
    #[arg(short, long, help_heading = "Server")]
    port: Option<u16>,

    /// Don't open the browser automatically
    #[arg(long, help_heading = "Server")]
    no_open: bool,

    // -- Theme ----------------------------------------------------------------
    /// Theme name or path to .toml theme file
    #[arg(long, help_heading = "Theme")]
    theme: Option<String>,

    /// Path to a .tmTheme syntax highlighting file
    #[arg(long, help_heading = "Theme")]
    syntax_theme: Option<PathBuf>,

    /// Start in light mode
    #[arg(long, conflicts_with = "dark", help_heading = "Theme")]
    light: bool,

    /// Start in dark mode
    #[arg(long, conflicts_with = "light", help_heading = "Theme")]
    dark: bool,

    /// List installed themes and exit
    #[arg(long, help_heading = "Theme")]
    list_themes: bool,

    // -- Display --------------------------------------------------------------
    /// Custom CSS file to inject after default styles
    #[arg(long, help_heading = "Display")]
    css: Option<PathBuf>,

    /// Body font family (e.g. "Georgia, serif")
    #[arg(long, help_heading = "Display")]
    font_body: Option<String>,

    /// Monospace font family (e.g. "JetBrains Mono")
    #[arg(long, help_heading = "Display")]
    font_mono: Option<String>,

    /// Start in reading mode
    #[arg(long, help_heading = "Display")]
    reading_mode: bool,

    /// Hide the header bar
    #[arg(long, help_heading = "Display")]
    no_header: bool,

    /// Disable the theme switching dropdown
    #[arg(long, help_heading = "Display")]
    no_theme_swap: bool,

    /// Disable the light/dark toggle
    #[arg(long, help_heading = "Display")]
    no_toggle: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Auto-install bundled themes to ~/.local/share/birta/themes/ on first run
    birta::theme::ensure_bundled_themes();

    if cli.list_themes {
        let entries = birta::theme::list_installed();
        if entries.is_empty() {
            eprintln!("no themes found");
        } else {
            let max_name = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
            for entry in &entries {
                let source = match entry.source {
                    birta::theme::ThemeSource::User => "user",
                    birta::theme::ThemeSource::Bundled => "bundled",
                };
                println!("  {:<width$}  ({source})", entry.name, width = max_name);
            }
        }
        return Ok(());
    }

    let file = cli
        .file
        .ok_or_else(|| anyhow::anyhow!("missing required argument: FILE"))?;

    let config = birta::config::load();

    let port = cli.port.or(config.port).unwrap_or(0);
    let no_open = cli.no_open || config.no_open.unwrap_or(false);

    let css_path = cli.css.or(config.css.clone());
    let custom_css = match &css_path {
        Some(path) => {
            if !path.exists() {
                anyhow::bail!("CSS file not found: {}", path.display());
            }
            Some(std::fs::read_to_string(path)?)
        }
        None => None,
    };

    let mut theme =
        birta::theme::resolve(&config, cli.theme.as_deref(), cli.syntax_theme.as_deref())?;

    if cli.light {
        theme.active_variant = birta::theme::Variant::Light;
    } else if cli.dark {
        theme.active_variant = birta::theme::Variant::Dark;
    }

    let enable_swap = !cli.no_theme_swap && config.theme.controls.show_controls.theme_swap;
    let enable_toggle = !cli.no_toggle && config.theme.controls.show_controls.theme_toggle;
    let show_header = !cli.no_header && config.theme.controls.show_controls.header;

    let font_config = birta::config::FontConfig {
        body: cli.font_body.or(config.font.body),
        mono: cli.font_mono.or(config.font.mono),
    };
    let font_css = font_config.to_css();

    if file.as_os_str() == "-" {
        let mut markdown = String::new();
        std::io::stdin().read_to_string(&mut markdown)?;
        let opts = birta::server::ServerOptions {
            port,
            no_open,
            custom_css,
            font_css,
            theme,
            enable_swap,
            enable_toggle,
            show_header,
            reading_mode: cli.reading_mode,
        };
        return birta::server::run_stdin(&markdown, opts).await;
    }

    if !file.exists() {
        anyhow::bail!("file not found: {}", file.display());
    }

    if let Some(ext) = file.extension().and_then(|e| e.to_str()) {
        if ext != "md" && ext != "markdown" {
            eprintln!(
                "birta: warning: {} does not have a .md or .markdown extension",
                file.display()
            );
        }
    }

    let opts = birta::server::ServerOptions {
        port,
        no_open,
        custom_css,
        font_css,
        theme,
        enable_swap,
        enable_toggle,
        show_header,
        reading_mode: cli.reading_mode,
    };
    birta::server::run(file, opts).await
}
