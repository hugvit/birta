//! `birta review`: the round handed back on a doc or a folder, and the claim that picks
//! it up. A claim runs under the review file's lock, with the doc read and checked before
//! anything is written, and the view prints after the lock is let go.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::{Value, json};

use super::identity::{self, Identity};
use super::render::{self, AnswerLine, ApprovedView, ItemLine, RoundView, Stance, Target};
use super::{CONTROL_PATH, ExitCode, ReviewArgs, hidden_in, relative, wait, warn};
use crate::config::Config;
use crate::paths::{self, Roots};
use crate::review::chain::Chain;
use crate::review::doc::Doc;
use crate::review::fold::{self, RoundKind, TurnIndex, TurnNote};
use crate::review::ladder;
use crate::review::log::{AppendCtx, AppendError, Held, Log};
use crate::review::ops::{Act, By, ByKind, Line, Op, Parsed, Scope, Verdict, Via};
use crate::review::state::{
    AtKind, AtState, Attr, Change, Item, ItemStatus, ReplyState, ReviewState,
};
use crate::review::sync::{read_doc, try_sync_bytes};
use crate::review::{FOLDER_REVIEW, Reject, review_path_for};
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
    let entry = Entry {
        log: review_path_for(doc),
        doc: Some(doc.to_path_buf()),
        name: doc.display().to_string(),
        path: String::new(),
        target: own_name(doc),
    };
    Ok(claim_file(&entry, by, now_ms)?.map(|(line, ..)| line))
}

pub fn run(args: &ReviewArgs, config: &Config) -> ExitCode {
    match review(args, config) {
        Ok(code) => code,
        Err(e) => {
            warn(&format!("error: {e:#}"));
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
    /// What a claim of it names: the folder it's listed under, or the doc itself,
    /// relative to the doc's real folder.
    target: Option<String>,
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
    /// Whether the target is a folder, which is waited on once for all its docs.
    dir: bool,
}

/// What a view shows: a round, as whose, or an approve.
#[derive(Clone, Copy, Debug)]
enum Pick {
    Mine { claim: u32, round: u32 },
    Peek { round: u32 },
    ReadOnly { claim: u32, round: u32 },
    Approved { round: u32 },
}

/// What a view is built from: the review file, and the doc's text read once.
struct Inputs {
    parsed: Parsed,
    /// `None` for a folder's own review file.
    text: Option<String>,
}

/// One doc's part of the output.
struct Block {
    text: String,
    json: Value,
    changed: bool,
    /// Set on a round's view, for a folder's batch line.
    handed: Option<Handed>,
}

/// A round as a folder's batch line counts it.
#[derive(Clone, Copy)]
struct Handed {
    at: u64,
    notes: usize,
    answers: usize,
    /// False for the folder's own review file.
    doc: bool,
    /// Claimed by whoever runs the view, so it has a turn to wait out.
    mine: bool,
}

/// What a review file looks like on disk, to notice a change or a replacement.
type Stamp = Option<(u64, u64, u64, i64, i64)>;

fn review(args: &ReviewArgs, config: &Config) -> anyhow::Result<ExitCode> {
    if let Some(drafter) = &args.drafted_by
        && (drafter.trim().is_empty() || drafter.chars().any(render::is_hidden))
    {
        warn("error: --drafted-by takes a name with no control characters");
        return Ok(ExitCode::Usage);
    }
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
        dir: false,
    };
    let place = resolve(args.target.as_deref())?;
    let opts = Opts {
        dir: matches!(place, Place::Dir { .. }),
        ..opts
    };
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
                let list = entries(root, name, path.as_deref(), &mut HashSet::new());
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
        let views = blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let handed: Vec<Handed> = blocks.iter().filter_map(|b| b.handed).collect();
        match (dir, handed.is_empty()) {
            (true, false) => folder(&handed, opts, name, path, &views),
            _ => views,
        }
    } else if let ([one], false) = (&blocks[..], dir) {
        json_line(&one.json)
    } else {
        let docs: Vec<&Value> = blocks.iter().map(|b| &b.json).collect();
        json_line(&json!({"status": "round", "docs": docs}))
    };
    (code, text)
}

