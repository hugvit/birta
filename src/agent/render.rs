//! The text `birta review` prints. Plain text on any output, since agents copy from it.

use crate::review::ops::Verdict;

/// Characters of a body or quote a view line keeps.
const CLIP_CHARS: usize = 160;
/// Item lines a view shows before `+N more`.
const ITEM_LINES: usize = 20;
/// Width of the label and verdict columns.
const COLUMN: usize = 6;
const ASK_HINT: &str = "   (answer only; don't edit)";
const COVERS: &str = "\"<new text you wrote for the whole-doc note>\"";

/// A note as a view line shows it.
#[derive(Clone, Debug)]
pub struct ItemLine {
    pub n: u32,
    pub verdict: Option<Verdict>,
    pub body: Option<String>,
    pub target: Target,
}

/// What a note points at, in source text.
#[derive(Clone, Debug)]
pub enum Target {
    Doc,
    Dir,
    Quote(String),
    Block(String),
    Section(String),
}

/// Whose round the view shows.
#[derive(Clone, Debug)]
pub enum Stance {
    /// Claimed by whoever runs the view.
    Mine,
    /// Shown without claiming; `tty` when that is because a person is at a terminal.
    Peek { tty: bool },
    /// Held by another agent's claim.
    ReadOnly { by: String, at: u64 },
}

/// One doc's round, ready to print.
#[derive(Clone, Debug)]
pub struct RoundView {
    pub name: String,
    /// The doc's path, shell-quoted.
    pub path: String,
    pub handed_at: u64,
    pub handed_by: Option<String>,
    pub drafted_by: Option<String>,
    pub stance: Stance,
    pub items: Vec<ItemLine>,
    pub you: Vec<String>,
    pub all: bool,
}

/// An approve, ready to print.
#[derive(Clone, Debug)]
pub struct ApprovedView {
    pub name: String,
    pub path: String,
    pub at: u64,
    pub by: Option<String>,
    pub drafted_by: Option<String>,
    pub rev: String,
    /// The doc's rev now, when it isn't the approved one.
    pub changed_to: Option<String>,
}

/// `path` as one shell word: as it is when plainly safe, else in single quotes.
pub fn shell_quote(path: &str) -> String {
    let plain = !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b));
    if plain {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', r"'\''"))
    }
}

/// `HH:MM` of epoch ms in the local time zone.
pub fn hhmm(ms: u64) -> String {
    let secs = libc::time_t::try_from(ms / 1000).unwrap_or(libc::time_t::MAX);
    // SAFETY: `tm` is plain data, for which all zeroes is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call, which keeps neither.
    let local = unsafe { libc::localtime_r(&secs, &mut tm) };
    if local.is_null() {
        let day = ms / 1000 % 86_400;
        return format!("{:02}:{:02}", day / 3600, day % 3600 / 60);
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// A character no view prints: C0 and C1 controls, and the bidi controls and marks that
/// reorder what a terminal shows.
pub fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' | '\u{061c}'
        )
}

/// `text` on one line: each run of whitespace holding a break becomes one space, and
/// other hidden characters are dropped, so no text can forge a line or reach a terminal.
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut broke = false;
    for c in text.chars() {
        if matches!(c, '\n' | '\r' | '\t' | '\u{2028}' | '\u{2029}') {
            out.truncate(out.trim_end().len());
            broke = true;
        } else if !(is_hidden(c) || broke && c.is_whitespace()) {
            if broke && !out.is_empty() {
                out.push(' ');
            }
            broke = false;
            out.push(c);
        }
    }
    out
}

