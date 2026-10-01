//! `sync_external`: a doc write nobody recorded lands in the review file once, under its
//! lock, and the fold credits it by the claim window. Called as a library, no server.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use birta::agent::review_cmd;
use birta::rev::Rev;
use birta::review::chain::Chain;
use birta::review::doc::Doc;
use birta::review::fold;
use birta::review::log::{AppendCtx, Log};
use birta::review::ops::{self, By, ByKind, Line, Op, Via};
use birta::review::review_path_for;
use birta::review::state::{Attr, ReviewState};
use birta::review::sync::{sync_bytes_with, sync_external};

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, Scenario, ts};

const EDITED: &str = "- second item, because S3 flakes";

fn agent(name: &str) -> By {
    By {
        kind: ByKind::Agent,
        name: Some(name.to_string()),
        via: Some(Via::Cli),
    }
}

/// A clock well after every line the kit writes.
fn later() -> u64 {
    ts(10_000)
}

fn epoch(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

/// A scenario's doc and review file in a temp folder.
struct Placed {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    doc: PathBuf,
}

impl Placed {
    fn new(s: &Scenario) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let doc = dir.join("pr.md");
        fs::write(&doc, s.text()).unwrap();
        fs::write(review_path_for(&doc), s.log()).unwrap();
        Self {
            _tmp: tmp,
            dir,
            doc,
        }
    }

    fn log(&self) -> Log {
        Log::open(&review_path_for(&self.doc))
    }

    fn bytes(&self) -> Vec<u8> {
        fs::read(review_path_for(&self.doc)).unwrap()
    }

    /// Writes the doc and dates it `mtime_ms`.
    fn edit(&self, text: &str, mtime_ms: u64) {
        fs::write(&self.doc, text).unwrap();
        self.touch(mtime_ms);
    }

    fn touch(&self, mtime_ms: u64) {
        File::options()
            .write(true)
            .open(&self.doc)
            .unwrap()
            .set_modified(epoch(mtime_ms))
            .unwrap();
    }

    fn sync(&self, now: u64) -> Option<u32> {
        let mut held = self.log().hold().unwrap();
        sync_external(&mut held, &self.doc, now)
    }

    /// [`Placed::sync`] with all the time git needs, so a loaded machine can't lose the
    /// head: only the timeout test depends on git's timing.
    fn sync_git(&self, now: u64) -> Option<u32> {
        let mut held = self.log().hold().unwrap();
        let bytes = fs::read(&self.doc).unwrap();
        sync_bytes_with(&mut held, &self.doc, &bytes, now, Duration::from_secs(30))
    }

    fn lines(&self) -> Vec<Line> {
        let parsed = ops::parse(self.bytes());
        assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
        parsed.lines
    }

    fn line(&self, n: u32) -> Line {
        self.lines()
            .into_iter()
            .find(|l| l.n == n)
            .unwrap_or_else(|| panic!("no line {n}"))
    }

    fn externals(&self) -> Vec<Line> {
        self.lines()
            .into_iter()
            .filter(|l| matches!(l.op, Op::External { .. }))
            .collect()
    }

    fn fold(&self) -> ReviewState {
        let parsed = ops::parse(self.bytes());
        let chain = Chain::build(&parsed);
        let text = fs::read_to_string(&self.doc).unwrap();
        let doc = Doc::new(&text);
        fold::fold(&parsed, &chain, &doc).state
    }
}

/// A note handed back on `DOC`; with `claimed`, claude has picked it up. Returns the
/// scenario and the claim line.
fn handed(claimed: bool) -> (Scenario, Option<u32>) {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let claim = claimed.then(|| s.claim("claude"));
    (s, claim)
}

fn edited(s: &Scenario) -> String {
    s.replaced("- second item", EDITED)
}

// --- recording ---

