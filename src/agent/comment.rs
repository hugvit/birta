//! `birta comment`: an agent's note or question on a doc, placed on the text on disk by
//! the one anchor constructor. Every check runs before the review file is made. Under the
//! file's lock the doc is read once, and those bytes are both what the note is placed on
//! and what is recorded ahead of it.

use std::io::{self, IsTerminal, Read};

use super::identity;
use super::render::one_line;
use super::respond_apply::replay;
use super::{CONTROL_PATH, CommentArgs, ExitCode, hidden_in, say, wait, warn};
use crate::anchor::{self, Cand};
use crate::config::Config;
use crate::review::doc::Doc;
use crate::review::log::{AppendCtx, AppendError, Log};
use crate::review::ops::{AnchorReq, At, BlockScope, By, ByKind, Line, Op, Verdict, Via};
use crate::review::sync::{read_doc, try_sync_bytes};
use crate::review::{anchor_from_req, review_path_for};

/// Bytes a note may hold once its control characters are dropped.
const MAX_BODY: usize = 64 * 1024;
/// Places an ambiguous quote's error lists.
const PLACES: usize = 5;
/// Code points of a place's line shown beside it.
const EXCERPT: usize = 60;
/// Code points of owner text either side of a quote that tell twins apart.
const CONTEXT: usize = 32;

/// Why a comment was refused, and the exit code that says so.
struct Refusal {
    code: ExitCode,
    msg: String,
}

fn usage(msg: impl Into<String>) -> Refusal {
    Refusal {
        code: ExitCode::Usage,
        msg: msg.into(),
    }
}

fn quote_err(msg: impl Into<String>) -> Refusal {
    Refusal {
        code: ExitCode::Quote,
        msg: msg.into(),
    }
}

fn failed(msg: impl Into<String>) -> Refusal {
    Refusal {
        code: ExitCode::Error,
        msg: msg.into(),
    }
}

/// What the flags point at, before the doc is read.
enum Target {
    Doc,
    Line(u32),
    Quote { q: String, hint: Option<u32> },
}

pub fn run(args: &CommentArgs, config: &Config) -> ExitCode {
    match comment(args, config) {
        Ok(n) => {
            say(&format!("commented: #{n}"));
            ExitCode::Ok
        }
        Err(Refusal { code, msg }) => {
            warn(&format!("comment: {msg}"));
            code
        }
    }
}

fn comment(args: &CommentArgs, config: &Config) -> Result<u32, Refusal> {
    let target = match (&args.quote, args.line, args.doc) {
        (Some(q), hint, _) => Target::Quote { q: q.clone(), hint },
        (None, Some(n), _) => Target::Line(n),
        (None, None, true) => Target::Doc,
        (None, None, false) => return Err(usage("choose --quote, --line or --doc")),
    };
    let verdict = match args.verdict.as_deref() {
        None => None,
        Some("ask") => Some(Verdict::Ask),
        Some(other) => {
            return Err(usage(format!(
                "an agent may only ask: --verdict ask, or no verdict (not {other:?})"
            )));
        }
    };
    let to = args.to.as_deref().map(parse_to).transpose()?;

    let file = &args.file;
    if hidden_in(file) {
        return Err(failed(CONTROL_PATH));
    }
    if file.as_os_str() == "-" || file.is_dir() {
        return Err(usage("comment needs a file"));
    }
    let doc_path = file
        .canonicalize()
        .map_err(|_| failed(format!("not found: {}", file.display())))?;
    let body = read_body(args.body.as_deref())?;

    let name = file.to_string_lossy().into_owned();
    let read = || {
        let bytes = read_doc(&doc_path).map_err(|e| failed(format!("reading {name}: {e}")))?;
        String::from_utf8(bytes).map_err(|_| failed(format!("{name} is not UTF-8")))
    };

    let log = Log::open(&review_path_for(&doc_path)).git_exclude(config.review.git_exclude);
    if log.read().ok().flatten().is_none() {
        // A note can only answer one already in the file, so this never makes the file
        if let Some(n) = to {
            return Err(usage(format!("#{n} is not a note in {name}")));
        }
        // Placed once here only so a refused note makes no file
        let text = read()?;
        place(&Doc::new(&text), &target, &name)?;
        let doc_name = doc_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        log.create_if_missing(&doc_name, &text, None)
            .map_err(|e| failed(format!("creating the review file: {e}")))?;
    }
    let mut held = log
        .hold()
        .map_err(|e| failed(format!("locking the review file: {e}")))?;
    if held.parsed().read_only {
        return Err(failed(
            "the review file is from a newer birta; it is read-only here",
        ));
    }
    if let Some(n) = to
        && !is_live_note(&held.parsed().lines, n)
    {
        return Err(usage(format!("#{n} is not a note in {name}")));
    }

    let text = read()?;
    let doc = Doc::new(&text);
    let at = place(&doc, &target, &name)?;
    let now = wait::now_ms();
    try_sync_bytes(&mut held, &doc_path, text.as_bytes(), now)
        .map_err(|e| failed(format!("recording the doc's last write: {e}")))?;
    let env = |key: &str| std::env::var(key).ok();
    let by = By {
        kind: ByKind::Agent,
        name: identity::detect(args.as_name.as_deref(), &env).map(|w| w.name),
        via: Some(Via::Cli),
    };
    let turn = replay(&held.parsed().lines)
        .iter()
        .rev()
        .find(|c| c.name == by.name && c.end.is_none())
        .map(|c| c.n);
    let op = Op::Comment {
        at,
        verdict,
        body: Some(body),
        to,
        kind: None,
        turn,
    };
    let ctx = AppendCtx {
        now_ms: now,
        by,
        cid: None,
    };
    let landed = held
        .append_with(ctx, |_, _| Ok(vec![op]))
        .map_err(|e| match e {
            AppendError::Io(e) => failed(format!("writing the review file: {e}")),
            AppendError::Rejected(why) => {
                failed(format!("the review file refused the note: {why:?}"))
            }
        })?;
    landed
        .first()
        .copied()
        .ok_or_else(|| failed("the note was not written"))
}

