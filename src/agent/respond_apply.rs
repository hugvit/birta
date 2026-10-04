//! `birta respond`: checks a parsed heredoc against the round this agent holds and the
//! doc as it is on disk, then records it. All of it runs under the review file's lock,
//! and a refusal writes nothing.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::Context;

use super::identity;
use super::render::{self, one_line};
use super::respond::{self, Entry};
use super::review_cmd::{answer_ids, own_name};
use super::{CONTROL_PATH, ExitCode, RespondArgs, hidden_in, relative, say, wait, warn};
use crate::anchor;
use crate::paths::Roots;
use crate::review::chain::Chain;
use crate::review::doc::Doc;
use crate::review::fold::{self, RoundKind, TurnIndex};
use crate::review::log::{AppendCtx, AppendError, Held, Log};
use crate::review::ops::{AnchorReq, At, By, ByKind, Line, Op, Parsed, ReplyKind, Verdict, Via};
use crate::review::state::{AskedKind, AtKind, Attr, Change, ItemStatus, ReviewState};
use crate::review::sync::{pending_write, read_doc, try_sync_bytes};
use crate::review::{Reject, anchor_from_req, review_path_for};
use crate::scan::{self, ScanBudget};

/// Places an ambiguous quote's error lists.
const PLACES: usize = 5;
/// Code points of a place's line shown beside it.
const EXCERPT: usize = 60;
/// Folders above the doc an old folder claim, or a batch's other files, are looked for in.
pub(super) const WALK_UP: usize = 3;

