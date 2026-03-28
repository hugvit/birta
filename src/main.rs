use std::io::Read;
use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    version,
    about = "Preview markdown files in the browser with GitHub-style rendering"
)]
struct Cli {
    /// Path to the markdown file to preview, or "-" for stdin
    file: Option<PathBuf>,

    /// Port to serve on (0 = auto-assign)
    #[arg(short, long)]
    port: Option<u16>,

    /// Don't open the browser automatically
    #[arg(long)]
    no_open: bool,

    /// Custom CSS file to inject after default styles
    #[arg(long)]
    css: Option<PathBuf>,

    /// Theme preset name or path to theme file
    #[arg(long)]
    theme: Option<String>,

    /// Path to a .tmTheme file for syntax highlighting (overrides preset)
    #[arg(long)]
    syntax_theme: Option<PathBuf>,

    /// List available theme presets and exit
    #[arg(long)]
    list_themes: bool,

    /// Enable runtime theme switching dropdown
    #[arg(long)]
    theme_swap: bool,

    /// Disable the light/dark variant toggle
    #[arg(long)]
    no_toggle: bool,

    /// Override body font family (e.g. "Georgia, serif")
    #[arg(long)]
    font_body: Option<String>,

    /// Override monospace font family (e.g. "JetBrains Mono, monospace")
    #[arg(long)]
    font_mono: Option<String>,

    /// Hide the header bar
    #[arg(long)]
    no_header: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Auto-install bundled themes to ~/.local/share/sheen/themes/ on first run
    sheen::theme::ensure_bundled_themes();

    if cli.list_themes {
        let entries = sheen::theme::list_installed();
        if entries.is_empty() {
            eprintln!("no themes found");
        } else {
            let max_name = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
            for entry in &entries {
                let source = match entry.source {
                    sheen::theme::ThemeSource::User => "user",
                    sheen::theme::ThemeSource::Bundled => "bundled",
                };
                println!("  {:<width$}  ({source})", entry.name, width = max_name);
            }
        }
        return Ok(());
    }

    let file = cli
        .file
        .ok_or_else(|| anyhow::anyhow!("missing required argument: FILE"))?;

    let config = sheen::config::load();

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

    let theme = sheen::theme::resolve(&config, cli.theme.as_deref(), cli.syntax_theme.as_deref())?;

    let enable_swap = cli.theme_swap || config.theme.controls.show_controls.theme_swap;
    let enable_toggle = !cli.no_toggle && config.theme.controls.show_controls.theme_toggle;
    let show_header = !cli.no_header && config.theme.controls.show_controls.header;

    let font_config = sheen::config::FontConfig {
        body: cli.font_body.or(config.font.body),
        mono: cli.font_mono.or(config.font.mono),
    };
    let font_css = font_config.to_css();

    if file.as_os_str() == "-" {
        let mut markdown = String::new();
        std::io::stdin().read_to_string(&mut markdown)?;
        let opts = sheen::server::ServerOptions {
            port,
            no_open,
            custom_css,
            font_css,
            theme,
            enable_swap,
            enable_toggle,
            show_header,
        };
        return sheen::server::run_stdin(&markdown, opts).await;
    }

    if !file.exists() {
        anyhow::bail!("file not found: {}", file.display());
    }

    if let Some(ext) = file.extension().and_then(|e| e.to_str()) {
        if ext != "md" && ext != "markdown" {
            eprintln!(
                "sheen: warning: {} does not have a .md or .markdown extension",
                file.display()
            );
        }
    }

    let opts = sheen::server::ServerOptions {
        port,
        no_open,
        custom_css,
        font_css,
        theme,
        enable_swap,
        enable_toggle,
        show_header,
    };
    sheen::server::run(file, opts).await
}