/// `N` or `#N`, a line number.
fn parse_to(raw: &str) -> Result<u32, Refusal> {
    raw.strip_prefix('#')
        .unwrap_or(raw)
        .parse::<u32>()
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| usage(format!("--to takes a note's number, N or #N (not {raw:?})")))
}

/// Whether line `n` is a note nobody took back.
fn is_live_note(lines: &[Line], n: u32) -> bool {
    let note = lines
        .iter()
        .any(|l| l.n == n && matches!(l.op, Op::Comment { .. }));
    let retracted = lines
        .iter()
        .any(|l| matches!(l.op, Op::Retract { to } if to == n));
    note && !retracted
}

/// The note from the argument, or else from stdin, as the log will keep it.
fn read_body(arg: Option<&str>) -> Result<String, Refusal> {
    let raw = match arg {
        Some(body) => body.to_string(),
        None => {
            if io::stdin().is_terminal() {
                return Err(usage(
                    "comment reads the note from stdin; pass it as an argument",
                ));
            }
            let mut input = Vec::new();
            io::stdin()
                .read_to_end(&mut input)
                .map_err(|e| failed(format!("reading stdin: {e}")))?;
            String::from_utf8(input).map_err(|_| usage("stdin is not UTF-8"))?
        }
    };
    let body: String = raw
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| matches!(c, '\n' | '\t') || !matches!(c, '\0'..='\x1f'))
        .collect();
    let body = body.trim_end();
    if body.trim().is_empty() {
        return Err(usage("the note is empty"));
    }
    if body.len() > MAX_BODY {
        return Err(usage(format!(
            "body too long: {} bytes, the most is {MAX_BODY}",
            body.len()
        )));
    }
    Ok(body.to_string())
}

/// Where the note goes on `doc`, the text on disk.
fn place(doc: &Doc, target: &Target, name: &str) -> Result<At, Refusal> {
    let req = match target {
        Target::Doc => AnchorReq::Doc,
        Target::Line(n) => {
            let stop = block_at(doc, *n, name)?;
            AnchorReq::Block {
                sp: stop.to_string(),
                scope: BlockScope::Block,
            }
        }
        Target::Quote { q, hint } => quote_req(doc, q, *hint, name)?,
    };
    anchor_from_req(&req, doc, &doc.rev).map_err(|why| failed(format!("can't anchor: {why:?}")))
}

