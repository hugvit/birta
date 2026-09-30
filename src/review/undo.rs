//! `x` undo: reverses one recorded write of the doc through the strict write.

use std::io;

use super::chain::Chain;
use super::doc::Doc;
use super::fold;
use super::hunk;
use super::log::Log;
use super::ops::{Act, Hunk, Op, Parsed};
use crate::doc_write::{self, Recording, Stage, Via, WriteErr, WriteTarget, Written};
use crate::paths::DocRef;
use crate::rev::Rev;

#[derive(Debug)]
#[expect(dead_code, reason = "undo is not served yet")]
pub(crate) enum UndoErr {
    /// The doc isn't the rev the page folded.
    ChangedSince,
    NotReversible,
    /// The doc, or the review file that must record the undo, can't be written.
    ReadOnly,
    Io(io::Error),
}

impl From<WriteErr> for UndoErr {
    fn from(e: WriteErr) -> Self {
        match e {
            WriteErr::ChangedSince => Self::ChangedSince,
            WriteErr::NotApplicable => Self::NotReversible,
            WriteErr::ReadOnly => Self::ReadOnly,
            WriteErr::Io(e) => Self::Io(e),
        }
    }
}

/// Reverts the write `change` belongs to, when the doc's bytes are still `client_rev`.
/// A write of more than one hunk is reverted whole, and only with `confirm`.
#[cfg_attr(not(test), expect(dead_code, reason = "undo is not served yet"))]
pub(crate) fn reverse_change(
    doc: &DocRef,
    change: &str,
    client_rev: &Rev,
    confirm: bool,
) -> Result<Written, UndoErr> {
    reverse_change_with(doc, change, client_rev, confirm, |_| {})
}

/// [`reverse_change`], telling `stage` how far the write got.
fn reverse_change_with(
    doc: &DocRef,
    change: &str,
    client_rev: &Rev,
    confirm: bool,
    mut stage: impl FnMut(Stage),
) -> Result<Written, UndoErr> {
    // The fold, the write and the record all happen under this one lock
    let Some(mut held) = Log::open(&doc.review_path).hold_for_edit()? else {
        return Err(UndoErr::NotReversible);
    };
    let before = std::fs::read(&doc.canonical).map_err(UndoErr::Io)?;
    stage(Stage::Read);
    if Rev::of(&before) != *client_rev {
        return Err(UndoErr::ChangedSince);
    }
    held.sync(&doc.canonical, &before);
    let text = std::str::from_utf8(&before).map_err(|_| UndoErr::NotReversible)?;
    let reverses = reverses(held.parsed(), text, change, confirm).ok_or(UndoErr::NotReversible)?;
    let after = apply_reverse(&before, &reverses).ok_or(UndoErr::NotReversible)?;
    let target = WriteTarget {
        canonical: doc.canonical.clone(),
        rel: doc.rel.clone(),
    };
    let via = Via::Undo {
        of: change.to_string(),
    };
    Ok(doc_write::replace(
        &target,
        Some(&mut held),
        &via,
        &before,
        &after,
        &mut stage,
    )?)
}

/// The hunks that revert the whole write `change` belongs to, in `text`'s lines, when
/// every one of them can be undone.
fn reverses(parsed: &Parsed, text: &str, change: &str, confirm: bool) -> Option<Vec<Hunk>> {
    let chain = Chain::build(parsed);
    let doc = Doc::new(text);
    let changes = fold::fold(parsed, &chain, &doc).state.changes;
    let pressed = changes.iter().find(|c| c.id == change)?;
    if pressed.op_hunks > 1 && !confirm {
        return None;
    }
    // Redoing is editing again
    let is_undo = parsed
        .lines
        .iter()
        .any(|l| l.n == pressed.op && matches!(l.op, Op::Edit { act: Act::Undo, .. }));
    if is_undo {
        return None;
    }
    let mut reverses: Vec<Hunk> = changes
        .iter()
        .filter(|c| c.op == pressed.op)
        .map(|c| c.reverse.clone())
        .collect::<Option<_>>()?;
    if reverses.len() != pressed.op_hunks as usize {
        return None;
    }
    reverses.sort_by_key(|h| h.a);
    Some(reverses)
}

