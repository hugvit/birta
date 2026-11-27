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
  birta --diff old.md new.md             Live diff of two files
  birta --list-themes                    Show available themes
  cat notes.md | birta -                 Preview from stdin

\x1b[1;4mConfig:\x1b[0m
  ~/.config/birta/config.toml            Persistent settings
  ~/.config/birta/themes/<name>.toml     Custom themes"
)]
struct Cli {
    /// Markdown file to preview, or "-" for stdin
    file: Option<PathBuf>,

    // -- Diff -----------------------------------------------------------------
    /// Show a live diff of OLD against NEW instead of a preview
    #[arg(
        long,
        num_args = 2,
        value_names = ["OLD", "NEW"],
        conflicts_with_all = ["file", "static_mode"],
        help_heading = "Diff"
    )]
    diff: Option<Vec<PathBuf>>,

    /// Diff layout; auto is side by side when the window is wide enough [default: auto]
    #[arg(long, value_enum, help_heading = "Diff")]
    diff_layout: Option<DiffLayout>,

    // -- Server ---------------------------------------------------------------
    /// Port to serve on [default: auto-assign]
    #[arg(short, long, help_heading = "Server")]
    port: Option<u16>,

    /// Don't open the browser automatically
    #[arg(long, help_heading = "Server")]
    no_open: bool,

    /// Render a self-contained HTML bundle (crawls linked files), open it, and exit
    #[arg(long = "static", help_heading = "Server")]
    static_mode: bool,

    /// Output directory for --static [default: a temp birta-<name> folder]
    #[arg(long, value_name = "DIR", help_heading = "Server")]
    out: Option<PathBuf>,

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

    /// Start in raw mode (show source instead of rendered preview)
    #[arg(long, help_heading = "Display")]
    raw: bool,

    /// Show YAML front matter as markdown instead of a table
    #[arg(long, help_heading = "Display")]
    no_front_matter: bool,

    /// Leave Obsidian ![[image]] embeds as plain text
    #[arg(long, help_heading = "Display")]
    no_embeds: bool,

    /// Render Obsidian #tags as tags
    #[arg(long, help_heading = "Display")]
    tags: bool,

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

#[derive(Clone, Copy, Default, clap::ValueEnum)]
enum DiffLayout {
    #[default]
    Auto,
    Unified,
    Split,
}

impl From<DiffLayout> for birta_core::diff::Layout {
    fn from(layout: DiffLayout) -> Self {
        match layout {
            DiffLayout::Auto => Self::Auto,
            DiffLayout::Unified => Self::Unified,
            DiffLayout::Split => Self::Split,
        }
    }
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

    // Checked here: clap waives `requires` when the required arg conflicts with FILE
    if cli.diff_layout.is_some() && cli.diff.is_none() {
        anyhow::bail!("--diff-layout needs --diff <OLD> <NEW>");
    }
    let diff_files = match cli.diff {
        Some(paths) => {
            for path in &paths {
                if !path.exists() {
                    anyhow::bail!("file not found: {}", path.display());
                }
            }
            Some(paths)
        }
        None => None,
    };
    let file = match (&diff_files, cli.file) {
        (Some(_), _) => None,
        (None, Some(file)) => Some(file),
        (None, None) => anyhow::bail!("missing required argument: FILE"),
    };

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
        raw_mode: cli.raw,
        no_header: cli.no_header,
        no_theme_swap: cli.no_theme_swap,
        no_toggle: cli.no_toggle,
        no_front_matter: cli.no_front_matter,
        no_embeds: cli.no_embeds,
        tags: cli.tags,
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

    // Whether the variant was explicitly requested (CLI flags or config
    // `theme.variant`). When set, the browser must not override it with the OS
    // `prefers-color-scheme` preference.
    let variant_explicit = merged.light || merged.dark;

    let font_config = birta::config::FontConfig {
        body: merged.font_body,
        mono: merged.font_mono,
    };
    let font_css = font_config.to_css();

    let mut keybindings = config.keybindings.clone();
    keybindings.apply_overrides(&cli.bind);
    let keybindings_json = keybindings.to_json();

    let Some(file) = file else {
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
            raw_mode: merged.raw_mode,
            features: merged.features,
            keybindings_json,
            variant_explicit,
        };
        let [old, new]: [PathBuf; 2] = diff_files
            .unwrap_or_default()
            .try_into()
            .map_err(|_| anyhow::anyhow!("--diff takes exactly two files"))?;
        return birta::server::run_diff(old, new, cli.diff_layout.unwrap_or_default().into(), opts)
            .await;
    };

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
            raw_mode: merged.raw_mode,
            features: merged.features,
            keybindings_json,
            variant_explicit,
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
                raw_mode: merged.raw_mode,
                features: merged.features,
                no_open: merged.no_open,
                keybindings_json: &keybindings_json,
                variant_explicit,
                out: cli.out.as_deref(),
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
        raw_mode: merged.raw_mode,
        features: merged.features,
        keybindings_json,
        variant_explicit,
    };
    birta::server::run(file, opts).await
}

struct StaticOptions<'a> {
    theme: &'a birta::theme::ResolvedTheme,
    custom_css: Option<&'a str>,
    font_css: Option<&'a str>,
    show_header: bool,
    reading_mode: bool,
    raw_mode: bool,
    features: birta::render::Features,
    no_open: bool,
    keybindings_json: &'a str,
    variant_explicit: bool,
    out: Option<&'a std::path::Path>,
}

fn run_static(file: &std::path::Path, opts: StaticOptions<'_>) -> anyhow::Result<()> {
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

    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());
    let out_dir = opts
        .out
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("birta-{stem}")));

    let result = birta::static_export::export_bundle(
        file,
        &base_dir,
        &out_dir,
        &birta::static_export::BundleOptions {
            theme: opts.theme,
            custom_css: opts.custom_css,
            font_css: opts.font_css,
            show_header: opts.show_header,
            reading_mode: opts.reading_mode,
            raw_mode: opts.raw_mode,
            features: opts.features,
            variant_explicit: opts.variant_explicit,
            keybindings_json: opts.keybindings_json,
        },
    )?;

    eprintln!(
        "birta: wrote bundle to {} ({} page(s), {} asset(s))",
        out_dir.display(),
        result.pages,
        result.assets
    );

    if !opts.no_open
        && let Err(e) = open::that(&result.entry_html)
    {
        eprintln!("birta: failed to open browser: {e}");
    }

    Ok(())
}
