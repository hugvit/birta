//! `birta review`: the round handed back on a doc or a folder, and the claim that picks
//! it up. A claim runs under the review file's lock, and the view prints after it's let go.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::{Value, json};

use super::identity::{self, Identity};
use super::render::{self, ApprovedView, ItemLine, RoundView, Stance, Target};
use super::{ExitCode, ReviewArgs, wait};
use crate::config::Config;
use crate::paths::{self, Roots};
use crate::review::chain::Chain;
use crate::review::doc::Doc;
use crate::review::fold::{self, RoundKind, TurnIndex};
use crate::review::log::{AppendCtx, AppendError, Held, Log};
use crate::review::ops::{Act, By, ByKind, Line, Op, Scope, Via};
use crate::review::state::{AtKind, AtState, Attr, Change, Item, ItemStatus, ReviewState};
use crate::review::{FOLDER_REVIEW, review_path_for};
use crate::scan::{self, ScanBudget};

/// How often a folder waiter lists its review files again.
const RELIST: Duration = Duration::from_secs(2);
/// Human writes the `YOU` lines name before `+N more`.
const YOU_LINES: usize = 5;
const REVIEW_SUFFIX: &str = ".birta.jsonl";

/// Whether this run picks the round up, or only looks. A person at a terminal with no
/// agent identity only looks; an agent's piped stdout picks up even without one.
pub fn should_claim(peek: bool, identity: Option<&Identity>, stdout_tty: bool) -> bool {
    !peek && (identity.is_some() || !stdout_tty)
}

/// Claims the latest due round on `doc` for `by`, if nobody has. Returns the claim's line.
pub fn claim(doc: &Path, by: &By, now_ms: u64) -> anyhow::Result<Option<u32>> {
    let claimed = claim_log(&Log::open(&review_path_for(doc)), Some(doc), by, now_ms)?;
    Ok(claimed.map(|(line, _)| line))
}

pub fn run(args: &ReviewArgs, config: &Config) -> ExitCode {
    match review(args, config) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::Error
        }
    }
}

/// A review file and what it reviews.
#[derive(Clone, Debug)]
struct Entry {
    log: PathBuf,
    /// `None` for a folder's own review file.
    doc: Option<PathBuf>,
    name: String,
    /// `name` as one shell word.
    path: String,
}

enum Place {
    File(Entry),
    Dir {
        root: PathBuf,
        name: String,
        /// Shell-quoted; `None` when no target was given.
        path: Option<String>,
    },
}

struct Opts {
    by: By,
    claims: bool,
    peek: bool,
    all: bool,
    json: bool,
}

/// What a view shows: a round, as whose, or an approve.
#[derive(Clone, Copy, Debug)]
enum Pick {
    Mine { claim: u32, round: u32 },
    Peek { round: u32 },
    ReadOnly { claim: u32, round: u32 },
    Approved { round: u32 },
}

/// One doc's part of the output.
struct Block {
    text: String,
    json: Value,
    changed: bool,
}

/// What a review file looks like on disk, to notice a change or a replacement.
type Stamp = Option<(u64, u64, u64, i64, i64)>;

