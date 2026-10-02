//! The agent verbs. They run synchronously, with no server, no runtime and no theme
//! install, and only while the review gate is on.

pub mod comment;
pub mod identity;
pub mod render;
pub mod respond;
pub mod respond_apply;
pub mod review_cmd;
pub mod show;
pub mod wait;

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use crate::config::Config;

pub const GATE_OFF: &str =
    "review is off: set [review] enabled = true in ~/.config/birta/config.toml";

/// The heredoc `respond` reads, one example per kind of line.
const RESPOND_GRAMMAR: &str = "\
Stdin holds one line per entry, as `birta review` prints them under next:

  #4 done
      you did what note #4 asked
  #4 done: I cut the intro to one line.
      the same, with a word on how
  #5 answer: Yes, five retries is plenty.
      a reply, for a note that asks or one you'd rather answer
  #6 didnt: The table is generated, so I left it.
      why you didn't do what the note asked
  #7 covers \"Retries now use jitter\"
      cites a whole-doc note for text you wrote; quote it exactly
  ask \"Retries now use jitter\": Is jitter enough here?
      a question for the reviewer, on words in the doc
  unasked \"more items\": I fixed a typo on the way.
      a change no note asked for, and why
  @: I cut the intro and kept the table.
      one or two lines to the reviewer, in first person; no questions

A line that starts no entry continues the one above it. End the heredoc with EOF.";

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
    #[command(hide = true, after_help = RESPOND_GRAMMAR)]
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
    #[arg(long, value_name = "SECS", requires = "wait")]
    pub timeout: Option<u64>,
    /// Look without picking up
    #[arg(long)]
    pub peek: bool,
    /// Name the doc's drafter if waiting creates its review file
    #[arg(long, value_name = "NAME", requires = "wait", conflicts_with = "peek")]
    pub drafted_by: Option<String>,
    /// Show every item, not only the first 20
    #[arg(long)]
    pub all: bool,
    /// Print the round as one JSON object
    #[arg(long)]
    pub json: bool,
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
    #[arg(long, value_name = "Q", conflicts_with = "doc")]
    pub quote: Option<String>,
    /// Anchor on the block at this source line, or pick the quote nearest it
    #[arg(long, value_name = "N", conflicts_with = "doc")]
    pub line: Option<u32>,
    /// Anchor on the whole doc
    #[arg(long)]
    pub doc: bool,
    /// Only `ask`
    #[arg(long, value_name = "VERDICT")]
    pub verdict: Option<String>,
    /// The note this one answers: N or #N
    #[arg(long, value_name = "N")]
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
    fn as_name(&self) -> Option<&str> {
        match self {
            Self::Review(a) => a.as_name.as_deref(),
            Self::Respond(a) => a.as_name.as_deref(),
            Self::Comment(a) => a.as_name.as_deref(),
            Self::Show(_) => None,
        }
    }
}

pub const CONTROL_PATH: &str = "path has control characters";

/// What a verb run with the gate off does, whatever its arguments.
pub fn gate_off() -> ExitCode {
    warn(GATE_OFF);
    ExitCode::Error
}

/// Prints `line` to stdout. A reader that went away is no error: what the verb wrote has
/// landed, and an error exit would only invite a retry that lands it twice.
pub(crate) fn say(line: &str) {
    let _ = writeln!(io::stdout().lock(), "{line}");
}

/// [`say`] for stderr.
pub(crate) fn warn(line: &str) {
    let _ = writeln!(io::stderr().lock(), "{line}");
}

/// Whether a view could print `path` only with a control, bidi or line-break character.
pub(crate) fn hidden_in(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .any(|c| render::is_hidden(c) || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// `to` as seen from the folder `from`, both canonical: `.`, `docs` or `../..`.
pub(crate) fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out: PathBuf = from[common..].iter().map(|_| "..").collect();
    out.extend(&to[common..]);
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

pub fn run(cmd: Cmd, config: &Config) -> ExitCode {
    if let Some(raw) = cmd.as_name()
        && let Err(e) = identity::parse_as(raw)
    {
        warn(&format!("error: {e}"));
        return ExitCode::Usage;
    }
    match &cmd {
        Cmd::Review(args) => review_cmd::run(args, config),
        Cmd::Respond(args) => respond_apply::run(args),
        Cmd::Comment(args) => comment::run(args, config),
        Cmd::Show(args) => show::run(args),
    }
}