#[test]
fn a_doc_at_the_recorded_rev_writes_nothing() {
    let (s, _) = handed(true);
    let p = Placed::new(&s);
    let before = p.bytes();
    assert_eq!(p.sync(later()), None);
    assert_eq!(p.bytes(), before);
}

#[test]
fn a_write_is_recorded_once_as_an_external_from_the_head() {
    let (s, _) = handed(true);
    let p = Placed::new(&s);
    let new = edited(&s);
    p.edit(&new, later() - 1_000);
    let before = p.bytes();

    let n = p.sync(later()).expect("the write is recorded");
    let line = p.line(n);
    assert_eq!(line.by.kind, ByKind::Birta);
    assert_eq!(line.ts, Some(later()));
    let Op::External {
        rev, to_rev, patch, ..
    } = &line.op
    else {
        panic!("line {n} is {:?}", line.op);
    };
    assert_eq!(*rev, Rev::of(s.text().as_bytes()));
    assert_eq!(*to_rev, Rev::of(new.as_bytes()));
    assert!(patch.as_ref().is_some_and(|p| !p.is_empty()), "{patch:?}");
    let after = p.bytes();
    assert_eq!(&after[..before.len()], &before[..], "only appended");
    let chain = Chain::build(&ops::parse(&after));
    assert_eq!(chain.head(), Some(&Rev::of(new.as_bytes())));
    assert_eq!(chain.text(to_rev).as_deref(), Some(new.as_str()));

    assert_eq!(
        p.sync(later() + 1),
        None,
        "recorded, so nothing left to sync"
    );
    assert_eq!(p.bytes(), after);
}

#[test]
fn a_same_length_edit_with_its_mtime_restored_is_still_recorded() {
    let (s, _) = handed(true);
    let p = Placed::new(&s);
    p.touch(later() - 5_000);
    let new = s.text().replacen("- second item", "- secoND item", 1);
    assert_eq!(new.len(), s.text().len());
    p.edit(&new, later() - 5_000);

    assert!(p.sync(later()).is_some(), "the rev changed");
    assert_eq!(p.fold().changes.len(), 1);
}

#[test]
fn a_missing_doc_writes_nothing() {
    let (s, _) = handed(true);
    let p = Placed::new(&s);
    fs::remove_file(&p.doc).unwrap();
    let before = p.bytes();
    assert_eq!(p.sync(later()), None);
    assert_eq!(p.bytes(), before);
}

#[test]
fn a_write_due_a_checkpoint_lands_with_its_snap() {
    let (mut s, _) = handed(true);
    let mut text = s.text().to_string();
    // Patches since the last snap, one short of a checkpoint.
    for i in 0.. {
        let chain = Chain::build(&s.parsed());
        let next = format!("{text}\nline {i}\n");
        if chain.record(&next).unwrap().checkpoint.is_some() {
            break;
        }
        s.external(&next);
        text = next;
    }
    let p = Placed::new(&s);
    let new = format!("{text}\nthe write due a checkpoint\n");
    p.edit(&new, later() - 1_000);

    let n = p.sync(later()).expect("recorded");
    let lines = p.lines();
    let last = lines.last().unwrap();
    assert_eq!(last.n, n + 1, "the snap follows the external");
    assert!(
        matches!(&last.op, Op::Snap { rev, text, checkpoint: true } if *rev == Rev::of(new.as_bytes()) && *text == new),
        "{:?}",
        last.op
    );
}

#[test]
fn two_syncs_racing_write_one_external() {
    for i in 0..50 {
        let (s, _) = handed(true);
        let p = Placed::new(&s);
        p.edit(&edited(&s), later() - 1_000);
        let barrier = Arc::new(Barrier::new(2));
        let racers: Vec<_> = (0..2)
            .map(|_| {
                let (log, doc, barrier) = (p.log(), p.doc.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut held = log.hold().unwrap();
                    sync_external(&mut held, &doc, later())
                })
            })
            .collect();
        let wrote: Vec<Option<u32>> = racers.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(
            wrote.iter().filter(|w| w.is_some()).count(),
            1,
            "iteration {i}: {wrote:?}"
        );
        assert_eq!(p.externals().len(), 1, "iteration {i}");
    }
}

