//! `birta respond`: checks a parsed heredoc against the round this agent holds and the
//! doc as it is on disk, then records it. All of it runs under the review file's lock,
//! and a refusal writes nothing.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, IsTerminal, Read};
use std::path::Path;

use super::identity;
use super::render;
use super::respond::{self, Entry};
use super::{ExitCode, RespondArgs, wait};
use crate::anchor;
use crate::review::chain::Chain;
use crate::review::doc::Doc;
use crate::review::fold::{self, RoundKind, TurnIndex};
use crate::review::log::{AppendCtx, AppendError, Held, Log};
use crate::review::ops::{AnchorReq, At, By, ByKind, Line, Op, ReplyKind, Verdict, Via};
use crate::review::state::{ItemStatus, ReviewState};
use crate::review::sync::sync_bytes;
use crate::review::{Reject, anchor_from_req, review_path_for};

/// Places an ambiguous quote's error lists.
const PLACES: usize = 5;
/// Code points of a place's line shown beside it.
const EXCERPT: usize = 60;

pub fn run(args: &RespondArgs) -> ExitCode {
    if io::stdin().is_terminal() {
        eprintln!("respond reads stdin; use the heredoc from `birta review`");
        return ExitCode::Usage;
    }
    let mut input = Vec::new();
    if let Err(e) = io::stdin().read_to_end(&mut input) {
        eprintln!("error: reading stdin: {e}");
        return ExitCode::Error;
    }
    let Ok(input) = String::from_utf8(input) else {
        eprintln!("respond: stdin is not UTF-8");
        return ExitCode::Usage;
    };
    let entries = match respond::parse(&input) {
        Ok(entries) => entries,
        Err(errors) => {
            for e in errors {
                eprintln!("respond: {e}");
            }
            return ExitCode::Usage;
        }
    };
    let env = |key: &str| std::env::var(key).ok();
    let by = By {
        kind: ByKind::Agent,
        name: identity::detect(args.as_name.as_deref(), &env).map(|w| w.name),
        via: Some(Via::Cli),
    };
    match apply(&args.file, &entries, &by, wait::now_ms()) {
        Ok(Finish { code, out, err }) => {
            for line in err {
                eprintln!("{line}");
            }
            if let Some(out) = out {
                println!("{out}");
            }
            code
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::Error
        }
    }
}

/// How a respond ended: the exit code, the summary for stdout and lines for stderr.
struct Finish {
    code: ExitCode,
    out: Option<String>,
    err: Vec<String>,
}

impl Finish {
    fn refused(code: ExitCode, err: Vec<String>) -> Self {
        Self {
            code,
            out: None,
            err,
        }
    }
}

/// A claim that opened a turn, and what became of it.
struct ClaimRec {
    n: u32,
    name: Option<String>,
    /// The submits it took.
    rounds: Vec<u32>,
    end: Option<End>,
}

enum End {
    Yielded,
    Released { at: u64 },
}

/// What this respond answers.
enum Mode {
    /// The agent's own open turn.
    Held { claim: u32 },
    /// A due hand-back nobody holds, which this respond claims first.
    Pickup { round: u32 },
    /// The agent's turn that already ended; nothing it writes re-opens it.
    Late { claim: u32, released: Option<u64> },
}

