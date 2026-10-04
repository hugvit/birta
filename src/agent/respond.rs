//! The `respond` heredoc grammar. An agent pastes what `birta review` printed under
//! `next:`, filled in, so the parser forgives the pasting: indent, a trailing `EOF`,
//! `\r`, and a code fence around the block. Every error is collected, with its line.

use std::collections::HashSet;
use std::fmt;

/// A quote longer than this is not a quote of one spot.
const QUOTE_MAX: usize = 200;
/// What `birta review` leaves for the agent to fill in, now and in earlier builds. The
/// covers one ends in the note's number, and the turn note's in an example.
const PLACEHOLDERS: &[&str] = &[
    "<your answer>",
    "<one line: what you changed for this whole-doc note>",
    "<a sentence you wrote for #",
    "<one or two lines to the reviewer in first person",
    "<why>",
    "<reply>",
    "<question>",
    "<exact words in the doc>",
    "<exact new words>",
    "<new text>",
    "<new text you wrote for the whole-doc note>",
    "<two lines: what you did, didn't, ask>",
    "<words you wrote>",
    "<exact words you added or removed>",
];

/// One line of the heredoc, with any lines that continue it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Done { n: u32, note: Option<String> },
    Answer { n: u32, body: String },
    Didnt { n: u32, body: String },
    Covers { n: u32, quote: String },
    Ask { quote: String, body: String },
    Unasked { quote: String, body: Option<String> },
    TurnNote { body: String },
}

impl Entry {
    /// The note it settles, for the one-outcome-per-note rule.
    fn outcome_of(&self) -> Option<u32> {
        match self {
            Self::Done { n, .. } | Self::Answer { n, .. } | Self::Didnt { n, .. } => Some(*n),
            _ => None,
        }
    }

    fn takes_text(&self) -> bool {
        !matches!(self, Self::Done { .. } | Self::Covers { .. })
    }

    /// The text continuation lines extend.
    fn text_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Answer { body, .. }
            | Self::Didnt { body, .. }
            | Self::Ask { body, .. }
            | Self::TurnNote { body } => Some(body),
            Self::Unasked { body, .. } => Some(body.get_or_insert_with(String::new)),
            Self::Done { .. } | Self::Covers { .. } => None,
        }
    }

    fn texts(&self) -> Vec<&str> {
        match self {
            Self::Done { note, .. } => note.as_deref().into_iter().collect(),
            Self::Answer { body, .. } | Self::Didnt { body, .. } | Self::TurnNote { body } => {
                vec![body]
            }
            Self::Covers { quote, .. } => vec![quote],
            Self::Ask { quote, body } => vec![quote, body],
            Self::Unasked { quote, body } => std::iter::once(quote.as_str())
                .chain(body.as_deref())
                .collect(),
        }
    }
}

/// A problem on 1-based line `line` of the input as it was piped in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