// --- crediting, through the fold ---

#[test]
fn a_write_inside_the_open_claim_is_the_claimers() {
    let (s, claim) = handed(true);
    let claim = claim.unwrap();
    let p = Placed::new(&s);
    p.edit(&edited(&s), ts(claim) + 1_000);
    p.sync(later()).expect("recorded");

    let state = p.fold();
    let change = &state.changes[0];
    assert_eq!(change.attr, Attr::Agent, "{change:?}");
    assert_eq!(change.by.name.as_deref(), Some("claude"));
    assert_eq!(change.turn, Some(claim));
}

#[test]
fn a_write_after_the_yield_is_outside() {
    let (mut s, claim) = handed(true);
    let yielded = s.yield_turn("claude", claim.unwrap());
    let p = Placed::new(&s);
    p.edit(&edited(&s), ts(yielded) + 1_000);
    p.sync(later()).expect("recorded");

    let state = p.fold();
    assert_eq!(state.changes[0].attr, Attr::Outside);
    assert_eq!(state.changes[0].turn, None);
}

#[test]
fn a_write_with_nobody_claiming_is_outside() {
    let (s, _) = handed(false);
    let p = Placed::new(&s);
    p.edit(&edited(&s), later() - 1_000);
    p.sync(later()).expect("recorded");
    assert_eq!(p.fold().changes[0].attr, Attr::Outside);
}

/// Crediting is by log position: a claim syncs first under its lock, so a write made
/// before the pickup lands before the claim line, whatever the doc's mtime.
#[test]
fn a_write_made_before_the_pickup_lands_before_the_claim_as_outside() {
    let (s, _) = handed(false);
    let p = Placed::new(&s);
    p.edit(&edited(&s), later() - 1_000);
    let claim = review_cmd::claim(&p.doc, &agent("claude"), later())
        .unwrap()
        .expect("the round is due");

    let external = p.externals();
    assert_eq!(external.len(), 1, "the claim records the write first");
    assert!(
        external[0].n < claim,
        "external {}, claim {claim}",
        external[0].n
    );
    let state = p.fold();
    assert_eq!(state.changes[0].attr, Attr::Outside);
    assert_eq!(state.changes[0].turn, None);
}

// --- git HEAD ---

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        // A pre-commit hook exports these, and they would aim git at the outer repo
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `git init` in the doc's folder, with one commit unless `unborn`.
fn repo(p: &Placed, unborn: bool) -> Option<String> {
    git(&p.dir, &["init", "-q"]);
    if unborn {
        return None;
    }
    git(&p.dir, &["commit", "-q", "--allow-empty", "-m", "one"]);
    Some(git(&p.dir, &["rev-parse", "HEAD"]))
}

fn head_and_cause(line: &Line) -> (Option<String>, Option<String>) {
    match &line.op {
        Op::External { head, cause, .. } => (head.clone(), cause.clone()),
        other => panic!("not an external: {other:?}"),
    }
}

#[test]
fn a_write_nobody_claimed_records_the_repos_head() {
    let (s, _) = handed(false);
    let p = Placed::new(&s);
    let head = repo(&p, false);
    p.edit(&edited(&s), later() - 1_000);
    let n = p.sync_git(later()).expect("recorded");
    assert_eq!(head_and_cause(&p.line(n)), (head, None));
}

#[test]
fn a_claimed_write_records_the_head_too() {
    let (s, claim) = handed(true);
    let p = Placed::new(&s);
    let head = repo(&p, false);
    p.edit(&edited(&s), ts(claim.unwrap()) + 1_000);
    let n = p.sync_git(later()).expect("recorded");
    assert_eq!(head_and_cause(&p.line(n)), (head, None));
}