fn apply(file: &Path, entries: &[(usize, Entry)], by: &By, now: u64) -> anyhow::Result<Finish> {
    let doc_path = file
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("not found: {}", file.display()))?;
    let name = file.to_string_lossy().into_owned();
    let mut held = match Log::open(&review_path_for(&doc_path)).hold() {
        Ok(held) => held,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Finish::refused(
                ExitCode::Stale,
                vec!["respond: nothing handed back to respond to".to_string()],
            ));
        }
        Err(e) => return Err(anyhow::Error::new(e).context("locking the review file")),
    };
    // Read once under the lock: the quotes are checked against the bytes recorded
    let bytes = std::fs::read(&doc_path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| anyhow::anyhow!("{name} is not UTF-8"))?;
    let doc = Doc::new(text);

    let parsed = held.parsed().clone();
    let index = fold::scan(&parsed, now);
    let claims = replay(&parsed.lines);
    let mode = match pick(&parsed.lines, &index, &claims, by, &doc, now) {
        Ok(mode) => mode,
        Err(refusal) => return Ok(refusal),
    };
    let state = fold::fold(&parsed, &Chain::build(&parsed), &doc).state;
    // A released claim no longer holds its notes, so they are named by their rounds
    let (holder, rounds) = match &mode {
        Mode::Held { claim }
        | Mode::Late {
            claim,
            released: None,
        } => (Some(*claim), Vec::new()),
        Mode::Late { claim, .. } => (
            None,
            claims
                .iter()
                .find(|c| c.n == *claim)
                .map(|c| c.rounds.clone())
                .unwrap_or_default(),
        ),
        Mode::Pickup { round } => (None, pending_through(&index, &claims, *round)),
    };
    let valid = valid_ids(&state, holder, &rounds);

    let checked = check(entries, &valid, &doc, &name);
    if !checked.errors.is_empty() {
        let code = if checked.bad_note {
            ExitCode::Usage
        } else {
            ExitCode::Quote
        };
        let err = checked
            .errors
            .iter()
            .map(|e| format!("respond: {e}"))
            .collect();
        return Ok(Finish::refused(code, err));
    }

    sync_bytes(&mut held, &doc_path, &bytes, now);
    let ctx = AppendCtx {
        now_ms: now,
        by: by.clone(),
        cid: None,
    };
    let turn = match mode {
        Mode::Held { claim } | Mode::Late { claim, .. } => claim,
        Mode::Pickup { round } => claim_round(&mut held, ctx.clone(), round)?,
    };
    let late = matches!(mode, Mode::Late { .. });
    let mut ops = to_ops(entries, &checked.anchors, turn, late);
    if !late {
        ops.push(Op::Yield {
            turn: Some(turn),
            release: false,
        });
    }
    held.append_with(ctx, |_, _| Ok(ops)).map_err(append_err)?;

    let tally = tally(entries);
    if let Mode::Late { released, .. } = mode {
        let why = released
            .map(|at| format!(" · your claim was released {}", render::hhmm(at)))
            .unwrap_or_default();
        return Ok(Finish {
            code: ExitCode::Ok,
            out: Some(format!("recorded late: {tally}{why}")),
            err: Vec::new(),
        });
    }
    let after = held.parsed().clone();
    drop(held);
    let state = fold::fold(&after, &Chain::build(&after), &doc).state;
    let err = state
        .items
        .iter()
        .filter(|i| valid.contains(&i.n) && i.status == ItemStatus::NotHandled)
        .map(|i| format!("warning: not handled: #{}", i.n))
        .collect();
    Ok(Finish {
        code: ExitCode::Ok,
        out: Some(format!("recorded: {tally} · yielded")),
        err,
    })
}

/// Which turn this respond is for, or why there is none.
fn pick(
    lines: &[Line],
    index: &TurnIndex,
    claims: &[ClaimRec],
    by: &By,
    doc: &Doc,
    now: u64,
) -> Result<Mode, Finish> {
    let mine = || claims.iter().rev().filter(|c| c.name == by.name);
    if let Some(c) = mine().find(|c| c.end.is_none()) {
        return Ok(Mode::Held { claim: c.n });
    }
    let last = mine().next();
    let late = |c: &ClaimRec| Mode::Late {
        claim: c.n,
        released: match c.end {
            Some(End::Released { at }) => Some(at),
            _ => None,
        },
    };
    let kind = |n: u32| index.rounds.iter().find(|r| r.n == n).map(|r| r.kind);
    if let Some(round) = index
        .claimable_round(now)
        .filter(|&r| kind(r) == Some(RoundKind::Submit))
    {
        // Taken back from this agent: its answers come late, it doesn't pick up again
        return Ok(match last {
            Some(c) if matches!(c.end, Some(End::Released { .. })) && c.rounds.contains(&round) => {
                late(c)
            }
            _ => Mode::Pickup { round },
        });
    }
    let refuse = |code, msg: String| Err(Finish::refused(code, vec![format!("respond: {msg}")]));
    let latest = index
        .rounds
        .iter()
        .rev()
        .find(|r| r.withdrawn.is_none() && now >= r.not_before);
    if let Some(approve) = latest.filter(|r| r.kind == RoundKind::Approve) {
        let approved = lines.iter().find_map(|l| match &l.op {
            Op::Approve { rev, .. } if l.n == approve.n => Some(rev),
            _ => None,
        });
        return if approved == Some(&doc.rev) {
            refuse(
                ExitCode::Stale,
                "approved; nothing to respond to".to_string(),
            )
        } else {
            refuse(
                ExitCode::ApprovedChanged,
                "approved, and the doc changed since; nothing to respond to".to_string(),
            )
        };
    }
    if let Some(c) = last {
        return Ok(late(c));
    }
    if let Some(other) = claims.iter().rev().find(|c| c.end.is_none()) {
        let name = other.name.as_deref().unwrap_or("someone");
        return refuse(ExitCode::Stale, format!("{name} holds this round"));
    }
    let msg = if index.rounds.is_empty() {
        "nothing handed back to respond to"
    } else {
        "nothing handed back yet"
    };
    refuse(ExitCode::Stale, msg.to_string())
}