/// `bytes` with `reverses` applied, each only where its lines are exactly as it expects.
pub(crate) fn apply_reverse(bytes: &[u8], reverses: &[Hunk]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    hunk::apply(text, reverses).ok().map(String::into_bytes)
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::thread::JoinHandle;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::doc_write::{Expect, WriteErr};
    use crate::paths::{self, Access, Roots};
    use crate::review::chain::Chain;
    use crate::review::doc::Doc;
    use crate::review::fold;
    use crate::review::hunk;
    use crate::review::log::{AppendCtx, Log};
    use crate::review::ops::{self, Act, At, By, ByKind, Line, Op};
    use crate::review::review_path_for;
    use crate::review::state::{Attr, Change, ReversibleReason};
    use crate::review::sync::sync_external;

    fn human() -> By {
        By {
            kind: ByKind::Human,
            name: None,
            via: Some(ops::Via::Browser),
        }
    }

    fn agent() -> By {
        By {
            kind: ByKind::Agent,
            name: Some("claude".to_string()),
            via: Some(ops::Via::Cli),
        }
    }

    /// A doc, its review file, and a hand-back an agent has claimed. Every line it
    /// appends is a second after the last, starting an hour ago.
    struct Fixture {
        _tmp: tempfile::TempDir,
        dir: PathBuf,
        doc: PathBuf,
        log: Log,
        clock: u64,
        claim: u32,
    }

    impl Fixture {
        fn claimed(base: &str) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().canonicalize().unwrap();
            let doc = dir.join("doc.md");
            fs::write(&doc, base).unwrap();
            let log = Log::open(&review_path_for(&doc));
            assert!(
                log.create_if_missing("doc.md", base, None).unwrap(),
                "setup"
            );
            let hour_ago = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let mut f = Self {
                _tmp: tmp,
                dir,
                doc,
                log,
                clock: hour_ago.as_millis() as u64 - 3_600_000,
                claim: 0,
            };
            let note = f.append(
                human(),
                Op::Comment {
                    at: At::Doc,
                    verdict: None,
                    body: Some("tighten this".to_string()),
                    to: None,
                    kind: None,
                    turn: None,
                },
            );
            let due = f.clock + 1_000;
            let round = f.append(
                human(),
                Op::Submit {
                    upto: note,
                    not_before: due,
                    hold_ms: 0,
                    to: None,
                    batch: None,
                },
            );
            f.claim = f.append(agent(), Op::Claim { round });
            f
        }

        fn tick(&mut self) -> u64 {
            self.clock += 1_000;
            self.clock
        }

        fn append(&mut self, by: By, op: Op) -> u32 {
            let ctx = AppendCtx {
                now_ms: self.tick(),
                by,
                cid: None,
            };
            self.log.append(ctx, vec![op], |_, _| Ok(())).unwrap()[0]
        }

        /// Ends the agent's turn, so later writes are outside ones.
        fn yield_turn(&mut self) {
            let turn = Some(self.claim);
            self.append(
                agent(),
                Op::Yield {
                    turn,
                    release: false,
                },
            );
        }

        /// Writes `text` and records it; the line of its `external`.
        fn writes(&mut self, text: &str) -> u32 {
            fs::write(&self.doc, text).unwrap();
            self.sync().expect("setup: write recorded")
        }

        fn sync(&mut self) -> Option<u32> {
            let now = self.tick();
            let mut held = self.log.hold().unwrap();
            sync_external(&mut held, &self.doc, now)
        }

        fn docref(&self) -> DocRef {
            DocRef {
                rel: "doc.md".to_string(),
                canonical: self.doc.clone(),
                review_path: self.log.path().to_path_buf(),
                writable: paths::is_writable(&self.doc),
                log_writable: true,
            }
        }

        fn undo(&self, id: &str, client: &[u8], confirm: bool) -> Result<Written, UndoErr> {
            reverse_change(&self.docref(), id, &Rev::of(client), confirm)
        }

        fn disk(&self) -> Vec<u8> {
            fs::read(&self.doc).unwrap()
        }

        fn log_bytes(&self) -> Vec<u8> {
            fs::read(self.log.path()).unwrap()
        }

        fn lines(&self) -> Vec<Line> {
            let parsed = ops::parse(self.log_bytes());
            assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
            parsed.lines
        }

        fn count(&self, pick: fn(&Op) -> bool) -> usize {
            self.lines().iter().filter(|l| pick(&l.op)).count()
        }

        fn head(&self) -> Option<Rev> {
            Chain::build(&self.log.read().unwrap().unwrap())
                .head()
                .cloned()
        }

        /// Change `id` as the fold shows it on the disk text now.
        fn change(&self, id: &str) -> Change {
            let parsed = self.log.read().unwrap().unwrap();
            let chain = Chain::build(&parsed);
            let text = String::from_utf8(self.disk()).unwrap();
            let doc = Doc::new(&text);
            let folded = fold::fold(&parsed, &chain, &doc);
            let found = folded.state.changes.iter().find(|c| c.id == id).cloned();
            found.unwrap_or_else(|| panic!("setup: no change {id}"))
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    fn is_edit(op: &Op) -> bool {
        matches!(op, Op::Edit { .. })
    }

    fn is_external(op: &Op) -> bool {
        matches!(op, Op::External { .. })
    }

    /// The agent's single-hunk change of `base` into `changed`, checked reversible.
    fn agent_change(base: &str, changed: &str) -> (Fixture, String) {
        let mut f = Fixture::claimed(base);
        let id = format!("{}.0", f.writes(changed));
        let c = f.change(&id);
        assert_eq!(c.attr, Attr::Agent, "setup: {c:?}");
        assert!(c.reversible.ok && c.op_hunks == 1, "setup: {c:?}");
        (f, id)
    }

    /// The last line is the `edit` that undid `id`, turning `from` into `to`.
    fn assert_undo_edit(f: &Fixture, id: &str, from: &[u8], to: &[u8]) {
        let lines = f.lines();
        let last = lines.last().unwrap();
        let Op::Edit {
            rev,
            to_rev,
            patch,
            of,
            act,
        } = &last.op
        else {
            panic!("last line is not an edit: {last:?}");
        };
        assert_eq!((*act, of.as_deref()), (Act::Undo, Some(id)));
        assert_eq!((rev, to_rev), (&Rev::of(from), &Rev::of(to)));
        let (from, to) = (std::str::from_utf8(from), std::str::from_utf8(to));
        let patched = hunk::apply(from.unwrap(), patch.as_ref().expect("a patch"));
        assert_eq!(patched.as_deref(), Ok(to.unwrap()));
        assert_eq!(last.by.kind, ByKind::Human);
        assert_eq!(f.head(), Some(Rev::of(&f.disk())));
        assert_eq!(f.change(id).undone_by, Some(last.n));
    }

    #[test]
    fn undo_restores_the_bytes_exactly_whatever_the_line_endings() {
        let cases = [
            ("lf", "# T\n\nold line\n\nend\n", "# T\n\nnew line\n\nend\n"),
            (
                "crlf",
                "# T\r\n\r\nold line\r\n\r\nend\r\n",
                "# T\r\n\r\nnew line\r\n\r\nend\r\n",
            ),
            (
                "mixed",
                "# T\r\n\nold line\n\r\nend\r\n",
                "# T\r\n\nnew line\n\r\nend\r\n",
            ),
            (
                "lone cr",
                "# T\r\rold line\r\rend\r",
                "# T\r\rnew line\r\rend\r",
            ),
            (
                "bom, line 1",
                "\u{feff}# Title\n\nbody\n",
                "\u{feff}# Title two\n\nbody\n",
            ),
            (
                "bom and crlf, line 1",
                "\u{feff}# Title\r\n\r\nbody\r\n",
                "\u{feff}# Title two\r\n\r\nbody\r\n",
            ),
            ("no final eol", "# T\n\nold line", "# T\n\nnew line"),
            ("final eol added", "# T\n\nold line", "# T\n\nold line\n"),
            ("final eol removed", "# T\n\nold line\n", "# T\n\nnew line"),
            ("insertion", "# T\n\nend\n", "# T\n\nnew\n\nend\n"),
            (
                "deletion",
                "# T\n\nkeep\n\ngone\n\nend\n",
                "# T\n\nkeep\n\nend\n",
            ),
            (
                "one eol flipped",
                "# T\n\nold line\n",
                "# T\n\nold line\r\n",
            ),
            ("lf to crlf", "# T\n\nold line\n", "# T\r\n\r\nold line\r\n"),
            (
                "bom added",
                "# Title\n\nbody\n",
                "\u{feff}# Title\n\nbody\n",
            ),
            (
                "bom removed",
                "\u{feff}# Title\n\nbody\n",
                "# Title\n\nbody\n",
            ),
            (
                "bom, line 3",
                "\u{feff}# T\n\nold line\n",
                "\u{feff}# T\n\nnew line\n",
            ),
            ("from empty", "", "# T\n"),
            ("to empty", "# T\n", ""),
        ];
        let built: Vec<_> = cases
            .iter()
            .map(|&(name, base, changed)| (name, base, changed, agent_change(base, changed)))
            .collect();
        for (name, base, changed, (f, id)) in built {
            let written = f.undo(&id, changed.as_bytes(), false);

            let written = written.unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(f.disk(), base.as_bytes(), "{name}");
            assert_eq!(written.rev, Rev::of(base.as_bytes()), "{name}");
            assert_undo_edit(&f, &id, changed.as_bytes(), base.as_bytes());
        }
    }

    #[test]
    fn undo_writes_through_a_symlink_and_keeps_it() {
        let base = "# T\n\nold line\n";
        let changed = "# T\n\nnew line\n";
        let (f, id) = agent_change(base, changed);
        let link = f.dir.join("link.md");
        std::os::unix::fs::symlink(&f.doc, &link).unwrap();
        let doc = Roots::new(f.dir.clone(), None)
            .resolve_doc("link.md", Access::Write)
            .unwrap();
        assert_eq!(doc.canonical, f.doc, "setup");
        assert_eq!(doc.review_path, f.log.path(), "setup");

        reverse_change(&doc, &id, &Rev::of(changed.as_bytes()), false).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&link).unwrap(), base.as_bytes());
        assert_undo_edit(&f, &id, changed.as_bytes(), base.as_bytes());
    }

    #[test]
    fn undo_keeps_the_mode_bits() {
        let built: Vec<_> = [0o640, 0o600]
            .into_iter()
            .map(|mode| {
                let (f, id) = agent_change("# T\n\nold line\n", "# T\n\nnew line\n");
                fs::set_permissions(&f.doc, fs::Permissions::from_mode(mode)).unwrap();
                (mode, f, id)
            })
            .collect();
        for (mode, f, id) in built {
            f.undo(&id, b"# T\n\nnew line\n", false).unwrap();

            let got = fs::metadata(&f.doc).unwrap().permissions().mode() & 0o7777;
            assert_eq!(got, mode, "mode {mode:o}");
            assert_eq!(f.disk(), b"# T\n\nold line\n");
        }
    }

    #[test]
    fn a_read_only_doc_is_refused_and_left_alone() {
        let changed = "# T\n\nnew line\n";
        let (f, id) = agent_change("# T\n\nold line\n", changed);
        fs::set_permissions(&f.doc, fs::Permissions::from_mode(0o444)).unwrap();
        let ino = fs::metadata(&f.doc).unwrap().ino();

        let result = f.undo(&id, changed.as_bytes(), false);

        assert!(matches!(result, Err(UndoErr::ReadOnly)), "{result:?}");
        assert_eq!(f.disk(), changed.as_bytes());
        assert_eq!(fs::metadata(&f.doc).unwrap().ino(), ino);
        assert_eq!(f.count(is_edit), 0);
    }

    #[test]
    fn a_doc_changed_after_the_fold_is_changed_since() {
        let changed = "# T\n\nnew line\n";
        let (f, id) = agent_change("# T\n\nold line\n", changed);
        let outside = "# T\n\nnew line\n\nand a paragraph from an editor\n";
        fs::write(&f.doc, outside).unwrap();

        let result = f.undo(&id, changed.as_bytes(), false);

        assert!(matches!(result, Err(UndoErr::ChangedSince)), "{result:?}");
        assert_eq!(f.disk(), outside.as_bytes());
        assert_eq!(f.count(is_edit), 0);
    }

    #[test]
    fn a_same_length_edit_with_its_mtime_restored_is_changed_since() {
        let changed = "# T\n\nnew line\n\nend\n";
        let (f, id) = agent_change("# T\n\nold line\n\nend\n", changed);
        let before = fs::metadata(&f.doc).unwrap();
        // Truncates in place, so the inode stays too.
        let outside = "# T\n\nnew line\n\nEND\n";
        assert_eq!(outside.len(), changed.len(), "setup");
        fs::write(&f.doc, outside).unwrap();
        File::options()
            .write(true)
            .open(&f.doc)
            .unwrap()
            .set_modified(before.modified().unwrap())
            .unwrap();
        let after = fs::metadata(&f.doc).unwrap();
        assert_eq!(
            (after.len(), after.modified().unwrap(), after.ino()),
            (before.len(), before.modified().unwrap(), before.ino()),
            "setup"
        );

        let result = f.undo(&id, changed.as_bytes(), false);

        assert!(matches!(result, Err(UndoErr::ChangedSince)), "{result:?}");
        assert_eq!(f.disk(), outside.as_bytes());
        assert_eq!(f.count(is_edit), 0);
    }

    #[test]
    fn a_byte_change_between_the_read_and_the_rename_is_changed_since() {
        let changed = "# T\n\nnew line\n\nend\n";
        let (f, id) = agent_change("# T\n\nold line\n\nend\n", changed);
        let outside = "# T\n\nnew line\n\nEND\n";
        let entries = f.entries();

        let result = reverse_change_with(
            &f.docref(),
            &id,
            &Rev::of(changed.as_bytes()),
            false,
            |stage| {
                if stage == Stage::Read {
                    fs::write(&f.doc, outside).unwrap();
                }
            },
        );

        assert!(matches!(result, Err(UndoErr::ChangedSince)), "{result:?}");
        assert_eq!(f.disk(), outside.as_bytes());
        assert_eq!(f.count(is_edit), 0);
        assert_eq!(f.entries(), entries, "temp file left behind");
    }

    /// Panics unless the review file's lock is taken.
    fn assert_locked(review: &Path) {
        match File::open(review).unwrap().try_lock() {
            Err(fs::TryLockError::WouldBlock) => {}
            other => panic!("review file not locked while the doc is written: {other:?}"),
        }
    }

    #[test]
    fn a_watcher_sync_during_the_undo_records_nothing() {
        let base = "# T\n\nold line\n";
        let (f, id) = agent_change(base, "# T\n\nnew line\n");
        let externals = f.count(is_external);
        let (review, doc, now) = (f.log.path().to_path_buf(), f.doc.clone(), f.clock);
        let mut watcher: Option<JoinHandle<Option<u32>>> = None;

        let result = reverse_change_with(
            &f.docref(),
            &id,
            &Rev::of(b"# T\n\nnew line\n"),
            false,
            |stage| {
                if stage != Stage::Read {
                    return;
                }
                assert_locked(&review);
                // Blocks on the lock until the undo lets go of it.
                let (review, doc) = (review.clone(), doc.clone());
                watcher = Some(std::thread::spawn(move || {
                    let mut held = Log::open(&review).hold().unwrap();
                    sync_external(&mut held, &doc, now + 500)
                }));
            },
        );

        result.unwrap();
        let synced = watcher.expect("hook ran").join().unwrap();
        assert_eq!(synced, None);
        assert_eq!(f.count(is_external), externals);
        assert_eq!(f.disk(), base.as_bytes());
        assert_eq!(f.head(), Some(Rev::of(base.as_bytes())));
    }

    const FOUR: &str = "# Plan\n\none\n\ntwo\n\nthree\n\nfour\n";

    /// Three hunks over [`FOUR`]. The first adds a line, so the others land only if
    /// that shift is kept.
    const THREE: &str = "# Plan\n\nONE\nONE-b\n\ntwo\n\nTHREE\n\nFOUR\n";

    /// The agent's three-hunk write of [`THREE`] over [`FOUR`]; its change ids.
    fn multi_hunk(f: &mut Fixture) -> Vec<String> {
        let n = f.writes(THREE);
        let ids: Vec<String> = (0..3).map(|j| format!("{n}.{j}")).collect();
        for id in &ids {
            let c = f.change(id);
            assert_eq!((c.attr, c.op_hunks), (Attr::Agent, 3), "setup: {c:?}");
            assert!(c.reversible.ok, "setup: {c:?}");
            assert_eq!(c.reversible.reason, ReversibleReason::MultiHunk, "setup");
        }
        ids
    }

    #[test]
    fn a_multi_hunk_change_needs_confirm_and_reverts_its_whole_write() {
        let mut f = Fixture::claimed(FOUR);
        let ids = multi_hunk(&mut f);
        let entries = f.entries();

        let unconfirmed = f.undo(&ids[1], THREE.as_bytes(), false);
        assert!(
            matches!(unconfirmed, Err(UndoErr::NotReversible)),
            "{unconfirmed:?}"
        );
        assert_eq!(f.disk(), THREE.as_bytes());
        assert_eq!(f.count(is_edit), 0);

        let written = f.undo(&ids[1], THREE.as_bytes(), true).unwrap();

        assert_eq!(f.disk(), FOUR.as_bytes());
        assert_eq!(written.rev, Rev::of(FOUR.as_bytes()));
        assert_eq!(f.count(is_edit), 1, "one edit for the whole write");
        assert_undo_edit(&f, &ids[1], THREE.as_bytes(), FOUR.as_bytes());
        let edit = f.lines().last().unwrap().n;
        for id in &ids {
            let c = f.change(id);
            assert_eq!((c.undone_by, c.reversible.ok), (Some(edit), false), "{id}");
        }
        assert_eq!(f.entries(), entries, "temp file left behind");
    }

    #[test]
    fn a_whole_write_undo_follows_a_later_insert_above_it() {
        let mut f = Fixture::claimed(FOUR);
        let ids = multi_hunk(&mut f);
        f.yield_turn();
        let prefaced = format!("Preface.\n\n{THREE}");
        f.writes(&prefaced);
        for id in &ids {
            assert!(f.change(id).reversible.ok, "setup: {id}");
        }

        f.undo(&ids[0], prefaced.as_bytes(), true).unwrap();

        let restored = format!("Preface.\n\n{FOUR}");
        assert_eq!(f.disk(), restored.as_bytes());
        assert_undo_edit(&f, &ids[0], prefaced.as_bytes(), restored.as_bytes());
    }

    #[test]
    fn changes_that_are_not_reversible_are_refused() {
        const TWO: &str = "# Plan\n\none\n\nTWO\n\nthree\n\nfour\n";
        // Every fixture is built and checked before the first undo runs.
        let mut outside = Fixture::claimed(FOUR);
        outside.yield_turn();
        let outside_id = format!("{}.0", outside.writes(TWO));
        let c = outside.change(&outside_id);
        assert_eq!(c.reversible.reason, ReversibleReason::Outside, "setup");

        let mut over = Fixture::claimed(FOUR);
        let over_id = format!("{}.0", over.writes(TWO));
        over.yield_turn();
        over.writes("# Plan\n\none\n\nTwo, again\n\nthree\n\nfour\n");
        let c = over.change(&over_id);
        assert_eq!(c.reversible.reason, ReversibleReason::LaterOverlap, "setup");

        let mut sibling = Fixture::claimed(FOUR);
        let sibling_ids = multi_hunk(&mut sibling);
        sibling.yield_turn();
        sibling.writes("# Plan\n\nOne, again\n\ntwo\n\nTHREE\n\nFOUR\n");
        let c = sibling.change(&sibling_ids[0]);
        assert_eq!(c.reversible.reason, ReversibleReason::LaterOverlap, "setup");
        assert!(sibling.change(&sibling_ids[2]).reversible.ok, "setup");

        let (other, other_id) = agent_change(FOUR, TWO);
        let (undone, undone_id) = agent_change("# T\n\nold line\n", "# T\n\nnew line\n");

        let mut bad = Vec::new();
        let mut check = |name: &str, f: &Fixture, id: &str| {
            let (disk, edits) = (f.disk(), f.count(is_edit));
            let result = f.undo(id, &disk, true);
            if !matches!(result, Err(UndoErr::NotReversible))
                || f.disk() != disk
                || f.count(is_edit) != edits
            {
                bad.push(format!("{name}: {result:?}"));
            }
        };
        check("outside", &outside, &outside_id);
        check("written over later", &over, &over_id);
        check("a sibling written over later", &sibling, &sibling_ids[2]);
        check("unknown id", &other, "999.0");
        check("not an id", &other, "two");
        check(
            "hunk past the write's last",
            &other,
            &other_id.replace(".0", ".1"),
        );
        undone
            .undo(&undone_id, b"# T\n\nnew line\n", false)
            .unwrap();
        assert!(!undone.change(&undone_id).reversible.ok, "setup");
        check("already undone", &undone, &undone_id);
        let undo = undone.lines().last().unwrap().n;
        check("the undo itself", &undone, &format!("{undo}.0"));

        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn a_stale_client_rev_is_changed_since() {
        let base = "# T\n\nold line\n";
        let changed = "# T\n\nnew line\n";
        let (f, id) = agent_change(base, changed);

        // The page still shows the text before the agent's write.
        let result = f.undo(&id, base.as_bytes(), false);

        assert!(matches!(result, Err(UndoErr::ChangedSince)), "{result:?}");
        assert_eq!(f.disk(), changed.as_bytes());
        assert_eq!(f.count(is_edit), 0);
    }

    #[test]
    fn without_a_review_file_there_is_nothing_to_undo() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let path = dir.join("doc.md");
        fs::write(&path, "# T\n\nline\n").unwrap();
        let doc = DocRef {
            rel: "doc.md".to_string(),
            canonical: path.clone(),
            review_path: review_path_for(&path),
            writable: true,
            log_writable: true,
        };

        let result = reverse_change(&doc, "3.0", &Rev::of(b"# T\n\nline\n"), true);

        assert!(matches!(result, Err(UndoErr::NotReversible)), "{result:?}");
        assert_eq!(fs::read(&path).unwrap(), b"# T\n\nline\n");
        assert!(!doc.review_path.exists(), "undo created a review file");
    }

    #[test]
    fn a_write_the_page_saw_but_the_log_missed_is_recorded_before_the_undo() {
        let mut f = Fixture::claimed(FOUR);
        let id = format!("{}.0", f.writes("# Plan\n\none\n\nTWO\n\nthree\n\nfour\n"));
        // Pushed to the page before the watcher recorded it.
        let outside = "# Plan\n\none\n\nTWO\n\nthree\n\nfour, edited\n";
        fs::write(&f.doc, outside).unwrap();

        f.undo(&id, outside.as_bytes(), false).unwrap();

        let restored = "# Plan\n\none\n\ntwo\n\nthree\n\nfour, edited\n";
        assert_eq!(f.disk(), restored.as_bytes());
        let lines = f.lines();
        let [.., external, _] = &lines[..] else {
            panic!("{lines:#?}");
        };
        assert!(
            matches!(&external.op, Op::External { to_rev, .. } if *to_rev == Rev::of(outside.as_bytes())),
            "{external:?}"
        );
        assert_undo_edit(&f, &id, outside.as_bytes(), restored.as_bytes());
    }

    #[test]
    fn a_claimers_write_stays_until_someone_presses_x_on_it() {
        let base = "# Plan\n\nold intro\n\n- [ ] ship it\n\nold outro\n";
        let mut f = Fixture::claimed(base);
        // Made by the human's editor, but inside the claim, so labelled as the agent's.
        let intro = f.writes("# Plan\n\nnew intro\n\n- [ ] ship it\n\nold outro\n");
        let both = "# Plan\n\nnew intro\n\n- [ ] ship it\n\nnew outro\n";
        let outro = f.writes(both);
        let intro_id = format!("{intro}.0");
        assert_eq!(f.change(&intro_id).attr, Attr::Agent, "setup");
        assert!(f.change(&format!("{outro}.0")).reversible.ok, "setup");
        let recorded = f.log_bytes();

        f.change(&intro_id);
        assert_eq!(f.sync(), None);
        let target = WriteTarget {
            canonical: f.doc.clone(),
            rel: "doc.md".to_string(),
        };
        doc_write::write_doc(
            &target,
            Some(&f.log),
            Expect::Fresh,
            Via::Checkbox { line: 5 },
            |b| doc_write::toggle_task_line(b, 5, true).ok_or(WriteErr::NotApplicable),
        )
        .unwrap();
        let ticked = "# Plan\n\nnew intro\n\n- [x] ship it\n\nnew outro\n";
        assert_eq!(f.disk(), ticked.as_bytes());
        f.undo(&format!("{outro}.0"), ticked.as_bytes(), false)
            .unwrap();

        assert_eq!(
            f.disk(),
            b"# Plan\n\nnew intro\n\n- [x] ship it\n\nold outro\n"
        );
        assert!(
            f.log_bytes().starts_with(&recorded),
            "the log was rewritten"
        );
        let c = f.change(&intro_id);
        assert_eq!((c.attr, c.undone_by), (Attr::Agent, None));
    }

    #[test]
    fn a_deletion_undone_once_is_not_undone_again() {
        let base = "# T\n\nkeep\n\ngone\n\nend\n";
        let changed = "# T\n\nkeep\n\nend\n";
        let (f, id) = agent_change(base, changed);
        f.undo(&id, changed.as_bytes(), false).unwrap();
        assert_eq!(f.disk(), base.as_bytes());
        assert_undo_edit(&f, &id, changed.as_bytes(), base.as_bytes());

        // The re-inserted lines only shift the gap, so only `undone_by` can refuse it.
        let again = f.undo(&id, base.as_bytes(), false);

        assert!(matches!(again, Err(UndoErr::NotReversible)), "{again:?}");
        assert_eq!(f.disk(), base.as_bytes());
        assert_eq!(f.count(is_edit), 1);
        let c = f.change(&id);
        assert_eq!(
            (c.reversible.ok, c.reversible.reason),
            (false, ReversibleReason::LaterOverlap)
        );
    }

    #[test]
    fn a_change_moved_by_a_later_write_is_undone_where_it_now_is() {
        let (mut f, id) = agent_change(FOUR, "# Plan\n\none\n\nTWO\n\nthree\n\nfour\n");
        f.yield_turn();
        let moved = "# Plan\n\nintro\n\nmore\n\none\n\nTWO\n\nthree\n\nfour\n";
        f.writes(moved);
        let c = f.change(&id);
        assert!(c.reversible.ok && c.range.lines != c.new, "setup: {c:?}");

        f.undo(&id, moved.as_bytes(), false).unwrap();

        let restored = "# Plan\n\nintro\n\nmore\n\none\n\ntwo\n\nthree\n\nfour\n";
        assert_eq!(f.disk(), restored.as_bytes());
        assert_undo_edit(&f, &id, moved.as_bytes(), restored.as_bytes());
    }

    #[test]
    fn a_doc_made_read_only_after_it_was_resolved_is_refused() {
        let changed = "# T\n\nnew line\n";
        let (f, id) = agent_change("# T\n\nold line\n", changed);
        let doc = f.docref();
        assert!(doc.writable, "setup");
        fs::set_permissions(&f.doc, fs::Permissions::from_mode(0o444)).unwrap();
        let ino = fs::metadata(&f.doc).unwrap().ino();

        let result = reverse_change(&doc, &id, &Rev::of(changed.as_bytes()), false);

        assert!(matches!(result, Err(UndoErr::ReadOnly)), "{result:?}");
        assert_eq!(f.disk(), changed.as_bytes());
        assert_eq!(fs::metadata(&f.doc).unwrap().ino(), ino);
        assert_eq!(f.count(is_edit), 0);
    }

    #[test]
    fn undo_replaces_the_doc_by_rename() {
        let changed = "# T\n\nnew line\n";
        let (f, id) = agent_change("# T\n\nold line\n", changed);
        let (ino, entries) = (fs::metadata(&f.doc).unwrap().ino(), f.entries());

        f.undo(&id, changed.as_bytes(), false).unwrap();

        assert_ne!(fs::metadata(&f.doc).unwrap().ino(), ino, "written in place");
        assert_eq!(f.entries(), entries, "temp file left behind");
    }

    #[test]
    fn a_review_file_that_cannot_take_the_edit_leaves_the_doc_alone() {
        let (base, changed) = ("# T\n\nold line\n", "# T\n\nnew line\n");
        let (locked, locked_id) = agent_change(base, changed);
        fs::set_permissions(locked.log.path(), fs::Permissions::from_mode(0o444)).unwrap();
        let (newer, newer_id) = agent_change(base, changed);
        let text = String::from_utf8(newer.log_bytes()).unwrap();
        fs::write(newer.log.path(), text.replacen("review/1", "review/2", 1)).unwrap();
        assert!(newer.log.read().unwrap().unwrap().read_only, "setup");

        let mut bad = Vec::new();
        for (name, f, id) in [
            ("0444", &locked, &locked_id),
            ("review/2", &newer, &newer_id),
        ] {
            let log = f.log_bytes();
            let result = f.undo(id, changed.as_bytes(), false);
            if !matches!(result, Err(UndoErr::ReadOnly))
                || f.disk() != changed.as_bytes()
                || f.log_bytes() != log
            {
                bad.push(format!("{name}: {result:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn the_lock_spans_the_read_the_rename_and_the_append() {
        let (base, changed) = ("# T\n\nold line\n", "# T\n\nnew line\n");
        let (f, id) = agent_change(base, changed);
        let review = f.log.path().to_path_buf();
        let mut stages = Vec::new();

        reverse_change_with(
            &f.docref(),
            &id,
            &Rev::of(changed.as_bytes()),
            false,
            |stage| {
                assert_locked(&review);
                if stage == Stage::Renamed {
                    assert_eq!(f.disk(), base.as_bytes(), "not after the rename");
                    assert_eq!(
                        f.count(is_edit),
                        0,
                        "edit appended before the doc was written"
                    );
                }
                stages.push(stage);
            },
        )
        .unwrap();

        assert_eq!(stages, [Stage::Read, Stage::Renamed]);
        assert_undo_edit(&f, &id, changed.as_bytes(), base.as_bytes());
    }

    #[test]
    fn a_reverse_lands_only_on_its_exact_lines() {
        let text = b"\xef\xbb\xbf# T\r\n\r\nnew line\r\nend";
        let rev = ops::Hunk {
            a: 2,
            d: vec!["new line\r\n".to_string()],
            i: vec!["old line\r\n".to_string()],
        };
        let want = b"\xef\xbb\xbf# T\r\n\r\nold line\r\nend";
        let one = |h: &ops::Hunk| apply_reverse(text, std::slice::from_ref(h));
        assert_eq!(one(&rev).as_deref(), Some(&want[..]));
        let line_one = ops::Hunk {
            a: 0,
            d: vec!["\u{feff}# T\r\n".to_string()],
            i: vec!["\u{feff}# Title\r\n".to_string()],
        };
        assert!(one(&line_one).is_some(), "bom on line 1");

        let refused = [
            (
                "a line early",
                ops::Hunk {
                    a: 1,
                    ..rev.clone()
                },
            ),
            (
                "a line late",
                ops::Hunk {
                    a: 3,
                    ..rev.clone()
                },
            ),
            (
                "past the end",
                ops::Hunk {
                    a: 9,
                    ..rev.clone()
                },
            ),
            (
                "other terminator",
                ops::Hunk {
                    d: vec!["new line\n".to_string()],
                    ..rev.clone()
                },
            ),
            (
                "a prefix",
                ops::Hunk {
                    d: vec!["new".to_string()],
                    ..rev.clone()
                },
            ),
            (
                "runs past the end",
                ops::Hunk {
                    d: vec!["new line\r\n".into(), "end".into(), "x".into()],
                    ..rev.clone()
                },
            ),
            (
                "bom left out",
                ops::Hunk {
                    d: vec!["# T\r\n".to_string()],
                    ..line_one.clone()
                },
            ),
        ];
        let bad: Vec<_> = refused
            .into_iter()
            .filter(|(_, h)| one(h).is_some())
            .map(|(name, _)| name)
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
    }
}
