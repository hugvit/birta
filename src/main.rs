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

    /// Render to a self-contained HTML file, open in browser, and exit
    #[arg(long = "static", help_heading = "Server")]
    static_mode: bool,

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

    /// Override a keybinding (e.g. toggle_reading=Alt+r)
    #[arg(long = "bind", value_name = "ACTION=KEY", help_heading = "Display")]
    bind: Vec<String>,
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

    let cli_opts = birta::options::CliOptions {
        port: cli.port,
        no_open: cli.no_open,
        css: cli.css,
        theme: cli.theme,
        syntax_theme: cli.syntax_theme,
        light: cli.light,
        dark: cli.dark,
        font_body: cli.font_body,
        font_mono: cli.font_mono,
        reading_mode: cli.reading_mode,
        no_header: cli.no_header,
        no_theme_swap: cli.no_theme_swap,
        no_toggle: cli.no_toggle,
    };
    let merged = birta::options::merge(cli_opts, &config);

    let custom_css = match &merged.css_path {
        Some(path) => {
            if !path.exists() {
                anyhow::bail!("CSS file not found: {}", path.display());
            }
            Some(std::fs::read_to_string(path)?)
        }
        None => None,
    };

    let mut theme = birta::theme::resolve(
        &config,
        merged.theme_name.as_deref(),
        merged.syntax_theme.as_deref(),
    )?;

    if merged.light {
        theme.active_variant = birta::theme::Variant::Light;
    } else if merged.dark {
        theme.active_variant = birta::theme::Variant::Dark;
    }

    let font_config = birta::config::FontConfig {
        body: merged.font_body,
        mono: merged.font_mono,
    };
    let font_css = font_config.to_css();

    let mut keybindings = config.keybindings.clone();
    keybindings.apply_overrides(&cli.bind);
    let keybindings_json = keybindings.to_json();

    if file.as_os_str() == "-" {
        let mut markdown = String::new();
        std::io::stdin().read_to_string(&mut markdown)?;
        let opts = birta::server::ServerOptions {
            port: merged.port,
            no_open: merged.no_open,
            custom_css,
            font_css,
            theme,
            enable_swap: merged.enable_swap,
            enable_toggle: merged.enable_toggle,
            show_header: merged.show_header,
            reading_mode: merged.reading_mode,
            keybindings_json,
        };
        return birta::server::run_stdin(&markdown, opts).await;
    }

    if !file.exists() {
        anyhow::bail!("file not found: {}", file.display());
    }

    if let Some(ext) = file.extension().and_then(|e| e.to_str())
        && ext != "md"
        && ext != "markdown"
    {
        eprintln!(
            "birta: warning: {} does not have a .md or .markdown extension",
            file.display()
        );
    }

    if cli.static_mode {
        return run_static(
            &file,
            StaticOptions {
                theme: &theme,
                custom_css: custom_css.as_deref(),
                font_css: font_css.as_deref(),
                show_header: merged.show_header,
                reading_mode: merged.reading_mode,
                no_open: merged.no_open,
                keybindings_json: &keybindings_json,
            },
        );
    }

    let opts = birta::server::ServerOptions {
        port: merged.port,
        no_open: merged.no_open,
        custom_css,
        font_css,
        theme,
        enable_swap: merged.enable_swap,
        enable_toggle: merged.enable_toggle,
        show_header: merged.show_header,
        reading_mode: merged.reading_mode,
        keybindings_json,
    };
    birta::server::run(file, opts).await
}

struct StaticOptions<'a> {
    theme: &'a birta::theme::ResolvedTheme,
    custom_css: Option<&'a str>,
    font_css: Option<&'a str>,
    show_header: bool,
    reading_mode: bool,
    no_open: bool,
    keybindings_json: &'a str,
}

fn run_static(file: &std::path::Path, opts: StaticOptions<'_>) -> anyhow::Result<()> {
    let markdown = std::fs::read_to_string(file)?;
    let base_dir = file
        .parent()
        .map(|p| {
            if p.as_os_str().is_empty() {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            } else {
                std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
            }
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let content_html = birta::render::render_static(
        &markdown,
        opts.theme.active_data().syntax.as_ref(),
        &base_dir,
    );

    let page = birta::template::render_page(&birta::template::PageOptions {
        filename: &file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string()),
        content_html: &content_html,
        custom_css: opts.custom_css,
        font_css: opts.font_css,
        show_header: opts.show_header,
        reading_mode: opts.reading_mode,
        theme: opts.theme,
        theme_names: &[],
        static_mode: true,
        keybindings_json: opts.keybindings_json,
    });

    let filename = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());
    let out_path = std::env::temp_dir().join(format!("birta-{filename}.html"));
    std::fs::write(&out_path, &page)?;

    eprintln!("birta: wrote {}", out_path.display());

    if !opts.no_open
        && let Err(e) = open::that(&out_path)
    {
        eprintln!("birta: failed to open browser: {e}");
    }

    Ok(())
}
