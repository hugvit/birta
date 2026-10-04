//! Records a doc write nobody logged, under the review file's lock. The server's watcher
//! and every CLI verb call it before their own ops, so a write lands once, in order.

use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::chain::{Chain, Recorded};
use super::fold::{self, RoundKind};
use super::log::{AppendCtx, AppendError, Held};
use super::ops::{By, ByKind, Op, Parsed};

/// How long `git rev-parse HEAD` may take before the write is recorded without it.
const GIT_TIMEOUT: Duration = Duration::from_millis(300);
const GIT_POLL: Duration = Duration::from_millis(10);

/// Reads the doc and records it if its rev isn't the log's head. Returns the line of
/// the `external`; `None` when there is nothing to record or the doc can't be read.
pub fn sync_external(held: &mut Held, doc: &Path, now_ms: u64) -> Option<u32> {
    let bytes = read_doc(doc).ok()?;
    sync_bytes(held, doc, &bytes, now_ms)
}

/// [`sync_external`] for doc bytes already read under the lock, so a caller that checked
/// them records exactly what it checked.
pub fn sync_bytes(held: &mut Held, doc: &Path, bytes: &[u8], now_ms: u64) -> Option<u32> {
    sync_bytes_with(held, doc, bytes, now_ms, GIT_TIMEOUT)
}

/// [`sync_bytes`], giving git `git_timeout` to name HEAD.
pub fn sync_bytes_with(
    held: &mut Held,
    doc: &Path,
    bytes: &[u8],
    now_ms: u64,
    git_timeout: Duration,
) -> Option<u32> {
    record_with(held, doc, bytes, now_ms, git_timeout)
        .ok()
        .flatten()
}

/// [`sync_bytes`] for a caller that must not write on past a write it couldn't record:
/// `Err` when the `external` failed to land.
pub fn try_sync_bytes(
    held: &mut Held,
    doc: &Path,
    bytes: &[u8],
    now_ms: u64,
) -> io::Result<Option<u32>> {
    record_with(held, doc, bytes, now_ms, GIT_TIMEOUT).map_err(|e| match e {
        AppendError::Io(e) => e,
        AppendError::Rejected(why) => io::Error::other(format!("{why:?}")),
    })
}

fn record_with(
    held: &mut Held,
    doc: &Path,
    bytes: &[u8],
    now_ms: u64,
    git_timeout: Duration,
) -> Result<Option<u32>, AppendError> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(None);
    };
    let Some(recorded) = Chain::build(held.parsed()).record(text) else {
        return Ok(None);
    };
    let head = doc.parent().and_then(|dir| git_head(dir, git_timeout));
    let claimed = claim_open(held.parsed());
    let last = last_head(held.parsed());
    // A checkout or pull moves HEAD; inside a claim the write is the claimer's anyway
    let git = !claimed && head.is_some() && last.is_some() && head != last;
    let ops = write_ops(recorded, git.then(|| "git".to_string()), head);
    let ctx = AppendCtx {
        now_ms,
        by: By {
            kind: ByKind::Birta,
            ..By::default()
        },
        cid: None,
    };
    Ok(held.append_with(ctx, |_, _| Ok(ops))?.first().copied())
}

/// The ops that record the doc's `text` as a write inside a claim, as
/// [`try_sync_bytes`] would append them; empty when the log already has it.
pub fn pending_write(parsed: &Parsed, text: &str) -> Vec<Op> {
    Chain::build(parsed)
        .record(text)
        .map(|recorded| write_ops(recorded, None, None))
        .unwrap_or_default()
}

/// An `external` line for a recorded write, and the checkpoint snap that may follow it.
fn write_ops(recorded: Recorded, cause: Option<String>, head: Option<String>) -> Vec<Op> {
    let mut ops = vec![Op::External {
        rev: recorded.rev,
        to_rev: recorded.to_rev.clone(),
        patch: recorded.patch,
        cause,
        head,
    }];
    if let Some(text) = recorded.checkpoint {
        ops.push(Op::Snap {
            rev: recorded.to_rev,
            text,
            checkpoint: true,
        });
    }
    ops
}

/// A doc's bytes, only from a regular file. Anything else is refused before it is opened,
/// and the open doesn't block, so a FIFO swapped in can't stall a caller holding a lock.
pub fn read_doc(path: &Path) -> io::Result<Vec<u8>> {
    let not_regular = || io::Error::new(io::ErrorKind::InvalidInput, "not a regular file");
    if !fs::metadata(path)?.is_file() {
        return Err(not_regular());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(not_regular());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Whether an agent holds a hand-back it hasn't yielded, so a write now is its.
fn claim_open(parsed: &Parsed) -> bool {
    fold::scan(parsed, 0)
        .rounds
        .iter()
        .any(|r| r.kind == RoundKind::Submit && r.claim.is_some() && r.yielded.is_none())
}

/// The `head` of the latest `external` that recorded one.
fn last_head(parsed: &Parsed) -> Option<String> {
    parsed.lines.iter().rev().find_map(|l| match &l.op {
        Op::External {
            head: Some(head), ..
        } => Some(head.clone()),
        _ => None,
    })
}

/// The commit HEAD names in `dir`'s repo; `None` outside a repo, before the first commit,
/// or when git doesn't answer in time.
fn git_head(dir: &Path, timeout: Duration) -> Option<String> {
    let mut child = Command::new("git")
        // An inherited GIT_DIR (e.g. from a hook) would name another repo than dir's
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(GIT_POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let head = out.trim();
    let hex = !head.is_empty() && head.bytes().all(|b| b.is_ascii_hexdigit());
    (status.success() && hex).then(|| head.to_string())
}