fn review(args: &ReviewArgs, config: &Config) -> anyhow::Result<ExitCode> {
    let env = |key: &str| std::env::var(key).ok();
    let who = identity::detect(args.as_name.as_deref(), &env);
    let opts = Opts {
        by: By {
            kind: ByKind::Agent,
            name: who.as_ref().map(|w| w.name.clone()),
            via: Some(Via::Cli),
        },
        claims: should_claim(args.peek, who.as_ref(), io::stdout().is_terminal()),
        peek: args.peek,
        all: args.all,
        json: args.json,
    };
    let place = resolve(args.target.as_deref())?;
    let limit = wait::timeout(args.timeout);

    let out = match &place {
        Place::File(entry) => {
            if let (Some(drafter), Some(doc)) = (&args.drafted_by, &entry.doc) {
                create_drafted(entry, doc, drafter, config)?;
            }
            let found = if args.wait {
                wait::until(limit, file_poll(entry, &opts))?
            } else {
                look_file(entry, &opts, wait::now_ms(), false)?.map(|b| vec![b])
            };
            let path = Some(entry.path.as_str());
            output(found, &opts, &entry.name, path, args.wait, false)
        }
        Place::Dir { root, name, path } => {
            if args.drafted_by.is_some() {
                eprintln!("error: --drafted-by names a doc's drafter, so it needs a file");
                return Ok(ExitCode::Usage);
            }
            let found = if args.wait {
                wait::until(limit, dir_poll(root, name, path.as_deref(), &opts))?
            } else {
                let list = entries(root, name, path.as_deref());
                let now = wait::now_ms();
                match look_dir(&list, &opts, now)?.0 {
                    Some(blocks) => Some(blocks),
                    None => settled_dir(&list, &opts, now)?,
                }
            };
            output(found, &opts, name, path.as_deref(), args.wait, true)
        }
    };
    let (code, text) = out;
    // A reader that went away wants no more; nothing is left to do.
    let _ = io::stdout().lock().write_all(text.as_bytes());
    Ok(code)
}