#[test]
fn outside_a_repo_or_before_its_first_commit_there_is_no_head() {
    for in_repo in [false, true] {
        let (s, _) = handed(false);
        let p = Placed::new(&s);
        if in_repo {
            repo(&p, true);
        }
        p.edit(&edited(&s), later() - 1_000);
        let n = p.sync_git(later()).expect("recorded");
        assert_eq!(
            head_and_cause(&p.line(n)),
            (None, None),
            "unborn: {in_repo}"
        );
    }
}

/// Proposed: `cause:"git"` when HEAD moved since the last recorded head, so a checkout
/// or pull is labelled and never credited to a claimer.
#[test]
fn a_write_after_head_moved_is_labelled_git() {
    let (s, _) = handed(false);
    let p = Placed::new(&s);
    let first = repo(&p, false);
    let one = edited(&s);
    p.edit(&one, later() - 3_000);
    let n = p.sync_git(later() - 2_000).unwrap();
    assert_eq!(head_and_cause(&p.line(n)), (first, None));

    git(&p.dir, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let second = git(&p.dir, &["rev-parse", "HEAD"]);
    let two = format!("{one}\nfrom a pull\n");
    p.edit(&two, later() - 1_000);
    let n = p.sync_git(later() - 500).unwrap();
    assert_eq!(
        head_and_cause(&p.line(n)),
        (Some(second.clone()), Some("git".to_string()))
    );

    p.edit(&format!("{two}\nby hand\n"), later() - 100);
    let n = p.sync_git(later()).unwrap();
    assert_eq!(
        head_and_cause(&p.line(n)),
        (Some(second), None),
        "HEAD stayed"
    );
}

#[test]
fn a_write_back_to_an_earlier_rev_is_recorded() {
    let (s, _) = handed(true);
    let p = Placed::new(&s);
    let new = edited(&s);
    p.edit(&new, later() - 2_000);
    p.sync(later() - 1_000).expect("the edit");
    p.edit(s.text(), later() - 500);
    let n = p
        .sync(later())
        .expect("the head moved, even to a rev the chain knows");
    let Op::External { rev, to_rev, .. } = &p.line(n).op else {
        panic!("{:?}", p.line(n).op);
    };
    assert_eq!(*rev, Rev::of(new.as_bytes()));
    assert_eq!(*to_rev, Rev::of(s.text().as_bytes()));
    let chain = Chain::build(&ops::parse(p.bytes()));
    assert_eq!(chain.head(), Some(&Rev::of(s.text().as_bytes())));
}

#[test]
fn a_write_dated_before_every_line_is_still_recorded() {
    let (s, _) = handed(true);
    let p = Placed::new(&s);
    p.edit(&edited(&s), ts(1));
    assert!(p.sync(later()).is_some());
}

#[test]
fn a_doc_that_is_no_longer_a_file_writes_nothing() {
    let (s, _) = handed(true);
    let p = Placed::new(&s);
    fs::remove_file(&p.doc).unwrap();
    fs::create_dir(&p.doc).unwrap();
    let before = p.bytes();
    assert_eq!(p.sync(later()), None);
    assert_eq!(p.bytes(), before);
}

#[test]
fn a_bom_crlf_doc_is_recorded_byte_for_byte() {
    let text = format!("\u{feff}{}", DOC.replace('\n', "\r\n"));
    let mut s = Scenario::drafted_by(&text, "claude");
    s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let claim = s.claim("claude");
    let p = Placed::new(&s);
    let new = text.replacen(
        "- second item\r\n",
        "- second item, because S3 flakes\r\n",
        1,
    ) + "no final newline";
    p.edit(&new, ts(claim) + 1_000);
    let n = p.sync(later()).expect("recorded");
    let Op::External { to_rev, .. } = &p.line(n).op else {
        panic!("{:?}", p.line(n).op);
    };
    assert_eq!(*to_rev, Rev::of(new.as_bytes()));
    let chain = Chain::build(&ops::parse(p.bytes()));
    assert_eq!(chain.text(to_rev).as_deref(), Some(new.as_str()));
    let state = p.fold();
    assert!(!state.changes.is_empty());
    assert!(
        state.changes.iter().all(|c| c.attr == Attr::Agent),
        "{:?}",
        state.changes
    );
}

#[test]
fn a_write_after_the_turn_ended_records_the_head() {
    for release in [false, true] {
        let (mut s, claim) = handed(true);
        let claim = claim.unwrap();
        if release {
            s.release(claim);
        } else {
            s.yield_turn("claude", claim);
        }
        let p = Placed::new(&s);
        let head = repo(&p, false);
        p.edit(&edited(&s), later() - 1_000);
        let n = p.sync_git(later()).expect("recorded");
        assert_eq!(
            head_and_cause(&p.line(n)),
            (head, None),
            "released: {release}"
        );
    }
}

/// Claimed by log position, so an mtime from before the claim changes nothing.
#[test]
fn a_claimed_write_is_never_git_whatever_its_mtime() {
    let (s, _) = handed(true);
    let p = Placed::new(&s);
    let head = repo(&p, false);
    p.edit(&edited(&s), ts(1));
    let n = p.sync_git(later()).expect("recorded");
    assert_eq!(head_and_cause(&p.line(n)), (head, None));
}

/// Every external records `head`, so a commit made inside a claim is the baseline the
/// next unclaimed write compares with.
#[test]
fn head_moving_inside_a_claim_labels_no_claimed_write() {
    let (s, _) = handed(false);
    let p = Placed::new(&s);
    let first = repo(&p, false);
    let one = edited(&s);
    p.edit(&one, later() - 5_000);
    let n = p.sync_git(later() - 4_000).unwrap();
    assert_eq!(head_and_cause(&p.line(n)), (first, None));
    let claim = review_cmd::claim(&p.doc, &agent("claude"), later() - 3_000)
        .unwrap()
        .expect("due");

    git(&p.dir, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let second = git(&p.dir, &["rev-parse", "HEAD"]);
    let two = format!("{one}\nwritten in the turn\n");
    p.edit(&two, later() - 2_500);
    let n = p.sync_git(later() - 2_000).unwrap();
    assert_eq!(
        head_and_cause(&p.line(n)),
        (Some(second.clone()), None),
        "a claimed write is the claimer's, HEAD or not"
    );

    p.log()
        .hold()
        .unwrap()
        .append_with(
            AppendCtx {
                now_ms: later() - 1_500,
                by: agent("claude"),
                cid: None,
            },
            |_, _| {
                Ok(vec![Op::Yield {
                    turn: Some(claim),
                    release: false,
                }])
            },
        )
        .unwrap();
    p.edit(&format!("{two}\nby hand\n"), later() - 1_000);
    let n = p.sync_git(later()).unwrap();
    // The claimed write recorded `second`, so HEAD has not moved since
    assert_eq!(head_and_cause(&p.line(n)), (Some(second), None));
}

#[test]
fn a_fifo_in_place_of_the_doc_is_skipped_without_blocking_the_lock() {
    let (s, _) = handed(false);
    let p = Placed::new(&s);
    let before = p.bytes();
    fs::remove_file(&p.doc).unwrap();
    let path = std::ffi::CString::new(p.doc.to_str().unwrap()).unwrap();
    // SAFETY: a valid C string for the call.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o644) }, 0);
    let (log, doc) = (p.log(), p.doc.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut held = log.hold().unwrap();
        let _ = tx.send(sync_external(&mut held, &doc, later()));
    });
    let synced = rx.recv_timeout(Duration::from_secs(1));
    if synced.is_err() {
        // Unblocks the stuck open so the thread can end
        let _ = fs::OpenOptions::new().write(true).open(&p.doc);
    }
    assert_eq!(synced, Ok(None), "sync blocked on the FIFO");
    assert_eq!(p.bytes(), before);
}