/// Every claim that opened a turn, with the submits it took and how it ended, the way
/// the fold reads them.
fn replay(lines: &[Line]) -> Vec<ClaimRec> {
    let mut index = TurnIndex::default();
    let mut claims: Vec<ClaimRec> = Vec::new();
    // Submit line to the claim holding it
    let mut taker: HashMap<u32, u32> = HashMap::new();
    for line in lines {
        index.step(line);
        match &line.op {
            Op::Claim { round } if index.opens_turn(*round, line.n) => {
                let rounds: Vec<u32> = index
                    .rounds
                    .iter()
                    .filter(|r| r.kind == RoundKind::Submit && r.n <= *round)
                    .filter(|r| r.withdrawn.is_none() && !taker.contains_key(&r.n))
                    .map(|r| r.n)
                    .collect();
                for &r in &rounds {
                    taker.insert(r, line.n);
                }
                claims.push(ClaimRec {
                    n: line.n,
                    name: line.by.name.clone(),
                    rounds,
                    end: None,
                });
            }
            Op::Yield {
                turn: Some(turn),
                release: true,
            } if line.by.kind == ByKind::Human => {
                if let Some(c) = claims.iter_mut().find(|c| c.n == *turn && c.end.is_none()) {
                    c.end = Some(End::Released {
                        at: line.ts.unwrap_or(0),
                    });
                    taker.retain(|_, holder| *holder != *turn);
                }
            }
            Op::Yield { turn, .. } if line.by.kind != ByKind::Human => {
                let ended = claims.iter_mut().rev().find(|c| {
                    c.end.is_none() && c.name == line.by.name && turn.is_none_or(|turn| c.n == turn)
                });
                if let Some(c) = ended {
                    c.end = Some(End::Yielded);
                }
            }
            _ => {}
        }
    }
    claims
}

/// The submits a claim of `round` would take: up to it, standing, and held by nobody.
fn pending_through(index: &TurnIndex, claims: &[ClaimRec], round: u32) -> Vec<u32> {
    let held: Vec<u32> = claims
        .iter()
        .filter(|c| !matches!(c.end, Some(End::Released { .. })))
        .flat_map(|c| c.rounds.iter().copied())
        .collect();
    index
        .rounds
        .iter()
        .filter(|r| r.kind == RoundKind::Submit && r.n <= round && r.withdrawn.is_none())
        .filter(|r| !held.contains(&r.n))
        .map(|r| r.n)
        .collect()
}

/// The notes a respond may name: exactly those the round view lists.
fn valid_ids(state: &ReviewState, claim: Option<u32>, rounds: &[u32]) -> BTreeSet<u32> {
    state
        .items
        .iter()
        .filter(|i| fold::listed(i, claim, rounds, false))
        .map(|i| i.n)
        .collect()
}

#[derive(Default)]
struct Checked {
    errors: Vec<String>,
    /// Set by a note outside the round, which is a usage error before any quote error.
    bad_note: bool,
    /// Anchors for the `ask` lines, by input line.
    anchors: HashMap<usize, At>,
}

/// Checks every entry's note and quote. The quotes are resolved on `doc`, the text on disk.
fn check(entries: &[(usize, Entry)], valid: &BTreeSet<u32>, doc: &Doc, name: &str) -> Checked {
    let mut out = Checked::default();
    let ids = valid
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    for (line, entry) in entries {
        let n = match entry {
            Entry::Done { n, .. }
            | Entry::Answer { n, .. }
            | Entry::Didnt { n, .. }
            | Entry::Covers { n, .. } => Some(*n),
            _ => None,
        };
        if let Some(n) = n.filter(|n| !valid.contains(n)) {
            out.bad_note = true;
            out.errors.push(format!(
                "line {line}: #{n} is not part of your round (valid: {ids})"
            ));
        }
        let quote = match entry {
            Entry::Covers { quote, .. }
            | Entry::Ask { quote, .. }
            | Entry::Unasked { quote, .. } => quote,
            _ => continue,
        };
        if let Err(e) = unique(doc, quote, name) {
            out.errors.push(format!("line {line}: {e}"));
            continue;
        }
        if let Entry::Ask { .. } = entry {
            let req = AnchorReq::Quote {
                sp: None,
                q: quote.clone(),
                pre: String::new(),
                suf: String::new(),
                off: None,
                tl: None,
            };
            match anchor_from_req(&req, doc, &doc.rev) {
                Ok(at) => {
                    out.anchors.insert(*line, at);
                }
                Err(why) => out
                    .errors
                    .push(format!("line {line}: can't anchor {quote:?}: {why:?}")),
            }
        }
    }
    out
}