/// The exit code and text for what was found: views, nothing, or a timeout.
fn output(
    found: Option<Vec<Block>>,
    opts: &Opts,
    name: &str,
    path: Option<&str>,
    waited: bool,
    dir: bool,
) -> (ExitCode, String) {
    let Some(blocks) = found else {
        let rerun = render::rerun(path);
        return match (waited, opts.json) {
            (true, false) => (ExitCode::Timeout, format!("{rerun}\n")),
            (true, true) => (
                ExitCode::Timeout,
                json_line(&json!({"status": "timeout", "rerun": format!("{rerun} --json")})),
            ),
            (false, false) => (ExitCode::Ok, render::nothing(name, path)),
            (false, true) => (
                ExitCode::Ok,
                json_line(&json!({"status": "nothing", "doc": name})),
            ),
        };
    };
    let code = if blocks.iter().any(|b| b.changed) {
        ExitCode::ApprovedChanged
    } else {
        ExitCode::Ok
    };
    let text = if !opts.json {
        blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else if let ([one], false) = (&blocks[..], dir) {
        json_line(&one.json)
    } else {
        let docs: Vec<&Value> = blocks.iter().map(|b| &b.json).collect();
        json_line(&json!({"status": "round", "docs": docs}))
    };
    (code, text)
}

fn json_line(value: &Value) -> String {
    format!("{value}\n")
}

fn resolve(target: Option<&Path>) -> anyhow::Result<Place> {
    let Some(target) = target else {
        let root = std::env::current_dir()?.canonicalize()?;
        return Ok(Place::Dir {
            root,
            name: ".".to_string(),
            path: None,
        });
    };
    let meta = fs::metadata(target).with_context(|| format!("not found: {}", target.display()))?;
    let typed = target.to_string_lossy();
    let typed = match typed.trim_end_matches('/') {
        "" => typed.to_string(),
        trimmed => trimmed.to_string(),
    };
    let canonical = target.canonicalize()?;
    let path = render::shell_quote(&typed);
    if meta.is_dir() {
        return Ok(Place::Dir {
            root: canonical,
            name: typed,
            path: Some(path),
        });
    }
    Ok(Place::File(Entry {
        log: review_path_for(&canonical),
        doc: Some(canonical),
        name: typed,
        path,
    }))
}

/// Makes the review file for a doc its drafter waits on, so the page shows who drafted
/// it from the first look. A file that exists keeps its owner.
fn create_drafted(entry: &Entry, doc: &Path, drafter: &str, config: &Config) -> anyhow::Result<()> {
    let text = fs::read_to_string(doc).with_context(|| format!("reading {}", entry.name))?;
    let doc_name = doc
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Log::open(&entry.log)
        .git_exclude(config.review.git_exclude)
        .create_if_missing(&doc_name, &text, Some(drafter))
        .context("creating the review file")?;
    Ok(())
}

// --- the claim ---

fn claim_log(
    log: &Log,
    doc: Option<&Path>,
    by: &By,
    now_ms: u64,
) -> anyhow::Result<Option<(u32, u32)>> {
    let mut held = match log.hold() {
        Ok(held) => held,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("locking the review file"),
    };
    claim_held(&mut held, doc, by, now_ms)
}

/// Under `held`'s lock: records any doc write nobody logged, then claims the latest due
/// round. Returns the claim's line and the round.
fn claim_held(
    held: &mut Held,
    doc: Option<&Path>,
    by: &By,
    now_ms: u64,
) -> anyhow::Result<Option<(u32, u32)>> {
    if let Some(doc) = doc {
        sync_external(held, doc);
    }
    let mut round = None;
    let ctx = AppendCtx {
        now_ms,
        by: by.clone(),
        cid: None,
    };
    let landed = held
        .append_with(ctx, |parsed, now| {
            round = fold::scan(parsed, now).claimable_round(now);
            Ok(round.map(|round| Op::Claim { round }).into_iter().collect())
        })
        .map_err(|e| match e {
            AppendError::Io(e) => anyhow::Error::new(e).context("writing the claim"),
            AppendError::Rejected(why) => {
                anyhow::anyhow!("the review file refused the claim: {why:?}")
            }
        })?;
    Ok(landed.first().copied().zip(round))
}

/// Records a write to the doc that isn't in the log yet. Nothing records one yet.
fn sync_external(_held: &mut Held, _doc: &Path) -> Option<u32> {
    None
}

// --- looking ---

/// The latest due round nobody can claim now: an approve, or a submit whose claim is live.
fn settled(index: &TurnIndex, now: u64, by: &By) -> Option<Pick> {
    let latest = index
        .rounds
        .iter()
        .rev()
        .find(|r| r.withdrawn.is_none() && now >= r.not_before)?;
    if latest.kind == RoundKind::Approve {
        return Some(Pick::Approved { round: latest.n });
    }
    let claim = latest.claim.filter(|_| latest.yielded.is_none())?;
    Some(if latest.claimed_by == by.name {
        Pick::Mine {
            claim,
            round: latest.n,
        }
    } else {
        Pick::ReadOnly {
            claim,
            round: latest.n,
        }
    })
}

fn kind_of(index: &TurnIndex, round: u32) -> RoundKind {
    index
        .rounds
        .iter()
        .find(|r| r.n == round)
        .map_or(RoundKind::Submit, |r| r.kind)
}

/// What one look at a doc finds. While `waiting`, only a round that can be claimed now
/// counts, and losing its claim to another agent means waiting on.
fn look_file(entry: &Entry, opts: &Opts, now: u64, waiting: bool) -> anyhow::Result<Option<Block>> {
    let log = Log::open(&entry.log);
    let Some(parsed) = log.read()? else {
        return Ok(None);
    };
    let index = fold::scan(&parsed, now);
    let pick = match index.claimable_round(now) {
        Some(round) if kind_of(&index, round) == RoundKind::Approve && !opts.claims => {
            Some(Pick::Approved { round })
        }
        Some(round) if !opts.claims => Some(Pick::Peek { round }),
        Some(_) => match claim_log(&log, entry.doc.as_deref(), &opts.by, now)? {
            Some((claim, round)) => Some(picked(&index, claim, round)),
            None if waiting => None,
            None => match log.read()? {
                Some(parsed) => settled(&fold::scan(&parsed, now), now, &opts.by),
                None => None,
            },
        },
        None if waiting => None,
        None => settled(&index, now, &opts.by),
    };
    pick.map(|pick| block(entry, pick, opts, now)).transpose()
}

fn picked(index: &TurnIndex, claim: u32, round: u32) -> Pick {
    match kind_of(index, round) {
        RoundKind::Approve => Pick::Approved { round },
        RoundKind::Submit => Pick::Mine { claim, round },
    }
}

/// The review files under `root`, the folder's own first.
fn entries(root: &Path, name: &str, path: Option<&str>) -> Vec<Entry> {
    let roots = Roots::new(root.to_path_buf(), dirs::home_dir());
    let found = scan::walk(root, &roots, ScanBudget::default());
    let shown = |rel: &str| match path {
        Some(_) => format!("{name}/{rel}"),
        None => rel.to_string(),
    };
    let mut out = Vec::new();
    if found.folder_review {
        out.push(Entry {
            log: root.join(FOLDER_REVIEW),
            doc: None,
            name: name.to_string(),
            path: path.unwrap_or(".").to_string(),
        });
    }
    for rel in &found.review_files {
        let Some(doc_rel) = rel.strip_suffix(REVIEW_SUFFIX) else {
            continue;
        };
        let doc = root.join(doc_rel);
        if !doc.is_file() || !paths::is_md(&doc) {
            continue;
        }
        out.push(Entry {
            log: root.join(rel),
            doc: Some(doc),
            name: shown(doc_rel),
            path: render::shell_quote(&shown(doc_rel)),
        });
    }
    out
}

/// What one look at a folder finds: the first claimable round in path order, the folder's
/// own first, with every other round of its batch. Also returns when a round in its hold
/// comes due. A batch is claimed whole, under every one of its locks at once.
fn look_dir(
    list: &[Entry],
    opts: &Opts,
    now: u64,
) -> anyhow::Result<(Option<Vec<Block>>, Option<u64>)> {
    let mut due: Option<u64> = None;
    let mut found = Vec::new();
    for entry in list {
        let Ok(Some(parsed)) = Log::open(&entry.log).read() else {
            continue;
        };
        let index = fold::scan(&parsed, now);
        if let Some(at) = index.pending_deadline().filter(|&at| at > now) {
            due = Some(due.map_or(at, |d| d.min(at)));
        }
        let Some(round) = index.claimable_round(now) else {
            continue;
        };
        let batch = parsed
            .lines
            .iter()
            .find(|l| l.n == round)
            .and_then(|l| match l.op {
                Op::Submit { batch, .. } => batch,
                _ => None,
            });
        found.push((entry, round, batch, kind_of(&index, round)));
    }
    let Some(&(_, _, batch, _)) = found.first() else {
        return Ok((None, due));
    };
    let group: Vec<_> = match batch {
        Some(_) => found.iter().filter(|f| f.2 == batch).collect(),
        None => vec![&found[0]],
    };

    let picks: Vec<(&Entry, Pick)> = if opts.claims {
        // Path order everywhere, so two waiters locking one batch can't deadlock.
        let mut order: Vec<&Entry> = group.iter().map(|f| f.0).collect();
        order.sort_by(|a, b| a.log.cmp(&b.log));
        let mut held = Vec::new();
        for entry in order {
            match Log::open(&entry.log).hold() {
                Ok(h) => held.push((entry, h)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("locking a review file"),
            }
        }
        let mut claimed = HashMap::new();
        for (entry, h) in &mut held {
            if let Some((claim, round)) = claim_held(h, entry.doc.as_deref(), &opts.by, now)? {
                claimed.insert(entry.log.clone(), (claim, round));
            }
        }
        drop(held);
        group
            .iter()
            .filter_map(|&&(entry, _, _, kind)| {
                let &(claim, round) = claimed.get(&entry.log)?;
                Some(match kind {
                    RoundKind::Approve => (entry, Pick::Approved { round }),
                    RoundKind::Submit => (entry, Pick::Mine { claim, round }),
                })
            })
            .collect()
    } else {
        group
            .iter()
            .map(|&&(entry, round, _, kind)| match kind {
                RoundKind::Approve => (entry, Pick::Approved { round }),
                RoundKind::Submit => (entry, Pick::Peek { round }),
            })
            .collect()
    };
    if picks.is_empty() {
        return Ok((None, due));
    }
    let blocks = picks
        .into_iter()
        .map(|(entry, pick)| block(entry, pick, opts, now))
        .collect::<anyhow::Result<_>>()?;
    Ok((Some(blocks), due))
}

/// With no batch to claim, what each review file under a folder still shows, as
/// `review FILE` would: an approve, or a round under a live claim.
fn settled_dir(list: &[Entry], opts: &Opts, now: u64) -> anyhow::Result<Option<Vec<Block>>> {
    let mut blocks = Vec::new();
    for entry in list {
        let Ok(Some(parsed)) = Log::open(&entry.log).read() else {
            continue;
        };
        if let Some(pick) = settled(&fold::scan(&parsed, now), now, &opts.by) {
            blocks.push(block(entry, pick, opts, now)?);
        }
    }
    Ok((!blocks.is_empty()).then_some(blocks))
}

/// Looks again on every tick where the file changed or a hold ran out.
fn file_poll<'a>(
    entry: &'a Entry,
    opts: &'a Opts,
) -> impl FnMut(u64) -> anyhow::Result<Option<Vec<Block>>> + 'a {
    let mut seen: Option<Stamp> = None;
    let mut due: Option<u64> = None;
    move |now| {
        let now_stamp = stamp(&entry.log);
        if seen == Some(now_stamp) && due.is_none_or(|d| now < d) {
            return Ok(None);
        }
        seen = Some(now_stamp);
        let Some(parsed) = Log::open(&entry.log).read()? else {
            due = None;
            return Ok(None);
        };
        let index = fold::scan(&parsed, now);
        due = index.pending_deadline().filter(|&d| d > now);
        if index.claimable_round(now).is_none() {
            return Ok(None);
        }
        Ok(look_file(entry, opts, now, true)?.map(|b| vec![b]))
    }
}