/// A folder's views under one batch line, then, unless only looking, the one wait for all
/// of them, as the last step of every doc's `next:`.
fn folder(handed: &[Handed], opts: &Opts, name: &str, path: Option<&str>, views: &str) -> String {
    let at = handed.iter().map(|h| h.at).max().unwrap_or(0);
    let files = handed.iter().filter(|h| h.doc).count();
    let notes = handed.iter().map(|h| h.notes).sum();
    let answers = handed.iter().map(|h| h.answers).sum();
    let mut out = format!(
        "{}/ · handed back {} · {}{}\n{views}",
        render::one_line(name.trim_end_matches('/')),
        render::hhmm(at),
        render::plural(files, "file"),
        render::counts(notes, answers)
    );
    if !opts.peek && handed.iter().any(|h| h.mine) {
        out.push_str(&format!("\n{}\n", render::folder_wait(path)));
    }
    out
}

fn json_line(value: &Value) -> String {
    format!("{value}\n")
}

fn resolve(target: Option<&Path>) -> anyhow::Result<Place> {
    let Some(target) = target else {
        let root = std::env::current_dir()?.canonicalize()?;
        if hidden_in(&root) {
            anyhow::bail!(CONTROL_PATH);
        }
        return Ok(Place::Dir {
            root,
            name: ".".to_string(),
            path: None,
        });
    };
    if hidden_in(target) {
        anyhow::bail!(CONTROL_PATH);
    }
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
        target: own_name(&canonical),
        doc: Some(canonical),
        name: typed,
        path,
    }))
}

/// A doc's name in its real folder, which a file claim records as what it claimed.
pub(super) fn own_name(doc: &Path) -> Option<String> {
    let real = doc.canonicalize().ok()?;
    Some(real.file_name()?.to_string_lossy().into_owned())
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

/// A review file under its lock, with its doc read under that lock.
struct Locked<'a> {
    entry: &'a Entry,
    held: Held,
    text: Option<String>,
}

impl Locked<'_> {
    /// Whether `round` is still the one to claim, checked before anything is written.
    fn claimable(&self, round: u32, now: u64) -> bool {
        let parsed = self.held.parsed();
        !parsed.read_only && fold::scan(parsed, now).claimable_round(now) == Some(round)
    }

    /// Records any doc write nobody logged, then claims `round`. Returns the claim's line.
    fn claim(&mut self, round: u32, by: &By, now_ms: u64) -> anyhow::Result<u32> {
        if let (Some(doc), Some(text)) = (&self.entry.doc, &self.text) {
            try_sync_bytes(&mut self.held, doc, text.as_bytes(), now_ms)
                .with_context(|| format!("recording the last write to {}", self.entry.name))?;
        }
        let ctx = AppendCtx {
            now_ms,
            by: by.clone(),
            cid: None,
        };
        let target = self.entry.target.clone();
        let landed = self
            .held
            .append_with(ctx, |parsed, now| {
                match fold::scan(parsed, now).claimable_round(now) {
                    Some(r) if r == round => Ok(vec![Op::Claim { round, target }]),
                    _ => Err(Reject::AlreadyClaimed),
                }
            })
            .map_err(|e| match e {
                AppendError::Io(e) => anyhow::Error::new(e).context("writing the claim"),
                AppendError::Rejected(why) => {
                    anyhow::anyhow!("the review file refused the claim: {why:?}")
                }
            })?;
        landed.first().copied().context("the claim was not written")
    }

    /// What the view needs, as the lock leaves it; the lock goes with it.
    fn into_inputs(self) -> Inputs {
        Inputs {
            parsed: self.held.parsed().clone(),
            text: self.text,
        }
    }
}

/// Locks `entry`'s review file and reads its doc. `None` when there is no finished file.
fn lock(entry: &Entry) -> anyhow::Result<Option<Locked<'_>>> {
    let held = match Log::open(&entry.log).hold() {
        Ok(held) => held,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("locking the review file of {}", entry.name));
        }
    };
    let text = entry
        .doc
        .as_deref()
        .map(|doc| read_text(doc, &entry.name))
        .transpose()?;
    Ok(Some(Locked { entry, held, text }))
}

/// A doc's text, which a view can be built from only when it is a regular UTF-8 file.
fn read_text(doc: &Path, name: &str) -> anyhow::Result<String> {
    let bytes = read_doc(doc).with_context(|| format!("reading {name}"))?;
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{name} is not UTF-8"))
}

/// A claim's line, its round, and the view's inputs as of the claim.
type Claimed = (u32, u32, Inputs);

