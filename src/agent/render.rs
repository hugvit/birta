//! The text `birta review` prints. Plain text on any output, since agents copy from it.

use crate::review::ops::{By, ByKind, Verdict};

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
    /// Whoever wrote the hand-back; a line with no `by` has the default, `Unknown`.
    pub handed_by: By,
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
    pub by: By,
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

/// How a view names whoever wrote a line: their name, else what they are. The page writes
/// no name, so a nameless human is the person reviewing.
pub fn who(by: &By) -> String {
    match (by.name.as_deref().filter(|n| !n.is_empty()), by.kind) {
        (Some(name), _) => one_line(name),
        (None, ByKind::Human) => "your reviewer".to_string(),
        (None, ByKind::Agent) => "an agent".to_string(),
        (None, ByKind::Birta) => "birta".to_string(),
        (None, ByKind::Unknown) => "someone".to_string(),
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
        who(&v.handed_by),
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
    // Wide enough for the widest verdict shown, so every item's text starts in one column
    let width = notes[..shown]
        .iter()
        .map(|i| verdict_word(i.verdict).len())
        .fold(COLUMN - 1, usize::max);
    for item in &notes[..shown] {
        out.push(format!(
            "{}{:<width$} {}",
            column(&format!("#{}", item.n)),
            verdict_word(item.verdict),
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
        who(&v.by)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn view(verdicts: &[Option<Verdict>]) -> RoundView {
        RoundView {
            name: "pr.md".to_string(),
            path: "pr.md".to_string(),
            handed_at: 0,
            handed_by: By {
                kind: ByKind::Human,
                name: Some("andres".to_string()),
                via: None,
            },
            drafted_by: None,
            stance: Stance::Mine,
            items: verdicts
                .iter()
                .enumerate()
                .map(|(i, v)| ItemLine {
                    n: 6 + i as u32,
                    verdict: *v,
                    body: None,
                    target: Target::Quote(format!("item {i}")),
                })
                .collect(),
            you: vec![],
            all: false,
        }
    }

    /// The item lines of a view: those that start with a note label.
    fn item_lines(out: &str) -> Vec<&str> {
        out.lines().filter(|l| l.starts_with('#')).collect()
    }

    #[test]
    fn the_verdict_column_fits_the_widest_verdict() {
        let out = round(&view(&[
            Some(Verdict::No),
            Some(Verdict::Rewrite),
            None,
            Some(Verdict::YourCall),
        ]));
        let lines = item_lines(&out);
        assert_eq!(lines.len(), 4, "{out}");
        let starts: Vec<usize> = lines
            .iter()
            .map(|l| l.chars().position(|c| c == '"').unwrap())
            .collect();
        assert!(starts.iter().all(|s| *s == starts[0]), "{out}");
        for (line, word) in lines.iter().zip(["no", "rewrite", "note", "your_call"]) {
            assert!(line.contains(&format!(" {word} ")), "{line}");
        }
    }

    #[test]
    fn short_verdicts_keep_the_five_wide_column() {
        let out = round(&view(&[None, Some(Verdict::No), Some(Verdict::Ask)]));
        assert_eq!(
            item_lines(&out),
            [
                "#6    note  \"item 0\"",
                "#7    no    \"item 1\"",
                "#8    ask   \"item 2\"   (answer only; don't edit)",
            ],
            "{out}"
        );
    }

    #[test]
    fn a_hand_back_or_approve_names_its_author_by_what_they_are() {
        let by = |kind, name: Option<&str>| By {
            kind,
            name: name.map(str::to_string),
            via: None,
        };
        let cases = [
            (by(ByKind::Human, Some("andres")), "andres"),
            (by(ByKind::Human, None), "your reviewer"),
            (by(ByKind::Human, Some("")), "your reviewer"),
            (by(ByKind::Agent, None), "an agent"),
            (By::default(), "someone"),
        ];
        for (author, want) in cases {
            let mut v = view(&[None]);
            v.handed_by = author.clone();
            let out = round(&v);
            assert!(
                out.lines()
                    .next()
                    .unwrap()
                    .contains(&format!(" by {want} · ")),
                "{out}"
            );
            let a = ApprovedView {
                name: "pr.md".into(),
                path: "pr.md".into(),
                at: 0,
                by: author,
                drafted_by: None,
                rev: "0123456789abcdef".into(),
                changed_to: None,
            };
            let out = approved(&a);
            assert!(
                out.lines()
                    .next()
                    .unwrap()
                    .ends_with(&format!(" by {want}")),
                "{out}"
            );
        }
    }
}
