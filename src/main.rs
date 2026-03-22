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
    file: PathBuf,

    /// Port to serve on (0 = auto-assign)
    #[arg(short, long, default_value_t = 0)]
    port: u16,

    /// Don't open the browser automatically
    #[arg(long)]
    no_open: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.file.as_os_str() == "-" {
        let mut markdown = String::new();
        std::io::stdin().read_to_string(&mut markdown)?;
        return sheen::server::run_stdin(&markdown, cli.port, cli.no_open).await;
    }

    if !cli.file.exists() {
        anyhow::bail!("file not found: {}", cli.file.display());
    }

    if let Some(ext) = cli.file.extension().and_then(|e| e.to_str()) {
        if ext != "md" && ext != "markdown" {
            eprintln!(
                "sheen: warning: {} does not have a .md or .markdown extension",
                cli.file.display()
            );
        }
    }

    sheen::server::run(cli.file, cli.port, cli.no_open).await
}