/// `text` with every hidden or line-breaking character written as `\u{..}`, so a name
/// that can't be printed as it is can still be told apart.
pub fn escaped(text: &str) -> String {
    text.chars()
        .map(|c| {
            if is_hidden(c) || matches!(c, '\u{2028}' | '\u{2029}') {
                format!("\\u{{{:x}}}", u32::from(c))
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// `text` on one line, cut to 160 characters.
pub fn clip(text: &str) -> String {
    let line = one_line(text);
    match line.char_indices().nth(CLIP_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

fn verdict_word(verdict: Option<Verdict>) -> &'static str {
    match verdict {
        None => "note",
        Some(Verdict::Yes) => "yes",
        Some(Verdict::No) => "no",
        Some(Verdict::Ask) => "ask",
        Some(Verdict::Later) => "later",
        Some(Verdict::YourCall) => "your_call",
        Some(Verdict::Rewrite) => "rewrite",
        Some(Verdict::Stet) => "stet",
    }
}

/// `text` padded to the column, with at least one space after it.
fn column(text: &str) -> String {
    format!("{text:<width$} ", width = COLUMN - 1)
}

fn count(n: usize) -> String {
    if n == 1 {
        "1 item".to_string()
    } else {
        format!("{n} items")
    }
}

fn target(t: &Target) -> String {
    match t {
        Target::Doc => "the whole doc".to_string(),
        Target::Dir => "the whole folder".to_string(),
        Target::Quote(q) => format!("\"{}\"", clip(q)),
        Target::Block(b) => clip(b.lines().next().unwrap_or_default()),
        Target::Section(h) => format!(
            "{} (whole section)",
            clip(h.lines().next().unwrap_or_default())
        ),
    }
}

fn is_level(t: &Target) -> bool {
    matches!(t, Target::Doc | Target::Dir)
}

/// The line the view shows for a note, without its label.
fn item_text(item: &ItemLine) -> String {
    let mut line = target(&item.target);
    if let Some(body) = &item.body {
        line.push_str(" — ");
        line.push_str(&clip(body));
    }
    if item.verdict == Some(Verdict::Ask) {
        line.push_str(ASK_HINT);
    }
    line
}

/// A doc- or folder-level note: its label, then its verdict and body.
fn level_line(label: &str, item: &ItemLine) -> String {
    let mut line = format!("{}#{}", column(label), item.n);
    if let Some(v) = item.verdict {
        line.push(' ');
        line.push_str(verdict_word(Some(v)));
    }
    if let Some(body) = &item.body {
        line.push_str(&format!(" \"{}\"", clip(body)));
    }
    if item.verdict == Some(Verdict::Ask) {
        line.push_str(ASK_HINT);
    }
    line
}

/// The respond line an item asks for, if any.
fn asked(item: &ItemLine) -> Option<String> {
    let n = item.n;
    match (item.verdict, &item.target) {
        (Some(Verdict::Stet | Verdict::Yes | Verdict::Later), _) => None,
        (Some(Verdict::Ask), _) => Some(format!("#{n} answer: <your answer>")),
        (_, Target::Doc) => Some(format!("#{n} covers {COVERS}")),
        _ => Some(format!("#{n} done")),
    }
}

pub fn round(v: &RoundView) -> String {
    let stets: Vec<&ItemLine> = v
        .items
        .iter()
        .filter(|i| i.verdict == Some(Verdict::Stet))
        .collect();
    let (levels, notes): (Vec<&ItemLine>, Vec<&ItemLine>) = v
        .items
        .iter()
        .filter(|i| i.verdict != Some(Verdict::Stet))
        .partition(|i| is_level(&i.target));
    let shown = if v.all {
        notes.len()
    } else {
        notes.len().min(ITEM_LINES)
    };

    let mut header = format!(
        "{} · handed back {} by {} · {}",
        one_line(&v.name),
        hhmm(v.handed_at),
        one_line(v.handed_by.as_deref().unwrap_or("someone")),
        count(levels.len() + notes.len())
    );
    if let Some(drafter) = &v.drafted_by {
        header.push_str(&format!(" · drafted by {}", one_line(drafter)));
    }
    match &v.stance {
        Stance::Mine => {}
        Stance::Peek { tty: true } => {
            header.push_str(" · peek: not an agent shell · pass --as NAME to pick up");
        }
        Stance::Peek { tty: false } => header.push_str(" · peek"),
        Stance::ReadOnly { by, at } => {
            header.push_str(&format!(
                " · picked up by {} {} · read only",
                one_line(by),
                hhmm(*at)
            ));
        }
    }

    let mut out = vec![header];
    for stet in &stets {
        out.push(format!(
            "{}{} — do not change",
            column("STET"),
            target(&stet.target)
        ));
    }
    for level in &levels {
        let label = if matches!(level.target, Target::Dir) {
            "DIR"
        } else {
            "DOC"
        };
        out.push(level_line(label, level));
    }
    for item in &notes[..shown] {
        out.push(format!(
            "{}{}{}",
            column(&format!("#{}", item.n)),
            column(verdict_word(item.verdict)),
            item_text(item)
        ));
    }
    if shown < notes.len() {
        out.push(format!(
            "+{} more · birta review {} --all",
            notes.len() - shown,
            v.path
        ));
    }
    if matches!(v.stance, Stance::ReadOnly { .. }) {
        return lines(out);
    }
    for you in &v.you {
        out.push(format!("{}{}", column("YOU"), one_line(you)));
    }

    out.push(format!(
        "next: Edit (not Write) {}. For any change outside the quoted spots, cite the note it serves. Then:",
        v.path
    ));
    out.push(format!("  birta respond {} <<'EOF'", v.path));
    let asks = notes[..shown]
        .iter()
        .chain(&levels)
        .filter_map(|i| asked(i));
    out.extend(asks.map(|line| format!("  {line}")));
    out.push("  @: <two lines: what you did, didn't, ask>".to_string());
    out.push("  EOF".to_string());
    out.push(format!(
        "  birta review {} --wait     # background it if your host supports that",
        v.path
    ));
    lines(out)
}

pub fn approved(v: &ApprovedView) -> String {
    let short = |rev: &str| rev.chars().take(8).collect::<String>();
    let mut header = format!(
        "{} · approved {} by {}",
        one_line(&v.name),
        hhmm(v.at),
        one_line(v.by.as_deref().unwrap_or("someone"))
    );
    if let Some(drafter) = &v.drafted_by {
        header.push_str(&format!(" · drafted by {}", one_line(drafter)));
    }
    let rev = short(&v.rev);
    let current = match &v.changed_to {
        None => "current ✓".to_string(),
        Some(now) => format!("current: {}  CHANGED", short(now)),
    };
    lines(vec![
        header,
        format!("APPROVED rev {rev} ({current})"),
        format!(
            "next: tmp=$(mktemp) && birta show {}@{rev} > \"$tmp\" && gh pr create --body-file \"$tmp\"",
            v.path
        ),
    ])
}

/// What a doc or folder with nothing handed back shows. `path` is shell-quoted, and
/// `None` when no target was given.
pub fn nothing(name: &str, path: Option<&str>) -> String {
    lines(vec![
        format!("{} · nothing handed back yet", one_line(name)),
        format!("next: {}", rerun(path)),
    ])
}

/// The command that waits again.
pub fn rerun(path: Option<&str>) -> String {
    match path {
        Some(path) => format!("birta review {path} --wait"),
        None => "birta review --wait".to_string(),
    }
}

fn lines(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}