/// The `sp` of the innermost stop holding line `n`.
fn block_at<'a>(doc: &'a Doc, n: u32, name: &str) -> Result<&'a str, Refusal> {
    let count = doc.lines.count();
    if n == 0 || n > count {
        return Err(usage(format!("line {n} is outside {name} (1-{count})")));
    }
    let stops = &doc.blocks.stops;
    let holder = stops
        .iter()
        .filter(|s| s.lines.0 <= n && n <= s.lines.1)
        .max_by_key(|s| (s.lines.0, std::cmp::Reverse(s.lines.1)));
    if let Some(stop) = holder {
        return Ok(&stop.sp);
    }
    let before = stops
        .iter()
        .filter(|s| s.lines.1 < n)
        .max_by_key(|s| s.lines.0);
    let after = stops.iter().find(|s| s.lines.0 > n);
    let near: Vec<String> = [before, after]
        .into_iter()
        .flatten()
        .map(|s| format!("line {} {}", s.lines.0, excerpt(doc, s.lines.0)))
        .collect();
    Err(quote_err(format!(
        "no block at line {n} in {name}; nearest: {}",
        near.join(", ")
    )))
}

/// The request for the one place `q` points at. A `hint` line picks among several.
fn quote_req(doc: &Doc, q: &str, hint: Option<u32>, name: &str) -> Result<AnchorReq, Refusal> {
    let hits = anchor::search_all(&doc.text, q);
    let span = |hit: &Cand| {
        let last = hit.bytes.end.max(hit.bytes.start + 1) - 1;
        (doc.lines.line_of(hit.bytes.start), doc.lines.line_of(last))
    };
    let whole = |q: &str| AnchorReq::Quote {
        sp: None,
        q: q.to_string(),
        pre: String::new(),
        suf: String::new(),
        off: None,
        tl: None,
    };
    let hit = match (&hits[..], hint) {
        ([], _) => return Err(quote_err(format!("quote not found in {name}: {q:?}"))),
        ([_], _) => return Ok(whole(q)),
        (_, Some(line)) => {
            let far = |hit: &&Cand| {
                let (a, b) = span(hit);
                a.saturating_sub(line).max(line.saturating_sub(b))
            };
            let best = hits.iter().map(|h| far(&h)).min().unwrap_or(0);
            let nearest: Vec<&Cand> = hits.iter().filter(|h| far(h) == best).collect();
            match nearest[..] {
                [hit] => hit,
                _ => return Err(ambiguous(doc, q, &hits, name)),
            }
        }
        (_, None) => return Err(ambiguous(doc, q, &hits, name)),
    };
    let Some(sp) = &hit.sp else {
        // Raw HTML has no owner to point into; the constructor places it
        return Ok(whole(q));
    };
    let text: Vec<char> = anchor::owner_text(&doc.text, sp)
        .unwrap_or_default()
        .chars()
        .collect();
    // A place the page can't show is no pick; never a guessed anchor
    let (a, b) = anchor::locate(&doc.text, hit.bytes.clone())
        .filter(|loc| loc.sp == *sp && loc.off.0 < loc.off.1)
        .map(|loc| loc.off)
        .ok_or_else(|| ambiguous(doc, q, &hits, name))?;
    let slice = |r: std::ops::Range<usize>| text.get(r).unwrap_or_default().iter().collect();
    Ok(AnchorReq::Quote {
        sp: Some(sp.clone()),
        q: slice(a..b),
        pre: slice(a.saturating_sub(CONTEXT)..a),
        suf: slice(b..(b + CONTEXT).min(text.len())),
        off: Some(a),
        tl: Some(text.len()),
    })
}

fn ambiguous(doc: &Doc, q: &str, hits: &[Cand], name: &str) -> Refusal {
    let mut lines = vec![format!(
        "{q:?} matches {} places in {name}; quote more of it, or add --line N to pick one:",
        hits.len()
    )];
    for (i, hit) in hits.iter().take(PLACES).enumerate() {
        let line = doc.lines.line_of(hit.bytes.start);
        lines.push(format!("  {}) line {line} {}", i + 1, excerpt(doc, line)));
    }
    if hits.len() > PLACES {
        lines.push(format!("  and {} more", hits.len() - PLACES));
    }
    quote_err(lines.join("\n"))
}

/// Line `line`'s text, trimmed, cut short and fit for a terminal.
fn excerpt(doc: &Doc, line: u32) -> String {
    doc.lines
        .range(line)
        .map(|r| one_line(&doc.text[r].trim().chars().take(EXCERPT).collect::<String>()))
        .unwrap_or_default()
}