/// Parses a whole heredoc. Entries carry the input line they start on.
pub fn parse(input: &str) -> Result<Vec<(usize, Entry)>, Vec<ParseError>> {
    let lines = trim_block(input);
    let mut errors = Vec::new();
    let mut entries: Vec<(usize, Entry)> = Vec::new();
    // Blank lines seen since the open entry's last text, kept only if more text follows
    let mut blanks = 0;
    for &(no, line) in &lines {
        if let Some(start) = item_start(line) {
            blanks = 0;
            match start {
                Ok(entry) => entries.push((no, entry)),
                Err(msg) => errors.push(ParseError { line: no, msg }),
            }
            continue;
        }
        if line.trim().is_empty() {
            blanks += 1;
            continue;
        }
        match entries.last_mut() {
            Some((_, entry)) if entry.takes_text() => {
                if let Some(text) = entry.text_mut() {
                    if !text.is_empty() {
                        text.push_str(&"\n".repeat(blanks + 1));
                    }
                    text.push_str(line);
                }
                blanks = 0;
            }
            Some((_, entry)) => errors.push(ParseError {
                line: no,
                msg: format!(
                    "`{}` takes nothing on the lines after it",
                    keyword(entry)
                ),
            }),
            None => errors.push(ParseError {
                line: no,
                msg: "not a respond line: start with #N done, #N answer:, #N didnt:, #N covers, ask, unasked or @:".to_string(),
            }),
        }
    }

    let mut settled = HashSet::new();
    for (no, entry) in &entries {
        if let Some(n) = entry.outcome_of()
            && !settled.insert(n)
        {
            errors.push(ParseError {
                line: *no,
                msg: format!("#{n} already has an outcome: one of done, answer or didnt"),
            });
        }
        let empty = match entry {
            Entry::Answer { body, .. }
            | Entry::Didnt { body, .. }
            | Entry::Ask { body, .. }
            | Entry::TurnNote { body } => body.is_empty(),
            _ => false,
        };
        if empty {
            errors.push(ParseError {
                line: *no,
                msg: format!("`{}` needs text after its colon", keyword(entry)),
            });
        }
        if let Some(left) = entry.texts().iter().find_map(|t| placeholder_in(t)) {
            errors.push(ParseError {
                line: *no,
                msg: format!("placeholder left in: {left}"),
            });
        }
    }
    if entries.is_empty() && errors.is_empty() {
        errors.push(ParseError {
            line: 1,
            msg: "nothing to record: pipe in the heredoc from `birta review`".to_string(),
        });
    }
    if errors.is_empty() {
        Ok(entries)
    } else {
        errors.sort_by_key(|e| e.line);
        Err(errors)
    }
}

/// The placeholder a line of `text` still is, so a tag in real text that reads like one
/// passes. One whose end holds a number or an example only has to start the line.
fn placeholder_in(text: &str) -> Option<&'static str> {
    text.lines().map(str::trim).find_map(|line| {
        PLACEHOLDERS
            .iter()
            .copied()
            .find(|p| line == *p || (!p.ends_with('>') && line.starts_with(p)))
    })
}

fn keyword(entry: &Entry) -> &'static str {
    match entry {
        Entry::Done { .. } => "done",
        Entry::Answer { .. } => "answer:",
        Entry::Didnt { .. } => "didnt:",
        Entry::Covers { .. } => "covers",
        Entry::Ask { .. } => "ask",
        Entry::Unasked { .. } => "unasked",
        Entry::TurnNote { .. } => "@:",
    }
}

/// The input's lines, numbered as piped in, without `\r`, a wrapping code fence, the
/// heredoc's closing `EOF`, and the indent they share.
fn trim_block(input: &str) -> Vec<(usize, &str)> {
    let mut lines: Vec<(usize, &str)> = input
        .split('\n')
        .enumerate()
        .map(|(i, l)| (i + 1, l.strip_suffix('\r').unwrap_or(l)))
        .collect();
    let mut fenced = false;
    loop {
        while lines.last().is_some_and(|(_, l)| l.trim().is_empty()) {
            lines.pop();
        }
        if lines.last().is_some_and(|(_, l)| l.trim() == "EOF") {
            lines.pop();
            continue;
        }
        let first = lines.iter().position(|(_, l)| !l.trim().is_empty());
        let wraps = first.is_some_and(|i| {
            i + 1 < lines.len()
                && lines[i].1.trim_start().starts_with("```")
                && lines[lines.len() - 1].1.trim() == "```"
        });
        if fenced || !wraps {
            break;
        }
        fenced = true;
        lines.pop();
        lines.drain(..=first.unwrap_or(0));
    }
    let indent = lines
        .iter()
        .map(|(_, l)| *l)
        .filter(|l| !l.trim().is_empty())
        .map(|l| &l[..l.len() - l.trim_start_matches([' ', '\t']).len()])
        .reduce(|a, b| common_prefix(a, b))
        .unwrap_or("");
    lines
        .into_iter()
        .map(|(no, l)| match l.strip_prefix(indent) {
            Some(rest) => (no, rest),
            None => (no, if l.trim().is_empty() { "" } else { l }),
        })
        .collect()
}

fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let len = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    &a[..len]
}