/// Claims the latest due round on `entry`, if nobody has.
fn claim_file(entry: &Entry, by: &By, now_ms: u64) -> anyhow::Result<Option<Claimed>> {
    let Some(mut locked) = lock(entry)? else {
        return Ok(None);
    };
    let Some(round) = fold::scan(locked.held.parsed(), now_ms).claimable_round(now_ms) else {
        return Ok(None);
    };
    let claim = locked.claim(round, by, now_ms)?;
    Ok(Some((claim, round, locked.into_inputs())))
}

/// Claims every file of a batch, or none of them. Each review file is locked once, even
/// under two names, in `(dev, ino)` order, so waiters from any root lock alike. `None`
/// when a file is no longer claimable as found.
fn claim_batch(
    group: &[(&Entry, u32)],
    by: &By,
    now_ms: u64,
) -> anyhow::Result<Option<HashMap<PathBuf, Claimed>>> {
    let mut keyed = Vec::new();
    for &(entry, round) in group {
        let Ok(meta) = fs::metadata(&entry.log) else {
            return Ok(None);
        };
        keyed.push(((meta.dev(), meta.ino()), entry, round));
    }
    keyed.sort_by_key(|k| k.0);
    keyed.dedup_by_key(|k| k.0);
    let mut locked = Vec::new();
    for (_, entry, round) in keyed {
        match lock(entry)? {
            Some(l) => locked.push((l, round)),
            None => return Ok(None),
        }
    }
    if !locked.iter().all(|(l, round)| l.claimable(*round, now_ms)) {
        return Ok(None);
    }
    // Only a failed write can stop it now; what landed stays, and the error names it
    let mut claims = Vec::new();
    for i in 0..locked.len() {
        let (l, round) = &mut locked[i];
        match l.claim(*round, by, now_ms) {
            Ok(claim) => claims.push(claim),
            Err(e) => {
                let names = |ls: &[(Locked, u32)]| {
                    ls.iter()
                        .map(|(l, _)| l.entry.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let done = match names(&locked[..i]) {
                    none if none.is_empty() => "none".to_string(),
                    done => done,
                };
                return Err(e.context(format!(
                    "claiming {}; claimed before it failed: {done}",
                    locked[i].0.entry.name
                )));
            }
        }
    }
    Ok(Some(
        locked
            .into_iter()
            .zip(claims)
            .map(|((l, round), claim)| (l.entry.log.clone(), (claim, round, l.into_inputs())))
            .collect(),
    ))
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
        Some(_) => match claim_file(entry, &opts.by, now)? {
            Some((claim, round, inputs)) => {
                let pick = picked(&fold::scan(&inputs.parsed, now), claim, round);
                return Ok(Some(block(entry, pick, opts, now, &inputs)));
            }
            None if waiting => None,
            None => match log.read()? {
                Some(parsed) => settled(&fold::scan(&parsed, now), now, &opts.by),
                None => None,
            },
        },
        None if waiting => None,
        None => settled(&index, now, &opts.by),
    };
    pick.map(|pick| Ok(block(entry, pick, opts, now, &inputs(entry)?)))
        .transpose()
}

/// The view's inputs for a look that claims nothing.
fn inputs(entry: &Entry) -> anyhow::Result<Inputs> {
    let parsed = Log::open(&entry.log)
        .read()?
        .context("the review file went away")?;
    let text = entry
        .doc
        .as_deref()
        .map(|doc| read_text(doc, &entry.name))
        .transpose()?;
    Ok(Inputs { parsed, text })
}

fn picked(index: &TurnIndex, claim: u32, round: u32) -> Pick {
    match kind_of(index, round) {
        RoundKind::Approve => Pick::Approved { round },
        RoundKind::Submit => Pick::Mine { claim, round },
    }
}

/// The review files under `root`, the folder's own first. One whose path a view would
/// print with a control character is left out, and said so on stderr once per path:
/// `warned` holds the paths already named.
fn entries(
    root: &Path,
    name: &str,
    path: Option<&str>,
    warned: &mut HashSet<String>,
) -> Vec<Entry> {
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
            target: Some(".".to_string()),
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
        if hidden_in(Path::new(doc_rel)) {
            if warned.insert(doc_rel.to_string()) {
                warn(&format!(
                    "warning: skipped {}: {CONTROL_PATH}",
                    render::escaped(&shown(doc_rel))
                ));
            }
            continue;
        }
        // From the doc's real folder, so a respond through any link finds this one
        let target = doc
            .canonicalize()
            .ok()
            .and_then(|real| Some(relative(real.parent()?, root)))
            .map(|rel| rel.to_string_lossy().into_owned());
        out.push(Entry {
            log: root.join(rel),
            doc: Some(doc),
            name: shown(doc_rel),
            path: render::shell_quote(&shown(doc_rel)),
            target,
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

    let blocks: Vec<Block> = if opts.claims {
        let rounds: Vec<(&Entry, u32)> = group.iter().map(|f| (f.0, f.1)).collect();
        let Some(mut claimed) = claim_batch(&rounds, &opts.by, now)? else {
            return Ok((None, due));
        };
        // An entry sharing its review file with an earlier one was claimed as that one
        group
            .iter()
            .filter_map(|&&(entry, _, _, kind)| {
                let (claim, round, inputs) = claimed.remove(&entry.log)?;
                let pick = match kind {
                    RoundKind::Approve => Pick::Approved { round },
                    RoundKind::Submit => Pick::Mine { claim, round },
                };
                Some(block(entry, pick, opts, now, &inputs))
            })
            .collect()
    } else {
        group
            .iter()
            .map(|&&(entry, round, _, kind)| {
                let pick = match kind {
                    RoundKind::Approve => Pick::Approved { round },
                    RoundKind::Submit => Pick::Peek { round },
                };
                Ok(block(entry, pick, opts, now, &inputs(entry)?))
            })
            .collect::<anyhow::Result<_>>()?
    };
    if blocks.is_empty() {
        return Ok((None, due));
    }
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
            blocks.push(block(entry, pick, opts, now, &inputs(entry)?));
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
    let mut warned = HashSet::new();
    let mut list = entries(root, name, path, &mut warned);
    let mut listed = Instant::now();
    let mut seen: Option<Vec<(PathBuf, Stamp)>> = None;
    let mut due: Option<u64> = None;
    move |now| {
        if listed.elapsed() >= RELIST {
            list = entries(root, name, path, &mut warned);
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

/// The view of `pick` on `entry`, from `inputs`. Nothing here can fail, so a claim is
/// never left with nothing printed.
fn block(entry: &Entry, pick: Pick, opts: &Opts, now: u64, inputs: &Inputs) -> Block {
    let parsed = &inputs.parsed;
    let index = fold::scan(parsed, now);
    let lines: HashMap<u32, &Line> = parsed.lines.iter().map(|l| (l.n, l)).collect();
    let (state, turn_notes, doc) = match &inputs.text {
        Some(text) => {
            let doc = Doc::new(text);
            let chain = Chain::build(parsed);
            let (state, turn_notes) = {
                let folded = fold::fold(parsed, &chain, &doc);
                (folded.state, folded.turn_notes)
            };
            (state, turn_notes, Some(doc))
        }
        None => (fold::fold_dir(parsed), Vec::new(), None),
    };
    let line = |n: u32| lines.get(&n).copied();
    let drafted_by = state.owner.drafted_by.clone();

    let (round, claim, stance) = match pick {
        Pick::Approved { round } => {
            return approved(
                entry,
                &state,
                &turn_notes,
                &index,
                line(round),
                drafted_by,
                opts,
            );
        }
        Pick::Mine { claim, round } => (round, Some(claim), Stance::Mine),
        Pick::Peek { round } => (round, None, Stance::Peek { tty: !opts.peek }),
        Pick::ReadOnly { claim, round } => {
            let by = render::who(&line(claim).map(|l| l.by.clone()).unwrap_or_default());
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
    let pending: Vec<u32> = pending.into_iter().collect();
    let shown: Vec<Item> = state
        .items
        .iter()
        .filter(|i| fold::listed(i, claim, &pending, read_only))
        .map(|i| as_sent(i, &lines, upto))
        .collect();
    let answers = answers(&state, &turn_notes, since(&index, round), upto);
    let you = match (&doc, read_only) {
        (Some(doc), false) => you_lines(&state, &index, &lines, doc),
        _ => Vec::new(),
    };
    let handed = line(round);
    let view = RoundView {
        name: entry.name.clone(),
        path: entry.path.clone(),
        handed_at: handed.and_then(|l| l.ts).unwrap_or(0),
        handed_by: handed.map(|l| l.by.clone()).unwrap_or_default(),
        drafted_by: drafted_by.clone(),
        stance,
        items: shown.iter().map(item_line).collect(),
        answers: answers.iter().map(Answer::line).collect(),
        you: you.clone(),
        all: opts.all,
        next: !opts.peek,
        wait: !opts.dir,
    };
    let json = json!({
        "status": "round",
        "doc": entry.name,
        "round": round,
        "claim": claim,
        "read_only": read_only,
        "handed_back": {"ts": view.handed_at, "by": view.handed_by.name},
        "drafted_by": drafted_by,
        "items": shown,
        "answers": answers.iter().map(Answer::json).collect::<Vec<_>>(),
        "you": you,
    });
    Block {
        text: render::round(&view),
        json,
        changed: false,
        handed: Some(Handed {
            at: view.handed_at,
            notes: view.items.len(),
            answers: view.answers.len(),
            doc: entry.doc.is_some(),
            mine: matches!(view.stance, Stance::Mine),
        }),
    }
}

fn approved(
    entry: &Entry,
    state: &ReviewState,
    turn_notes: &[TurnNote],
    index: &TurnIndex,
    line: Option<&Line>,
    drafted_by: Option<String>,
    opts: &Opts,
) -> Block {
    let Some(approve) = &state.approve else {
        return Block {
            text: render::nothing(&entry.name, Some(&entry.path)),
            json: json!({"status": "nothing", "doc": entry.name}),
            changed: false,
            handed: None,
        };
    };
    let changed = !approve.current_matches;
    // Replies written before the approve are all the agent will get
    let before = approve.n.saturating_sub(1);
    let answers = answers(state, turn_notes, since(index, approve.n), before);
    let unanswered: Vec<&Item> = state
        .items
        .iter()
        .filter(|i| agents(i) && i.verdict == Some(Verdict::Ask))
        .filter(|i| {
            !i.replies
                .iter()
                .any(|r| r.by.kind == ByKind::Human && r.n < approve.n)
        })
        .collect();
    let view = ApprovedView {
        name: entry.name.clone(),
        path: entry.path.clone(),
        at: line.and_then(|l| l.ts).unwrap_or(approve.ts),
        by: line.map(|l| l.by.clone()).unwrap_or_default(),
        drafted_by,
        rev: approve.rev.clone(),
        changed_to: changed.then(|| state.rev.clone()),
        answers: answers.iter().map(Answer::line).collect(),
        unanswered: unanswered.iter().map(|i| target_of(&i.at)).collect(),
        next: !opts.peek,
    };
    let unanswered: Vec<Value> = unanswered
        .iter()
        .map(|i| asked_json(i.n, Some(&i.at)))
        .collect();
    Block {
        text: render::approved(&view),
        json: json!({"status": "approved", "doc": entry.name, "approve": approve, "rev": state.rev,
                     "answers": answers.iter().map(Answer::json).collect::<Vec<_>>(),
                     "unanswered": unanswered}),
        changed,
        handed: None,
    }
}

/// The last human line an earlier claim already took, so replies up to it reached the
/// agent then.
fn since(index: &TurnIndex, round: u32) -> u32 {
    index
        .rounds
        .iter()
        .filter(|r| r.kind == RoundKind::Submit && r.n < round)
        .filter(|r| r.claim.is_some() && r.withdrawn.is_none())
        .map(|r| r.upto)
        .max()
        .unwrap_or(0)
}

/// A note of the agent's that the reviewer can answer.
fn agents(item: &Item) -> bool {
    item.by.kind != ByKind::Human
        && item.op == "comment"
        && !matches!(item.status, ItemStatus::Retracted | ItemStatus::Withdrawn)
}

/// The reviewer's reply to an agent's note, ask or turn note.
struct Answer<'a> {
    to: u32,
    ask: bool,
    /// `None` for a turn note, which is on the whole doc.
    at: Option<&'a AtState>,
    reply: &'a ReplyState,
}

impl Answer<'_> {
    fn line(&self) -> AnswerLine {
        AnswerLine {
            n: Some(self.reply.n),
            ask: self.ask,
            target: self.at.map_or(Target::Doc, target_of),
            verdict: self.reply.verdict,
            body: self.reply.body.clone(),
        }
    }

    fn json(&self) -> Value {
        let mut out = asked_json(self.to, self.at);
        out["kind"] = json!(if self.ask { "answer" } else { "re" });
        out["n"] = json!(self.reply.n);
        if let Some(verdict) = self.reply.verdict {
            out["verdict"] = json!(verdict);
        }
        if let Some(body) = &self.reply.body {
            out["body"] = json!(body);
        }
        out
    }
}

/// Each agent note's and turn note's latest reply from the reviewer written after line
/// `since`, up to line `upto`.
fn answers<'a>(
    state: &'a ReviewState,
    turn_notes: &'a [TurnNote],
    since: u32,
    upto: u32,
) -> Vec<Answer<'a>> {
    let latest = |replies: &'a [ReplyState]| {
        replies
            .iter()
            .filter(|r| r.by.kind == ByKind::Human && r.n > since && r.n <= upto)
            .max_by_key(|r| r.n)
    };
    let on_notes = state.items.iter().filter(|i| agents(i)).filter_map(|i| {
        Some(Answer {
            to: i.n,
            ask: i.verdict == Some(Verdict::Ask),
            at: Some(&i.at),
            reply: latest(&i.replies)?,
        })
    });
    let on_turns = turn_notes.iter().filter_map(|t| {
        Some(Answer {
            to: t.n,
            ask: false,
            at: None,
            reply: latest(&t.replies)?,
        })
    });
    on_notes.chain(on_turns).collect()
}

/// The lines of the answers the round view of `round` lists, which `#N covers` may cite.
pub(super) fn answer_ids(
    state: &ReviewState,
    turn_notes: &[TurnNote],
    index: &TurnIndex,
    round: u32,
) -> Vec<u32> {
    let upto = index
        .rounds
        .iter()
        .find(|r| r.n == round)
        .map_or(0, |r| r.upto);
    answers(state, turn_notes, since(index, round), upto)
        .iter()
        .map(|a| a.reply.n)
        .collect()
}

/// `{to, quote?}`: an agent's note, with its quote when it has one.
fn asked_json(to: u32, at: Option<&AtState>) -> Value {
    let mut out = json!({"to": to});
    if let Some(AtState {
        kind: AtKind::Anchor,
        quote: Some(quote),
        ..
    }) = at
    {
        out["quote"] = json!(quote);
    }
    out
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

/// `item` as it was handed back, which both the text and the JSON show: amends and human
/// replies kept after the hand-back are drafts, and drafts never show.
fn as_sent(item: &Item, lines: &HashMap<u32, &Line>, upto: u32) -> Item {
    let mut sent = item.clone();
    sent.amended.retain(|&n| n <= upto);
    sent.replies
        .retain(|r| r.by.kind != ByKind::Human || r.n <= upto);
    if let Some(Op::Comment { verdict, body, .. }) = lines.get(&item.n).map(|l| &l.op) {
        sent.verdict = *verdict;
        sent.body.clone_from(body);
    }
    for &n in &sent.amended {
        if let Some(Op::Amend {
            body,
            verdict,
            drop_verdict,
            ..
        }) = lines.get(&n).map(|l| &l.op)
        {
            if body.is_some() {
                sent.body.clone_from(body);
            }
            if verdict.is_some() || *drop_verdict {
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
        .map(|c| match reverted(state, c, lines, doc) {
            Some(line) => line,
            None => format!("{label}: {}", what_changed(c, lines, doc)),
        })
        .collect();
    if mine.len() > YOU_LINES {
        out.push(format!("+{} more changes", mine.len() - YOU_LINES));
    }
    out
}

/// A revert of the agent's change, named by the text it put back (or, when it only
/// took text out, the text it took), its section and the change's label.
fn reverted(
    state: &ReviewState,
    change: &Change,
    lines: &HashMap<u32, &Line>,
    doc: &Doc,
) -> Option<String> {
    let Some(Op::Edit {
        act: Act::Undo, of, ..
    }) = lines.get(&change.op).map(|l| &l.op)
    else {
        return None;
    };
    let (a, b) = change.range.bytes;
    let text = match doc.text.get(a..b).filter(|t| !t.trim().is_empty()) {
        Some(text) => text,
        None => change.removed.as_deref().unwrap_or_default(),
    };
    let words = text.lines().map(str::trim).find(|l| !l.is_empty());
    let mut line = format!(
        "reverted your change to \"{}\"",
        render::clip(words.unwrap_or_default())
    );
    if let Some(heading) = ladder::section_heading(doc, change.range.lines.0) {
        line.push_str(&format!(" ({})", render::clip(&heading)));
    }
    let label = of
        .as_deref()
        .and_then(|of| state.changes.iter().find(|c| c.id == of))
        .and_then(|c| c.label.as_deref());
    if let Some(label) = label {
        line.push_str(&format!(" — {label}"));
    }
    Some(line)
}

fn what_changed(change: &Change, lines: &HashMap<u32, &Line>, doc: &Doc) -> String {
    let (first, last) = change.range.lines;
    match lines.get(&change.op).map(|l| &l.op) {
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
