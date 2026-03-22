use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    version,
    about = "Preview markdown files in the browser with GitHub-style rendering"
)]
struct Cli {
    /// Path to the markdown file to preview
    file: PathBuf,

    /// Port to serve on (0 = auto-assign)
    #[arg(short, long, default_value_t = 0)]
    port: u16,

    /// Don't open the browser automatically
    #[arg(long)]
    no_open: bool,
}

fn main() {
    let cli = Cli::parse();
    eprintln!(
        "sheen: file={}, port={}, no_open={}",
        cli.file.display(),
        cli.port,
        cli.no_open
    );
}
