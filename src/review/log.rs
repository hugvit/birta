//! Review file I/O. Every write takes an exclusive lock on the file, re-reads it, runs
//! its guard, and lands all its lines in one write, so racing processes serialize.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use super::Reject;
use super::chain::Chain;
use super::ops::{self, Act, By, ByKind, Header, Line, Op, Parsed, ParsedLine};
use super::sync;
use crate::doc_write::{EditRecorder, Recording, Via, WriteErr};
use crate::rev::Rev;

/// Sync tools replace files; give up if the path keeps changing under us.
const REOPEN_TRIES: usize = 8;
/// How much of the file before a [`Tailer`]'s offset must stay the same.
const TAIL_CHECK: usize = 4096;
const EXCLUDE_PATTERN: &str = "*.birta.jsonl";

/// Who is appending, and when.
#[derive(Clone, Debug)]
pub struct AppendCtx {
    pub now_ms: u64,
    pub by: By,
    pub cid: Option<String>,
}

#[derive(Debug)]
pub enum AppendError {
    Rejected(Reject),
    Io(io::Error),
}

impl From<io::Error> for AppendError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// A review file, by path. Opening never creates it.
#[derive(Clone, Debug)]
pub struct Log {
    path: PathBuf,
    git_exclude: bool,
}

impl Log {
    pub fn open(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            git_exclude: true,
        }
    }

    /// Whether creating the file also adds review files to the repo's `info/exclude`.
    pub fn git_exclude(self, on: bool) -> Self {
        Self {
            git_exclude: on,
            ..self
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `None` when there is no file, or only what an unfinished creation left.
    pub fn read(&self) -> io::Result<Option<Parsed>> {
        match fs::read(&self.path) {
            Ok(bytes) if ops::is_unfinished(&bytes) => Ok(None),
            Ok(bytes) => Ok(Some(ops::parse(bytes))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Appends `ops` if `guard` accepts the file as it is under the lock. Returns the
    /// line numbers they landed on.
    pub fn append(
        &self,
        ctx: AppendCtx,
        ops: Vec<Op>,
        guard: impl FnOnce(&Parsed, u64) -> Result<(), Reject>,
    ) -> Result<Vec<u32>, AppendError> {
        self.append_with(ctx, |parsed, now| guard(parsed, now).map(|()| ops))
    }

    /// Like [`Log::append`], for ops that depend on what the file holds under the lock.
    pub fn append_with(
        &self,
        ctx: AppendCtx,
        build: impl FnOnce(&Parsed, u64) -> Result<Vec<Op>, Reject>,
    ) -> Result<Vec<u32>, AppendError> {
        self.hold()?.append_with(ctx, build)
    }

    /// Takes the file's exclusive lock until the [`Held`] is dropped, so a write can
    /// span several files. `NotFound` when there is no finished file.
    pub fn hold(&self) -> io::Result<Held> {
        let (file, bytes) = self.lock()?;
        if ops::is_unfinished(&bytes) {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        let parsed = ops::parse(&bytes);
        Ok(Held {
            file,
            bytes,
            parsed,
            poisoned: false,
        })
    }
    /// Creates the file with its header, baseline snap and owner, all or nothing.
    /// Returns whether this call created it; the first creator's owner stands.
    pub fn create_if_missing(
        &self,
        doc_name: &str,
        baseline: &str,
        drafted_by: Option<&str>,
    ) -> io::Result<bool> {
        match fs::read(&self.path) {
            Ok(bytes) if !ops::is_unfinished(&bytes) => return Ok(false),
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("review file has no parent directory"))?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".birta-review-")
            .tempfile_in(dir)?;
        if let Some(mode) = doc_mode(&self.path) {
            tmp.as_file()
                .set_permissions(fs::Permissions::from_mode(mode))?;
        }
        tmp.write_all(&first_lines(doc_name, baseline, drafted_by)?)?;
        tmp.as_file().sync_data()?;
        // A link never replaces a file, so of racing creators exactly one lands.
        match fs::hard_link(tmp.path(), &self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // Only an unfinished file may be replaced, and only under its lock.
                let (_held, bytes) = self.lock()?;
                if !ops::is_unfinished(&bytes) {
                    return Ok(false);
                }
                tmp.persist(&self.path).map_err(|e| e.error)?;
            }
            Err(e) => return Err(e),
        }
        if self.git_exclude {
            exclude_from_git(&self.path);
        }
        Ok(true)
    }

    /// Opens and exclusively locks the file at the path, reopening if a sync tool
    /// replaced it while we waited. Returns the locked file and its bytes.
    fn lock(&self) -> io::Result<(File, Vec<u8>)> {
        for _ in 0..REOPEN_TRIES {
            let mut file = OpenOptions::new()
                .read(true)
                .append(true)
                .open(&self.path)?;
            file.lock()?;
            let now = fs::metadata(&self.path)?;
            let held = file.metadata()?;
            if (now.dev(), now.ino()) != (held.dev(), held.ino()) {
                continue;
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            return Ok((file, bytes));
        }
        Err(io::Error::other("review file kept being replaced"))
    }
}

impl Log {
    /// The file under its lock, for a doc write to be recorded in. `None` when there is
    /// no finished file; `ReadOnly` when it can't take another line.
    pub(crate) fn hold_for_edit(&self) -> Result<Option<Held>, WriteErr> {
        match self.hold() {
            Ok(held) if held.parsed.read_only => Err(WriteErr::ReadOnly),
            Ok(held) => Ok(Some(held)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(WriteErr::ReadOnly),
            Err(e) => Err(WriteErr::Io(e)),
        }
    }
}

impl EditRecorder for Log {
    fn begin(&self) -> Result<Option<Box<dyn Recording + '_>>, WriteErr> {
        Ok(self
            .hold_for_edit()?
            .map(|held| Box::new(held) as Box<dyn Recording>))
    }
}

impl Recording for Held {
    fn sync(&mut self, doc: &Path, bytes: &[u8]) -> io::Result<()> {
        sync::try_sync_bytes(self, doc, bytes, epoch_ms()).map(drop)
    }

    fn record(&mut self, via: &Via, _from: &[u8], to: &[u8]) -> io::Result<()> {
        // Text the chain can't hold is left for the next sync to skip as well
        let Ok(text) = std::str::from_utf8(to) else {
            return Ok(());
        };
        let Some(recorded) = Chain::build(&self.parsed).record(text) else {
            return Ok(());
        };
        let (act, of, cid) = match via {
            Via::Checkbox { .. } => (Act::Checkbox, None, None),
            Via::Undo { of, cid } => (Act::Undo, Some(of.clone()), cid.clone()),
        };
        let mut ops = vec![Op::Edit {
            rev: recorded.rev,
            to_rev: recorded.to_rev.clone(),
            patch: recorded.patch,
            of,
            act,
        }];
        if let Some(text) = recorded.checkpoint {
            ops.push(Op::Snap {
                rev: recorded.to_rev,
                text,
                checkpoint: true,
            });
        }
        let ctx = AppendCtx {
            now_ms: epoch_ms(),
            by: By {
                kind: ByKind::Human,
                name: None,
                via: Some(ops::Via::Browser),
            },
            cid,
        };
        match self.append_with(ctx, |_, _| Ok(ops)) {
            Ok(_) => Ok(()),
            Err(AppendError::Io(e)) => Err(e),
            Err(AppendError::Rejected(r)) => Err(io::Error::other(format!("{r:?}"))),
        }
    }
}

/// A review file under its exclusive lock, read as it was when locked.
#[derive(Debug)]
pub struct Held {
    file: File,
    bytes: Vec<u8>,
    parsed: Parsed,
    /// Set by a failed write, after which the file's tail and line count are unknown.
    poisoned: bool,
}

impl Held {
    pub fn parsed(&self) -> &Parsed {
        &self.parsed
    }

    /// [`Log::append_with`] on the file this holds.
    pub fn append_with(
        &mut self,
        ctx: AppendCtx,
        build: impl FnOnce(&Parsed, u64) -> Result<Vec<Op>, Reject>,
    ) -> Result<Vec<u32>, AppendError> {
        if self.parsed.read_only {
            return Err(AppendError::Rejected(Reject::ReadOnly));
        }
        if self.poisoned {
            return Err(AppendError::Io(io::Error::other(
                "an earlier write to the review file failed",
            )));
        }
        let ops = build(&self.parsed, ctx.now_ms).map_err(AppendError::Rejected)?;
        let first = physical_lines(&self.bytes) + 1;
        let mut out = Vec::new();
        // Close a line left without its newline, so ours starts on a line of its own.
        if !self.bytes.ends_with(b"\n") {
            out.push(b'\n');
        }
        let mut landed = Vec::with_capacity(ops.len());
        let durable = ops.iter().any(|op| {
            matches!(
                op,
                Op::Submit { .. } | Op::Approve { .. } | Op::Claim { .. } | Op::Withdraw { .. }
            )
        });
        for (n, op) in (first..).zip(ops) {
            let line = Line {
                n,
                ts: Some(ctx.now_ms),
                by: ctx.by.clone(),
                cid: ctx.cid.clone(),
                op,
            };
            serde_json::to_writer(&mut out, &line).map_err(io::Error::other)?;
            out.push(b'\n');
            landed.push(n);
        }
        if landed.is_empty() {
            return Ok(landed);
        }
        let written = self.file.write_all(&out).and_then(|()| {
            if durable {
                self.file.sync_data()
            } else {
                Ok(())
            }
        });
        if let Err(e) = written {
            self.poisoned = true;
            return Err(AppendError::Io(e));
        }
        self.bytes.extend_from_slice(&out);
        self.parsed = ops::parse(&self.bytes);
        Ok(landed)
    }
}

/// The doc's permission bits without exec, so a private doc gets a private review file.
/// The owner can always write, so a read-only doc still takes notes. The folder file
/// follows its folder.
fn doc_mode(review: &Path) -> Option<u32> {
    let name = review.file_name()?.to_str()?;
    let doc = match name.strip_suffix(".birta.jsonl") {
        Some("") => review.parent()?.to_path_buf(),
        Some(doc) => review.with_file_name(doc),
        None => return None,
    };
    Some((fs::metadata(doc).ok()?.permissions().mode() & 0o666) | 0o200)
}

/// Lines as [`ops::parse`] numbers them: an unterminated last line counts.
fn physical_lines(bytes: &[u8]) -> u32 {
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    let open = usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    u32::try_from(newlines + open).unwrap_or(u32::MAX)
}

fn first_lines(doc_name: &str, baseline: &str, drafted_by: Option<&str>) -> io::Result<Vec<u8>> {
    let ts = Some(epoch_ms());
    let by = By {
        kind: ByKind::Birta,
        ..By::default()
    };
    let snap = Op::Snap {
        rev: Rev::of(baseline.as_bytes()),
        text: baseline.to_string(),
        checkpoint: false,
    };
    let owner = Op::Owner {
        drafted_by: drafted_by.map(str::to_string),
    };
    let mut out = serde_json::to_vec(&Header::new(doc_name)).map_err(io::Error::other)?;
    out.push(b'\n');
    for (n, op) in [(2, snap), (3, owner)] {
        let line = Line {
            n,
            ts,
            by: by.clone(),
            cid: None,
            op,
        };
        serde_json::to_writer(&mut out, &line).map_err(io::Error::other)?;
        out.push(b'\n');
    }
    Ok(out)
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Adds review files to the repo's `info/exclude` unless git already ignores this one.
/// Any failure, including no git, leaves things as they are.
fn exclude_from_git(review: &Path) {
    let (Some(dir), Some(name)) = (review.parent(), review.file_name()) else {
        return;
    };
    let git = |args: &[&str]| {
        let mut cmd = Command::new("git");
        // An inherited GIT_DIR (e.g. from a hook) would name another repo than dir's
        cmd.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        cmd
    };
    // 1 is "not ignored"; anything else is ignored, not a repo, or no git.
    let ignored = git(&["check-ignore", "-q", "--"])
        .arg(name)
        .stdout(Stdio::null())
        .status();
    if !matches!(ignored, Ok(status) if status.code() == Some(1)) {
        return;
    }
    let Ok(out) = git(&["rev-parse", "--git-path", "info/exclude"]).output() else {
        return;
    };
    let Ok(rel) = String::from_utf8(out.stdout) else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let _ = append_pattern(&dir.join(rel.trim_end_matches(['\n', '\r'])));
}

/// Appends the pattern under a lock, so creators in one repo add it once.
fn append_pattern(exclude: &Path) -> io::Result<()> {
    if let Some(info) = exclude.parent() {
        fs::create_dir_all(info)?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(exclude)?;
    file.lock()?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    if text.lines().any(|line| line.trim() == EXCLUDE_PATTERN) {
        return Ok(());
    }
    let sep = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    file.write_all(format!("{sep}{EXCLUDE_PATTERN}\n").as_bytes())
}

/// The ops for an approve of `text`. When the chain can't rebuild that text (a doc
/// recorded by rev only), a snap of it goes first so the approved text stays readable.
pub fn approve_ops(chain: &Chain, text: &str, not_before: u64, hold_ms: u64) -> Vec<Op> {
    let rev = Rev::of(text.as_bytes());
    let mut ops = Vec::with_capacity(2);
    if chain.text(&rev).is_none() {
        ops.push(Op::Snap {
            rev: rev.clone(),
            text: text.to_string(),
            checkpoint: true,
        });
    }
    ops.push(Op::Approve {
        rev,
        not_before,
        hold_ms,
    });
    ops
}

/// What changed in the file since the last poll.
#[derive(Debug)]
pub enum Tail {
    /// Whole new lines at the end.
    Appended(Vec<ParsedLine>),
    /// Anything else: read the file again from the start.
    Reset,
    Unchanged,
}

/// Follows a review file's growth by offset, noticing when it was replaced or rewritten.
#[derive(Debug, Default)]
pub struct Tailer {
    seen: Option<Seen>,
}

#[derive(Debug)]
struct Seen {
    id: (u64, u64),
    /// File length when last read from the start.
    len: u64,
    /// End of the last whole line read.
    off: u64,
    /// Hash of the bytes just before `off`.
    before: [u8; 20],
    next_n: u32,
}

impl Tailer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn poll(&mut self, log: &Log) -> Tail {
        match self.advance(log) {
            Ok(tail) => tail,
            Err(_) => {
                let had = self.seen.take().is_some();
                if had { Tail::Reset } else { Tail::Unchanged }
            }
        }
    }

    fn advance(&mut self, log: &Log) -> io::Result<Tail> {
        let mut file = File::open(log.path())?;
        let meta = file.metadata()?;
        let id = (meta.dev(), meta.ino());
        let len = meta.len();
        let Some(seen) = self.seen.as_mut().filter(|s| s.id == id) else {
            return self.restart(file, id);
        };
        if len == seen.off || (seen.off == 0 && len == seen.len) {
            return Ok(Tail::Unchanged);
        }
        // Before the first whole line the header isn't read yet, so start over.
        if len < seen.off || seen.off == 0 {
            return self.restart(file, id);
        }
        let start = seen.off.saturating_sub(TAIL_CHECK as u64);
        file.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let kept = usize::try_from(seen.off - start).map_err(io::Error::other)?;
        if buf.len() < kept || digest(&buf[..kept]) != seen.before {
            return self.restart(file, id);
        }
        let Some(end) = buf[kept..].iter().rposition(|&b| b == b'\n') else {
            return Ok(Tail::Unchanged);
        };
        let whole = &buf[kept..kept + end];
        let mut lines = Vec::new();
        for raw in whole.split(|&b| b == b'\n') {
            lines.push(ops::parse_raw_line(seen.next_n, raw));
            seen.next_n += 1;
        }
        let new_off = kept + end + 1;
        seen.off += u64::try_from(end + 1).map_err(io::Error::other)?;
        seen.before = digest(&buf[new_off.saturating_sub(TAIL_CHECK)..new_off]);
        Ok(Tail::Appended(lines))
    }

    /// Takes the file as it is now as the new starting point.
    fn restart(&mut self, mut file: File, id: (u64, u64)) -> io::Result<Tail> {
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let off = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let whole = &bytes[..off];
        self.seen = Some(Seen {
            id,
            len: bytes.len() as u64,
            off: off as u64,
            before: digest(&whole[off.saturating_sub(TAIL_CHECK)..]),
            next_n: physical_lines(whole) + 1,
        });
        Ok(Tail::Reset)
    }
}

fn digest(bytes: &[u8]) -> [u8; 20] {
    sha1_smol::Sha1::from(bytes).digest().bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc_write::{self, Expect, WriteTarget};

    const TASKS: &[u8] = b"- [ ] a\n";

    /// `pr.md` holding `text`, and its review file.
    fn logged(dir: &Path, text: &[u8]) -> (PathBuf, Log) {
        let doc = dir.join("pr.md");
        fs::write(&doc, text).unwrap();
        let log = Log::open(&dir.join("pr.md.birta.jsonl"));
        log.create_if_missing("pr.md", std::str::from_utf8(text).unwrap(), None)
            .unwrap();
        (doc, log)
    }

    /// Makes every write through `held` fail, as a full disk would.
    fn break_writes(held: &mut Held, path: &Path) {
        held.file = File::open(path).unwrap();
    }

    fn ctx() -> AppendCtx {
        AppendCtx {
            now_ms: 1,
            by: By::default(),
            cid: None,
        }
    }

    fn owner() -> Vec<Op> {
        vec![Op::Owner { drafted_by: None }]
    }

    #[test]
    fn a_held_file_whose_write_failed_takes_no_more_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (_, log) = logged(dir.path(), TASKS);
        let mut held = log.hold().unwrap();
        break_writes(&mut held, log.path());
        assert!(held.append_with(ctx(), |_, _| Ok(owner())).is_err());

        held.file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(log.path())
            .unwrap();
        let before = fs::read(log.path()).unwrap();
        let again = held.append_with(ctx(), |_, _| Ok(owner()));
        assert!(again.is_err(), "wrote after a failed write: {again:?}");
        assert_eq!(fs::read(log.path()).unwrap(), before);
    }

    /// A review file whose every write fails once it is held.
    struct Full(Log);

    impl EditRecorder for Full {
        fn begin(&self) -> Result<Option<Box<dyn Recording + '_>>, WriteErr> {
            let mut held = self.0.hold_for_edit()?.expect("a review file");
            break_writes(&mut held, self.0.path());
            Ok(Some(Box::new(held)))
        }
    }

    #[test]
    fn an_outside_write_that_cannot_be_recorded_stops_the_doc_write() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, log) = logged(dir.path(), TASKS);
        let outside = b"- [ ] a\n- [ ] b\n";
        fs::write(&doc, outside).unwrap();
        let t = WriteTarget {
            canonical: doc.clone(),
            rel: "pr.md".to_string(),
        };
        let result = doc_write::write_doc(
            &t,
            Some(&Full(log)),
            Expect::Fresh,
            Via::Checkbox { line: 1 },
            |b| doc_write::toggle_task_line(b, 1, true).ok_or(WriteErr::NotApplicable),
        );
        assert!(result.is_err(), "{result:?}");
        assert_eq!(fs::read(&doc).unwrap(), outside, "the doc was replaced");
    }
}