/// The entry a line starts, if it starts one. Only a well-formed start counts, so a
/// body line that merely begins with `#12` continues the text before it.
fn item_start(line: &str) -> Option<Result<Entry, String>> {
    if let Some(rest) = line.strip_prefix('#') {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let (number, rest) = rest.split_at(digits);
        let rest = rest.strip_prefix(' ')?;
        let starts = ["done", "answer:", "didnt:", "covers "]
            .iter()
            .any(|k| rest.starts_with(k));
        if digits == 0 || !starts {
            return None;
        }
        return Some(numbered(number, rest));
    }
    if let Some(rest) = line.strip_prefix("ask ").filter(|r| opens_quote(r)) {
        return Some(ask(rest));
    }
    if let Some(rest) = line.strip_prefix("unasked ").filter(|r| opens_quote(r)) {
        return Some(unasked(rest));
    }
    let body = line.strip_prefix("@:")?;
    Some(Ok(Entry::TurnNote {
        body: body.trim().to_string(),
    }))
}

fn numbered(number: &str, rest: &str) -> Result<Entry, String> {
    let n = match number.parse::<u32>() {
        Ok(0) | Err(_) => return Err(format!("#{number} is not a note number")),
        Ok(n) => n,
    };
    if let Some(after) = rest.strip_prefix("done") {
        let note = match after.strip_prefix(':') {
            Some(note) => Some(note.trim()).filter(|t| !t.is_empty()),
            None if after.trim().is_empty() => None,
            None => return Err(format!("#{n}: write `done` or `done: why`")),
        };
        return Ok(Entry::Done {
            n,
            note: note.map(str::to_string),
        });
    }
    if let Some(body) = rest.strip_prefix("answer:") {
        return Ok(Entry::Answer {
            n,
            body: body.trim().to_string(),
        });
    }
    if let Some(body) = rest.strip_prefix("didnt:") {
        return Ok(Entry::Didnt {
            n,
            body: body.trim().to_string(),
        });
    }
    let rest = rest.strip_prefix("covers ").unwrap_or(rest);
    let (quote, after) = quote(rest)?;
    if !after.trim().is_empty() {
        return Err(format!("#{n} covers takes only a quote"));
    }
    Ok(Entry::Covers { n, quote })
}

fn ask(rest: &str) -> Result<Entry, String> {
    let (quote, after) = quote(rest)?;
    let body = after
        .trim_start()
        .strip_prefix(':')
        .ok_or("write `ask \"quote\": your question`")?;
    Ok(Entry::Ask {
        quote,
        body: body.trim().to_string(),
    })
}

fn unasked(rest: &str) -> Result<Entry, String> {
    let (quote, after) = quote(rest)?;
    let after = after.trim();
    let body = match after.strip_prefix(':') {
        Some(body) => Some(body.trim().to_string()).filter(|b| !b.is_empty()),
        None if after.is_empty() => None,
        None => return Err("write `unasked \"quote\"` or `unasked \"quote\": why`".to_string()),
    };
    Ok(Entry::Unasked { quote, body })
}

fn opens_quote(s: &str) -> bool {
    s.starts_with('"') || s.starts_with('\u{201c}')
}

/// A `"…"` quote with `\"` escapes, or a `“…”` pair. Returns it and the rest of the line.
fn quote(s: &str) -> Result<(String, &str), String> {
    let (close, body) = match (s.strip_prefix('"'), s.strip_prefix('\u{201c}')) {
        (Some(body), _) => ('"', body),
        (_, Some(body)) => ('\u{201d}', body),
        _ => return Err("expected a quote in \"…\"".to_string()),
    };
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        if close == '"' && c == '\\' {
            match chars.next() {
                Some((_, next)) => out.push(next),
                None => break,
            }
            continue;
        }
        if c == close {
            let count = out.chars().count();
            if count == 0 {
                return Err("the quote is empty".to_string());
            }
            if count > QUOTE_MAX {
                return Err(format!(
                    "the quote is {count} chars; quote at most {QUOTE_MAX} of one spot"
                ));
            }
            return Ok((out, &body[i + c.len_utf8()..]));
        }
        out.push(c);
    }
    Err("the quote is never closed".to_string())
}