pub fn run(args: &RespondArgs) -> ExitCode {
    if io::stdin().is_terminal() {
        warn("respond reads stdin; use the heredoc from `birta review`");
        return ExitCode::Usage;
    }
    let mut input = Vec::new();
    if let Err(e) = io::stdin().read_to_end(&mut input) {
        warn(&format!("error: reading stdin: {e}"));
        return ExitCode::Error;
    }
    let Ok(input) = String::from_utf8(input) else {
        warn("respond: stdin is not UTF-8");
        return ExitCode::Usage;
    };
    let entries = match respond::parse(&input) {
        Ok(entries) => entries,
        Err(errors) => {
            for e in errors {
                warn(&format!("respond: {e}"));
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
                warn(&line);
            }
            if let Some(out) = out {
                say(&out);
            }
            code
        }
        Err(e) => {
            warn(&format!("error: {e:#}"));
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
pub(super) struct ClaimRec {
    pub(super) n: u32,
    pub(super) name: Option<String>,
    /// The submits it took.
    rounds: Vec<u32>,
    pub(super) end: Option<End>,
}

pub(super) enum End {
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
    if hidden_in(file) {
        anyhow::bail!(CONTROL_PATH);
    }
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
    let bytes = read_doc(&doc_path).map_err(|e| anyhow::anyhow!("reading {name}: {e}"))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| anyhow::anyhow!("{name} is not UTF-8"))?;
    let doc = Doc::new(text);

    let parsed = held.parsed().clone();
    let index = fold::scan(&parsed, now);
    let claims = replay(&parsed.lines);
    let mode = match pick(&parsed.lines, &index, &claims, by, &doc, now) {
        Ok(mode) => mode,
        Err(refusal) => return Ok(refusal),
    };
    let chain = Chain::build(&parsed);
    let folded = fold::fold(&parsed, &chain, &doc);
    let state = &folded.state;
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
    let valid = valid_ids(state, holder, &rounds);
    let round = match &mode {
        Mode::Held { claim } | Mode::Late { claim, .. } => parsed
            .lines
            .iter()
            .find_map(|l| match l.op {
                Op::Claim { round, .. } if l.n == *claim => Some(round),
                _ => None,
            })
            .unwrap_or(0),
        Mode::Pickup { round } => *round,
    };
    let answers = answer_ids(state, &folded.turn_notes, &index, round);
    let removed = removed_by(&parsed, state, &mode, text);

    let checked = check(
        entries,
        &Ids {
            notes: &valid,
            answers: &answers,
        },
        &removed,
        &doc,
        &name,
    );
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

    let ctx = AppendCtx {
        now_ms: now,
        by: by.clone(),
        cid: None,
    };
    let late = matches!(mode, Mode::Late { .. });
    // The turn as the log would read with this respond in it, folded before anything is
    // written; a late respond only adds lines to a turn that already ended
    let (turn, after) = match &mode {
        Mode::Late { claim, .. } => (*claim, state.clone()),
        Mode::Held { .. } | Mode::Pickup { .. } => {
            let (turn, claim) = match &mode {
                Mode::Pickup { round } => (
                    held.next_line(),
                    vec![Op::Claim {
                        round: *round,
                        target: own_name(&doc_path),
                    }],
                ),
                Mode::Held { claim } | Mode::Late { claim, .. } => (*claim, Vec::new()),
            };
            let write = AppendCtx {
                now_ms: now,
                by: By {
                    kind: ByKind::Birta,
                    ..By::default()
                },
                cid: None,
            };
            let would = held
                .preview(vec![
                    (ctx.clone(), claim),
                    (write, pending_write(held.parsed(), text)),
                    (ctx.clone(), to_ops(entries, &checked.anchors, turn, false)),
                ])
                .context("reading the review file as it would be")?;
            (turn, fold::fold(&would, &Chain::build(&would), &doc).state)
        }
    };
    let unresolved: Vec<String> = checked
        .ambiguous
        .iter()
        .filter(|a| !in_turn(&after, turn, &doc, &a.quote))
        .map(|a| format!("respond: line {}: {}", a.line, a.msg))
        .collect();
    if !unresolved.is_empty() {
        return Ok(Finish::refused(ExitCode::Quote, unresolved));
    }
    if !late {
        // A cut of whitespace alone can't be quoted, so it can't hold the turn up
        let missing: Vec<&Change> = uncited(&after, turn)
            .into_iter()
            .filter(|c| c.what.new_lines > 0 || quote_for(c, &doc).is_some())
            .collect();
        if !missing.is_empty() {
            let reminders = doc_notes(state, &valid);
            return Ok(Finish::refused(
                ExitCode::Quote,
                refusal(&missing, &reminders, &doc),
            ));
        }
    }

    // Claimed first, so the write it records is this turn's
    let turn = match mode {
        Mode::Held { claim } | Mode::Late { claim, .. } => claim,
        Mode::Pickup { round } => claim_round(&mut held, ctx.clone(), round, &doc_path)?,
    };
    try_sync_bytes(&mut held, &doc_path, &bytes, now).context("recording the doc's last write")?;
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
    let err: Vec<String> = state
        .items
        .iter()
        .filter(|i| valid.contains(&i.n) && i.status == ItemStatus::NotHandled)
        .map(|i| format!("warning: not handled: #{}", i.n))
        .collect();
    let next = waits_on(&after.lines, turn, &doc_path, &name, by);
    Ok(Finish {
        code: ExitCode::Ok,
        out: Some(format!(
            "recorded: {tally} · yielded\nnext: birta review {next} --wait"
        )),
        err,
    })
}

/// This turn's changes that no note asked for and no `unasked` line names. Changes that
/// render the same never reach the fold's list. One Claude wrote over again in the same
/// turn is the later change's, as the news list counts it; one someone else wrote over
/// has no words of its own left to quote.
fn uncited(state: &ReviewState, turn: u32) -> Vec<&Change> {
    state
        .changes
        .iter()
        .filter(|c| c.turn == Some(turn) && c.attr == Attr::Agent)
        .filter(|c| c.asked.kind == AskedKind::Unasked && !c.flagged && c.undone_by.is_none())
        .filter(|c| fold::overwritten(c, &state.changes).is_none())
        .collect()
}

/// Words around which to cite each side of a change with none of its own.
const AROUND: usize = 2;

/// The quote a line citing `change` would carry: its first words, or for written lines
/// with no words (a blank line in a code block), the words just before and after them,
/// which read across it. `None` for a cut of whitespace, which no quote can name.
fn quote_for(change: &Change, doc: &Doc) -> Option<String> {
    let own = first_words(words_of(change, doc));
    if !own.is_empty() {
        return Some(own);
    }
    if change.what.new_lines == 0 {
        return None;
    }
    let (a, b) = change.range.bytes;
    let before: Vec<&str> = doc
        .text
        .get(..a)?
        .split_whitespace()
        .rev()
        .take(AROUND)
        .collect();
    let after: Vec<&str> = doc.text.get(b..)?.split_whitespace().take(AROUND).collect();
    if before.is_empty() || after.is_empty() {
        return None;
    }
    let around: Vec<&str> = before.into_iter().rev().chain(after).collect();
    Some(first_words(&around.join(" "))).filter(|q| !q.is_empty())
}

/// The whole-doc notes of the round that a `covers` can cite, in note order.
fn doc_notes(state: &ReviewState, valid: &BTreeSet<u32>) -> Vec<u32> {
    state
        .items
        .iter()
        .filter(|i| valid.contains(&i.n) && i.at.kind == AtKind::Doc)
        .filter(|i| {
            !matches!(
                i.verdict,
                Some(Verdict::Ask | Verdict::Yes | Verdict::Stet | Verdict::Later)
            )
        })
        .map(|i| i.n)
        .collect()
}

/// The words a change wrote, or for a cut the words it took.
fn words_of<'a>(change: &'a Change, doc: &'a Doc) -> &'a str {
    if change.what.new_lines == 0 {
        return change.removed.as_deref().unwrap_or_default();
    }
    doc.text
        .get(change.range.bytes.0..change.range.bytes.1)
        .unwrap_or_default()
}

/// Why nothing was recorded, then the lines that would cite each change: a `covers` per
/// whole-doc note of the round as a reminder, quoting the first change that wrote words,
/// and an `unasked` per change.
fn refusal(missing: &[&Change], notes: &[u32], doc: &Doc) -> Vec<String> {
    let head = match missing.len() {
        1 => "respond: 1 change cites no note, so nothing was recorded. Add a line for it and run respond again:".to_string(),
        n => format!(
            "respond: {n} changes cite no note, so nothing was recorded. Add a line for each and run respond again:"
        ),
    };
    let mut out = vec![head];
    let wrote = missing
        .iter()
        .filter(|c| c.what.new_lines > 0)
        .find_map(|c| quote_for(c, doc));
    if let Some(words) = wrote {
        out.extend(notes.iter().map(|n| format!("#{n} covers \"{words}\"")));
    }
    out.extend(
        missing
            .iter()
            .filter_map(|c| quote_for(c, doc))
            .map(|q| format!("unasked \"{q}\": <why>")),
    );
    out
}

/// Whether `quote`, found in several places, reads in a change this turn made, so the
/// fold credits that change with it.
fn in_turn(state: &ReviewState, turn: u32, doc: &Doc, quote: &str) -> bool {
    let changed: Vec<(usize, usize)> = state
        .changes
        .iter()
        .filter(|c| c.turn == Some(turn) && c.attr == Attr::Agent && c.what.new_lines > 0)
        .map(|c| c.range.bytes)
        .collect();
    places(doc, quote, true)
        .iter()
        .any(|&(start, end)| changed.iter().any(|&(a, b)| start < b && a < end))
}

/// The byte ranges `quote` reads at in the doc, as the page finds them; with `cites`, in
/// the source, whitespace aside, when the page finds none.
fn places(doc: &Doc, quote: &str, cites: bool) -> Vec<(usize, usize)> {
    let hits: Vec<(usize, usize)> = anchor::search_all(&doc.text, quote)
        .iter()
        .map(|hit| (hit.bytes.start, hit.bytes.end))
        .collect();
    if hits.is_empty() && cites {
        return fold::quote_places(&doc.text, quote);
    }
    hits
}

/// The opening words of `text`'s first line with words on it, as a respond quote:
/// whole words up to 60 code points, with `\` and `"` escaped.
fn first_words(text: &str) -> String {
    let line = one_line(
        text.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default(),
    );
    let mut out = String::new();
    for word in line.split_whitespace() {
        let len = out.chars().count() + usize::from(!out.is_empty()) + word.chars().count();
        if !out.is_empty() && len > EXCERPT {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    let cut: String = out.chars().take(EXCERPT).collect();
    cut.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Text the turn's writes removed, which an `unasked` quote may name: its recorded
/// removals, and those of the write on disk not yet recorded.
fn removed_by(parsed: &Parsed, state: &ReviewState, mode: &Mode, text: &str) -> Vec<String> {
    let turn = match mode {
        Mode::Held { claim } | Mode::Late { claim, .. } => Some(*claim),
        Mode::Pickup { .. } => None,
    };
    let recorded = state
        .changes
        .iter()
        .filter(|c| turn.is_some() && c.turn == turn && c.attr == Attr::Agent)
        .filter(|c| c.what.new_lines == 0)
        .filter_map(|c| c.removed.clone());
    let pending = Chain::build(parsed)
        .record(text)
        .and_then(|r| r.patch)
        .into_iter()
        .flatten()
        .filter(|h| h.i.is_empty())
        .map(|h| h.d.concat());
    recorded.chain(pending).collect()
}

/// What the agent waits on next, as one shell word: the folder its claim was made on,
/// else this doc as named.
fn waits_on(lines: &[Line], turn: u32, doc: &Path, name: &str, by: &By) -> String {
    let claim = lines.iter().find(|l| l.n == turn);
    let folder = match claim.map(|l| &l.op) {
        Some(Op::Claim {
            target: Some(target),
            ..
        }) => doc
            .parent()
            .and_then(|parent| parent.join(target).canonicalize().ok())
            .filter(|claimed| claimed.is_dir()),
        Some(Op::Claim {
            round,
            target: None,
        }) => {
            let batch = lines.iter().find_map(|l| match l.op {
                Op::Submit { batch, .. } if l.n == *round => batch,
                _ => None,
            });
            batch.and_then(|batch| claimed_with(doc, batch, claim?.ts, &by.name))
        }
        _ => None,
    };
    match folder {
        Some(folder) => render::shell_quote(&shown(&folder).to_string_lossy()),
        None => render::shell_quote(name),
    }
}

/// `path` from the working folder, or absolute when they share nothing but the root.
pub(super) fn shown(path: &Path) -> PathBuf {
    let Ok(cwd) = std::env::current_dir().and_then(|d| d.canonicalize()) else {
        return path.to_path_buf();
    };
    let shared = cwd
        .components()
        .zip(path.components())
        .take_while(|(a, b)| a == b)
        .count();
    if shared > 1 {
        relative(&cwd, path)
    } else {
        path.to_path_buf()
    }
}

/// For a folder claim from before claims named their folder: the nearest folder, a few
/// above the doc, holding another doc of `batch` this agent claimed in the same instant.
fn claimed_with(
    doc: &Path,
    batch: u64,
    ts: Option<u64>,
    agent: &Option<String>,
) -> Option<PathBuf> {
    let home = dirs::home_dir();
    let own = review_path_for(doc);
    for dir in doc.parent()?.ancestors().take(WALK_UP + 1) {
        if dir.parent().is_none() || home.as_deref() == Some(dir) {
            break;
        }
        let roots = Roots::new(dir.to_path_buf(), home.clone());
        let listed = scan::walk(dir, &roots, ScanBudget::default());
        let hit = listed
            .review_files
            .iter()
            .filter_map(|rel| dir.join(rel).canonicalize().ok())
            .filter(|log| *log != own)
            .any(|log| claimed_in(&log, batch, ts, agent));
        if hit {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// Whether `log` holds a claim by `agent` at `ts` of a submit in `batch`.
fn claimed_in(log: &Path, batch: u64, ts: Option<u64>, agent: &Option<String>) -> bool {
    let Ok(Some(parsed)) = Log::open(log).read() else {
        return false;
    };
    let rounds: HashSet<u32> = parsed
        .lines
        .iter()
        .filter(|l| matches!(l.op, Op::Submit { batch: Some(b), .. } if b == batch))
        .map(|l| l.n)
        .collect();
    parsed.lines.iter().any(|l| {
        matches!(l.op, Op::Claim { round, .. } if rounds.contains(&round))
            && l.ts == ts
            && l.by.name == *agent
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
        let claim = lines.iter().find(|l| l.n == other.n);
        let name = render::who(&claim.map(|l| l.by.clone()).unwrap_or_default());
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
/// the fold reads them. Linear in the log, so a hostile one can't stall the lock.
pub(super) fn replay(lines: &[Line]) -> Vec<ClaimRec> {
    let mut index = TurnIndex::default();
    let mut claims: Vec<ClaimRec> = Vec::new();
    // Claim line to its place in `claims`
    let mut at: HashMap<u32, usize> = HashMap::new();
    // Per agent name, the claims whose turn is still open
    let mut open: HashMap<Option<String>, BTreeSet<u32>> = HashMap::new();
    // Standing submits no claim holds
    let mut untaken: BTreeSet<u32> = BTreeSet::new();
    for line in lines {
        index.step(line);
        match &line.op {
            Op::Submit { .. } => {
                untaken.insert(line.n);
            }
            Op::Withdraw { to } => {
                let withdrawn = index
                    .rounds
                    .binary_search_by_key(to, |r| r.n)
                    .is_ok_and(|i| index.rounds[i].withdrawn == Some(line.n));
                if withdrawn {
                    untaken.remove(to);
                }
            }
            Op::Claim { round, .. } if index.opens_turn(*round, line.n) => {
                let later = untaken.split_off(&(round + 1));
                let rounds: Vec<u32> = std::mem::replace(&mut untaken, later).into_iter().collect();
                at.insert(line.n, claims.len());
                open.entry(line.by.name.clone()).or_default().insert(line.n);
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
                if let Some(c) = at.get(turn).map(|&i| &mut claims[i])
                    && c.end.is_none()
                {
                    c.end = Some(End::Released {
                        at: line.ts.unwrap_or(0),
                    });
                    // A submit swept into the claim may have been withdrawn since
                    let standing = |r: &u32| {
                        index
                            .rounds
                            .binary_search_by_key(r, |round| round.n)
                            .is_ok_and(|i| index.rounds[i].withdrawn.is_none())
                    };
                    untaken.extend(c.rounds.iter().copied().filter(standing));
                    if let Some(mine) = open.get_mut(&c.name) {
                        mine.remove(turn);
                    }
                }
            }
            Op::Yield { turn, .. } if line.by.kind != ByKind::Human => {
                let Some(mine) = open.get_mut(&line.by.name) else {
                    continue;
                };
                let ended = match turn {
                    Some(turn) => mine.contains(turn).then_some(*turn),
                    None => mine.last().copied(),
                };
                if let Some(n) = ended {
                    mine.remove(&n);
                    claims[at[&n]].end = Some(End::Yielded);
                }
            }
            _ => {}
        }
    }
    claims
}

/// The submits a claim of `round` would take: up to it, standing, and held by nobody.
fn pending_through(index: &TurnIndex, claims: &[ClaimRec], round: u32) -> Vec<u32> {
    let held: HashSet<u32> = claims
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
    /// `covers` and `unasked` quotes found in several places, which stand only if one of
    /// them reads in a change of the turn.
    ambiguous: Vec<Ambiguous>,
    /// Set by a note outside the round, which is a usage error before any quote error.
    bad_note: bool,
    /// Anchors for the `ask` lines, by input line.
    anchors: HashMap<usize, At>,
}

struct Ambiguous {
    line: usize,
    quote: String,
    /// The error to give when none of its places is a change of the turn.
    msg: String,
}

/// What a respond's `#N` may name.
struct Ids<'a> {
    /// The notes the round view lists.
    notes: &'a BTreeSet<u32>,
    /// The reviewer's answers it lists, which only `covers` cites.
    answers: &'a [u32],
}

/// Checks every entry's note and quote. The quotes are resolved on `doc`, the text on
/// disk; an `unasked` quote may name text in `removed` instead.
fn check(
    entries: &[(usize, Entry)],
    valid: &Ids,
    removed: &[String],
    doc: &Doc,
    name: &str,
) -> Checked {
    let mut out = Checked::default();
    let ids = valid
        .notes
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
        let covers = matches!(entry, Entry::Covers { .. });
        if let Some(n) = n.filter(|n| !valid.notes.contains(n)) {
            out.bad_note |= !covers || !valid.answers.contains(&n);
            if !valid.answers.contains(&n) {
                out.errors.push(format!(
                    "line {line}: #{n} is not part of your round (valid: {ids})"
                ));
            } else if !covers {
                out.errors.push(format!(
                    "line {line}: #{n} is your reviewer's answer; cite it with #{n} covers \"<new text>\""
                ));
            }
        }
        let quote = match entry {
            Entry::Covers { quote, .. }
            | Entry::Ask { quote, .. }
            | Entry::Unasked { quote, .. } => quote,
            _ => continue,
        };
        let gone = matches!(entry, Entry::Unasked { .. })
            && removed.iter().any(|r| fold::quotes_removed(quote, r));
        if gone {
            continue;
        }
        let cites = !matches!(entry, Entry::Ask { .. });
        match unique(doc, quote, name, cites) {
            Err(Found::Several(msg)) if !matches!(entry, Entry::Ask { .. }) => {
                out.ambiguous.push(Ambiguous {
                    line: *line,
                    quote: quote.clone(),
                    msg,
                });
                continue;
            }
            Err(Found::None(msg) | Found::Several(msg)) => {
                out.errors.push(format!("line {line}: {msg}"));
                continue;
            }
            Ok(()) => {}
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

/// Why a quote isn't found exactly once.
enum Found {
    None(String),
    Several(String),
}

/// A quote found exactly once in the current text, the way `comment` finds one.
/// With `cites`, a quote the page can't find still counts where it reads in the source,
/// whitespace aside, as the fold credits a `covers` or `unasked` quote.
fn unique(doc: &Doc, quote: &str, name: &str, cites: bool) -> Result<(), Found> {
    let hits = places(doc, quote, cites);
    match hits.len() {
        1 => Ok(()),
        0 => Err(Found::None(format!(
            "quote not found in the current text: {quote:?}"
        ))),
        count => {
            let places: Vec<String> = hits
                .iter()
                .take(PLACES)
                .enumerate()
                .map(|(i, &(start, _))| {
                    let line = doc.lines.line_of(start);
                    let excerpt = doc
                        .lines
                        .range(line)
                        .map(|r| {
                            one_line(&doc.text[r].trim().chars().take(EXCERPT).collect::<String>())
                        })
                        .unwrap_or_default();
                    format!("{}) line {line} {excerpt}", i + 1)
                })
                .collect();
            Err(Found::Several(format!(
                "{quote:?} matches {count} places in {name}: {}  (quote more of it)",
                places.join(" ")
            )))
        }
    }
}

/// Claims `round` for this respond; the claim's line is the turn the lines belong to.
fn claim_round(held: &mut Held, ctx: AppendCtx, round: u32, doc: &Path) -> anyhow::Result<u32> {
    let landed = held
        .append_with(ctx, |parsed, now| {
            match fold::scan(parsed, now).claimable_round(now) {
                Some(r) if r == round => Ok(vec![Op::Claim {
                    round,
                    target: own_name(doc),
                }]),
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::review::ops::Parsed;

    fn line(n: u32, kind: ByKind, op: Op) -> Line {
        Line {
            n,
            ts: Some(u64::from(n)),
            by: By {
                kind,
                name: (kind == ByKind::Agent).then(|| "claude".to_string()),
                via: None,
            },
            cid: None,
            op,
        }
    }

    #[test]
    fn replaying_30k_turns_takes_under_2_s() {
        let mut lines = Vec::new();
        for i in 0..30_000u32 {
            let n = 4 + i * 3;
            let submit = Op::Submit {
                upto: n - 1,
                not_before: 0,
                hold_ms: 0,
                to: None,
                batch: None,
            };
            lines.push(line(n, ByKind::Human, submit));
            lines.push(line(
                n + 1,
                ByKind::Agent,
                Op::Claim {
                    round: n,
                    target: None,
                },
            ));
            let end = Op::Yield {
                turn: Some(n + 1),
                release: false,
            };
            lines.push(line(n + 2, ByKind::Agent, end));
        }
        let parsed = Parsed {
            lines,
            ..Parsed::default()
        };
        let started = Instant::now();
        let index = fold::scan(&parsed, u64::MAX);
        let claims = replay(&parsed.lines);
        let pending = pending_through(&index, &claims, u32::MAX);
        let took = started.elapsed();
        assert_eq!(claims.len(), 30_000);
        assert!(pending.is_empty());
        assert!(took < Duration::from_secs(2), "took {took:?}");
    }
}