/// A quote found exactly once in the current text, the way `comment` finds one.
fn unique(doc: &Doc, quote: &str, name: &str) -> Result<(), String> {
    let hits = anchor::search(&doc.text, quote);
    match hits.len() {
        1 => Ok(()),
        0 => Err(format!("quote not found in the current text: {quote:?}")),
        count => {
            let places: Vec<String> = hits
                .iter()
                .take(PLACES)
                .enumerate()
                .map(|(i, hit)| {
                    let line = doc.lines.line_of(hit.bytes.start);
                    let excerpt = doc
                        .lines
                        .range(line)
                        .map(|r| doc.text[r].trim().chars().take(EXCERPT).collect::<String>())
                        .unwrap_or_default();
                    format!("{}) line {line} {excerpt}", i + 1)
                })
                .collect();
            Err(format!(
                "{quote:?} matches {count} places in {name}: {}  (quote more of it)",
                places.join(" ")
            ))
        }
    }
}

/// Claims `round` for this respond; the claim's line is the turn the lines belong to.
fn claim_round(held: &mut Held, ctx: AppendCtx, round: u32) -> anyhow::Result<u32> {
    let landed = held
        .append_with(ctx, |parsed, now| {
            match fold::scan(parsed, now).claimable_round(now) {
                Some(r) if r == round => Ok(vec![Op::Claim { round }]),
                _ => Err(Reject::AlreadyClaimed),
            }
        })
        .map_err(append_err)?;
    landed
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("the claim was not written"))
}

fn append_err(e: AppendError) -> anyhow::Error {
    match e {
        AppendError::Io(e) => anyhow::Error::new(e).context("writing the review file"),
        AppendError::Rejected(why) => anyhow::anyhow!("the review file refused the write: {why:?}"),
    }
}

/// The log ops for the heredoc, in its order. A `done` with a reason is a `done` and a
/// note reply.
fn to_ops(
    entries: &[(usize, Entry)],
    anchors: &HashMap<usize, At>,
    turn: u32,
    late: bool,
) -> Vec<Op> {
    let reply = |to: u32, kind, body: &str| Op::Reply {
        to,
        verdict: None,
        body: Some(body.to_string()),
        kind: Some(kind),
        late,
    };
    let mut ops = Vec::new();
    for (line, entry) in entries {
        match entry {
            Entry::Done { n, note } => {
                ops.push(Op::Done { to: *n, turn, late });
                if let Some(note) = note {
                    ops.push(reply(*n, ReplyKind::Note, note));
                }
            }
            Entry::Answer { n, body } => ops.push(reply(*n, ReplyKind::Answer, body)),
            Entry::Didnt { n, body } => ops.push(reply(*n, ReplyKind::Didnt, body)),
            Entry::Covers { n, quote } => ops.push(Op::Covers {
                to: *n,
                quote: quote.clone(),
                turn,
                late,
            }),
            Entry::Ask { body, .. } => ops.push(Op::Comment {
                at: anchors.get(line).cloned().unwrap_or(At::Doc),
                verdict: Some(Verdict::Ask),
                body: Some(body.clone()),
                to: None,
                kind: None,
                turn: Some(turn),
            }),
            Entry::Unasked { quote, body } => ops.push(Op::Unasked {
                quote: quote.clone(),
                body: body.clone(),
                turn,
                late,
            }),
            Entry::TurnNote { body } => ops.push(Op::TurnNote {
                body: body.clone(),
                turn,
                late,
            }),
        }
    }
    ops
}

/// `2 done · 1 answered · …` for the entries recorded.
fn tally(entries: &[(usize, Entry)]) -> String {
    let count = |f: fn(&Entry) -> bool| entries.iter().filter(|(_, e)| f(e)).count();
    let parts = [
        (count(|e| matches!(e, Entry::Done { .. })), "done"),
        (count(|e| matches!(e, Entry::Answer { .. })), "answered"),
        (count(|e| matches!(e, Entry::Didnt { .. })), "didn't"),
        (count(|e| matches!(e, Entry::Covers { .. })), "cited"),
        (count(|e| matches!(e, Entry::Unasked { .. })), "unasked"),
        (count(|e| matches!(e, Entry::Ask { .. })), "asked"),
        (count(|e| matches!(e, Entry::TurnNote { .. })), "turn note"),
    ];
    parts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect::<Vec<_>>()
        .join(" · ")
}