/// Like [`file_poll`] over every review file under `root`, listed again every 2 s.
fn dir_poll<'a>(
    root: &'a Path,
    name: &'a str,
    path: Option<&'a str>,
    opts: &'a Opts,
) -> impl FnMut(u64) -> anyhow::Result<Option<Vec<Block>>> + 'a {
    let mut list = entries(root, name, path);
    let mut listed = Instant::now();
    let mut seen: Option<Vec<(PathBuf, Stamp)>> = None;
    let mut due: Option<u64> = None;
    move |now| {
        if listed.elapsed() >= RELIST {
            list = entries(root, name, path);
            listed = Instant::now();
        }
        let stamps: Vec<(PathBuf, Stamp)> = list
            .iter()
            .map(|e| (e.log.clone(), stamp(&e.log)))
            .collect();
        if seen.as_ref() == Some(&stamps) && due.is_none_or(|d| now < d) {
            return Ok(None);
        }
        seen = Some(stamps);
        let (blocks, next) = look_dir(&list, opts, now)?;
        due = next;
        Ok(blocks)
    }
}

fn stamp(path: &Path) -> Stamp {
    fs::metadata(path)
        .ok()
        .map(|m| (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
}

// --- the view ---

/// The view of `pick` on `entry`, from the review file as it is now.
fn block(entry: &Entry, pick: Pick, opts: &Opts, now: u64) -> anyhow::Result<Block> {
    let parsed = Log::open(&entry.log)
        .read()?
        .context("the review file went away")?;
    let index = fold::scan(&parsed, now);
    let lines: HashMap<u32, &Line> = parsed.lines.iter().map(|l| (l.n, l)).collect();
    let (state, doc) = match &entry.doc {
        Some(path) => {
            let text =
                fs::read_to_string(path).with_context(|| format!("reading {}", entry.name))?;
            let chain = Chain::build(&parsed);
            let doc = Doc::new(&text);
            let state = fold::fold(&parsed, &chain, &doc).state;
            (state, Some(doc))
        }
        None => (fold::fold_dir(&parsed), None),
    };
    let line = |n: u32| lines.get(&n).copied();
    let drafted_by = state.owner.drafted_by.clone();

    let (round, claim, stance) = match pick {
        Pick::Approved { round } => return Ok(approved(entry, &state, line(round), drafted_by)),
        Pick::Mine { claim, round } => (round, Some(claim), Stance::Mine),
        Pick::Peek { round } => (round, None, Stance::Peek { tty: !opts.peek }),
        Pick::ReadOnly { claim, round } => {
            let by = index
                .rounds
                .iter()
                .find(|r| r.n == round)
                .and_then(|r| r.claimed_by.clone())
                .unwrap_or_else(|| "someone".to_string());
            let at = line(claim).and_then(|l| l.ts).unwrap_or(0);
            (round, Some(claim), Stance::ReadOnly { by, at })
        }
    };
    let read_only = matches!(stance, Stance::ReadOnly { .. });
    let upto = index
        .rounds
        .iter()
        .find(|r| r.n == round)
        .map_or(0, |r| r.upto);
    let pending = pending_through(&index, round);
    let shown: Vec<Item> = state
        .items
        .iter()
        .filter(|i| i.by.kind == ByKind::Human && i.status != ItemStatus::Retracted)
        .filter(|i| {
            let held = match claim {
                Some(c) => i.turn == Some(c),
                None => i.sent_in.is_some_and(|s| pending.contains(&s)),
            };
            held || (!read_only && i.status == ItemStatus::NotHandled)
        })
        .map(|i| as_sent(i, &lines, upto))
        .collect();
    let you = match (&doc, read_only) {
        (Some(doc), false) => you_lines(&state, &index, &lines, doc),
        _ => Vec::new(),
    };
    let handed = line(round);
    let view = RoundView {
        name: entry.name.clone(),
        path: entry.path.clone(),
        handed_at: handed.and_then(|l| l.ts).unwrap_or(0),
        handed_by: handed.and_then(|l| l.by.name.clone()),
        drafted_by: drafted_by.clone(),
        stance,
        items: shown.iter().map(item_line).collect(),
        you: you.clone(),
        all: opts.all,
    };
    let json = json!({
        "status": "round",
        "doc": entry.name,
        "round": round,
        "claim": claim,
        "read_only": read_only,
        "handed_back": {"ts": view.handed_at, "by": view.handed_by},
        "drafted_by": drafted_by,
        "items": shown,
        "you": you,
    });
    Ok(Block {
        text: render::round(&view),
        json,
        changed: false,
    })
}

fn approved(
    entry: &Entry,
    state: &ReviewState,
    line: Option<&Line>,
    drafted_by: Option<String>,
) -> Block {
    let Some(approve) = &state.approve else {
        return Block {
            text: render::nothing(&entry.name, Some(&entry.path)),
            json: json!({"status": "nothing", "doc": entry.name}),
            changed: false,
        };
    };
    let changed = !approve.current_matches;
    let view = ApprovedView {
        name: entry.name.clone(),
        path: entry.path.clone(),
        at: line.and_then(|l| l.ts).unwrap_or(approve.ts),
        by: line.and_then(|l| l.by.name.clone()),
        drafted_by,
        rev: approve.rev.clone(),
        changed_to: changed.then(|| state.rev.clone()),
    };
    Block {
        text: render::approved(&view),
        json: json!({"status": "approved", "doc": entry.name, "approve": approve, "rev": state.rev}),
        changed,
    }
}

/// Submits a claim of `round` would take: unclaimed, not withdrawn, up to it.
fn pending_through(index: &TurnIndex, round: u32) -> HashSet<u32> {
    let taken = index
        .rounds
        .iter()
        .filter(|r| r.claim.is_some())
        .map(|r| r.n)
        .max()
        .unwrap_or(0);
    index
        .rounds
        .iter()
        .filter(|r| r.kind == RoundKind::Submit && r.withdrawn.is_none())
        .filter(|r| r.n > taken && r.n <= round)
        .map(|r| r.n)
        .collect()
}

/// `item` with the verdict and body it was handed back with: amends kept after the
/// hand-back are drafts, and drafts never show.
fn as_sent(item: &Item, lines: &HashMap<u32, &Line>, upto: u32) -> Item {
    let mut sent = item.clone();
    if let Some(Op::Comment { verdict, body, .. }) = lines.get(&item.n).map(|l| &l.op) {
        sent.verdict = *verdict;
        sent.body.clone_from(body);
    }
    for &n in item.amended.iter().filter(|&&n| n <= upto) {
        if let Some(Op::Amend { body, verdict, .. }) = lines.get(&n).map(|l| &l.op) {
            if body.is_some() {
                sent.body.clone_from(body);
            }
            if verdict.is_some() {
                sent.verdict = *verdict;
            }
        }
    }
    sent
}

fn item_line(item: &Item) -> ItemLine {
    ItemLine {
        n: item.n,
        verdict: item.verdict,
        body: item.body.clone(),
        target: target_of(&item.at),
    }
}

fn target_of(at: &AtState) -> Target {
    let quote = || at.quote.clone().unwrap_or_default();
    match (at.kind, at.scope) {
        (AtKind::Doc, _) => Target::Doc,
        (AtKind::Dir, _) => Target::Dir,
        (AtKind::Anchor, Some(Scope::Section)) => Target::Section(quote()),
        (AtKind::Anchor, Some(Scope::Block)) => Target::Block(quote()),
        (AtKind::Anchor, _) => Target::Quote(quote()),
    }
}

/// The human's own writes to the doc since the last agent turn ended.
fn you_lines(
    state: &ReviewState,
    index: &TurnIndex,
    lines: &HashMap<u32, &Line>,
    doc: &Doc,
) -> Vec<String> {
    let last = index
        .rounds
        .iter()
        .filter_map(|r| Some((r.yielded?, r.claimed_by.clone())))
        .max_by_key(|(n, _)| *n);
    let since = last.as_ref().map_or(0, |(n, _)| *n);
    let label = match &last {
        Some((_, Some(agent))) => format!("since {}'s last turn", capitalised(agent)),
        Some((_, None)) => "since the agent's last turn".to_string(),
        None => "before handing back".to_string(),
    };
    let mine: Vec<&Change> = state
        .changes
        .iter()
        .filter(|c| c.attr == Attr::Human && c.op > since)
        .collect();
    let mut out: Vec<String> = mine
        .iter()
        .take(YOU_LINES)
        .map(|c| format!("{label}: {} (change {})", what_changed(c, lines, doc), c.id))
        .collect();
    if mine.len() > YOU_LINES {
        out.push(format!("+{} more changes", mine.len() - YOU_LINES));
    }
    out
}

fn what_changed(change: &Change, lines: &HashMap<u32, &Line>, doc: &Doc) -> String {
    let (first, last) = change.range.lines;
    match lines.get(&change.op).map(|l| &l.op) {
        Some(Op::Edit {
            act: Act::Undo, of, ..
        }) => format!("undid change {}", of.as_deref().unwrap_or("?")),
        Some(Op::Edit {
            act: Act::Checkbox, ..
        }) => format!("ticked a task on line {first}"),
        _ if change.what.new_lines == 0 => {
            let removed = change.removed.as_deref().unwrap_or_default();
            format!("removed \"{}\"", render::clip(removed))
        }
        _ => {
            let span = if first == last {
                format!("line {first}")
            } else {
                format!("lines {first}–{last}")
            };
            let (a, b) = change.range.bytes;
            let text = doc.text.get(a..b).and_then(|t| t.lines().next());
            match text {
                Some(text) => format!("edited {span} \"{}\"", render::clip(text)),
                None => format!("edited {span}"),
            }
        }
    }
}

fn capitalised(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}
