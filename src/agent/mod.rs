//! The agent verbs. They run synchronously, with no server, no runtime and no theme
//! install, and only while the review gate is on.

pub mod identity;

use std::path::PathBuf;

use clap::{Args, Subcommand};

pub const GATE_OFF: &str =
    "review is off: set [review] enabled = true in ~/.config/birta/config.toml";

/// Every exit status an agent verb can end with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitCode {
    Ok = 0,
    Error = 1,
    Usage = 2,
    ApprovedChanged = 3,
    Stale = 4,
    Quote = 65,
    Timeout = 75,
}

/// Each verb is hidden from `--help` until the gate is on.
#[derive(Subcommand)]
pub enum Cmd {
    /// Show the notes handed back on a doc, and pick them up
    #[command(hide = true)]
    Review(ReviewArgs),
    /// Answer the notes picked up, from a heredoc on stdin
    #[command(hide = true)]
    Respond(RespondArgs),
    /// Leave a note or question on a doc
    #[command(hide = true)]
    Comment(CommentArgs),
    /// Print a doc's exact bytes at a revision
    #[command(hide = true)]
    Show(ShowArgs),
}

#[derive(Args)]
pub struct ReviewArgs {
    /// Markdown file or directory
    pub target: Option<PathBuf>,
    /// Wait until notes are handed back
    #[arg(long)]
    pub wait: bool,
    /// Seconds to wait; 0 waits forever
    #[arg(long, value_name = "SECS")]
    pub timeout: Option<u64>,
    /// Look without picking up
    #[arg(long)]
    pub peek: bool,
    /// Name the doc's drafter if waiting creates its review file
    #[arg(long, value_name = "NAME", requires = "wait")]
    pub drafted_by: Option<String>,
    /// Agent name to record
    #[arg(long = "as", value_name = "NAME")]
    pub as_name: Option<String>,
}

#[derive(Args)]
pub struct RespondArgs {
    pub file: PathBuf,
    /// Agent name to record
    #[arg(long = "as", value_name = "NAME")]
    pub as_name: Option<String>,
}

#[derive(Args)]
pub struct CommentArgs {
    pub file: PathBuf,
    /// Anchor on this quoted text
    #[arg(long, value_name = "Q", conflicts_with_all = ["line", "doc"])]
    pub quote: Option<String>,
    /// Anchor on this source line
    #[arg(long, value_name = "N", conflicts_with = "doc")]
    pub line: Option<u32>,
    /// Anchor on the whole doc
    #[arg(long)]
    pub doc: bool,
    /// Only `ask`
    #[arg(long, value_name = "VERDICT")]
    pub verdict: Option<String>,
    /// Who the note is for
    #[arg(long, value_name = "NAME")]
    pub to: Option<String>,
    /// Agent name to record
    #[arg(long = "as", value_name = "NAME")]
    pub as_name: Option<String>,
    /// The note; read from stdin when absent
    pub body: Option<String>,
}

#[derive(Args)]
pub struct ShowArgs {
    /// FILE, FILE@REV, FILE@approved or FILE@latest
    pub target: String,
}

impl Cmd {
    fn verb(&self) -> &'static str {
        match self {
            Self::Review(_) => "review",
            Self::Respond(_) => "respond",
            Self::Comment(_) => "comment",
            Self::Show(_) => "show",
        }
    }

    fn as_name(&self) -> Option<&str> {
        match self {
            Self::Review(a) => a.as_name.as_deref(),
            Self::Respond(a) => a.as_name.as_deref(),
            Self::Comment(a) => a.as_name.as_deref(),
            Self::Show(_) => None,
        }
    }
}

/// What a verb run with the gate off does, whatever its arguments.
pub fn gate_off() -> ExitCode {
    eprintln!("{GATE_OFF}");
    ExitCode::Error
}

pub fn run(cmd: Cmd) -> ExitCode {
    if let Some(raw) = cmd.as_name()
        && let Err(e) = identity::parse_as(raw)
    {
        eprintln!("error: {e}");
        return ExitCode::Usage;
    }
    eprintln!("birta: `{}` is not available in this build", cmd.verb());
    ExitCode::Error
}
