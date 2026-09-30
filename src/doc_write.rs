//! The one strict write path for document bytes.

use std::io;
use std::path::{Path, PathBuf};

use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document};

use crate::rev::Rev;
use crate::{anchor, atomic_write, paths};

/// A document already resolved and authorised by `Roots::resolve_doc(.., Access::Write)`.
pub(crate) struct WriteTarget {
    pub(crate) canonical: PathBuf,
    #[expect(dead_code, reason = "for edit recorders, which arrive with reviews")]
    pub(crate) rel: String,
}

/// What the caller believes is on disk.
pub(crate) enum Expect {
    Rev(Rev),
    /// Whatever is on disk, after recording any write the review file missed.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    Fresh,
}

/// Why the document is being written.
#[derive(Clone, Debug)]
pub(crate) enum Via {
    Checkbox {
        #[expect(dead_code, reason = "the recorded patch already names the line")]
        line: u32,
    },
    /// `x` on change `of`, reverting the write it belongs to.
    Undo { of: String },
}

#[derive(Debug)]
pub(crate) enum WriteErr {
    /// Disk bytes differ from the expected rev, or changed while the edit ran.
    ChangedSince,
    /// The edit no longer applies to the current bytes.
    NotApplicable,
    /// The doc, or the review file that must record the write, can't be written.
    ReadOnly,
    Io(io::Error),
}

#[derive(Debug)]
pub(crate) struct Written {
    pub(crate) rev: Rev,
}

/// Where a write records itself, such as a review file.
pub(crate) trait EditRecorder {
    /// Takes the recorder's lock for the whole write. `None` when there is nothing to
    /// record into; `ReadOnly` when it couldn't take the record, so the doc isn't written.
    fn begin(&self) -> Result<Option<Box<dyn Recording + '_>>, WriteErr>;
}

/// A recorder under its lock.
pub(crate) trait Recording {
    /// Records `bytes`, read under the lock, unless they are what it already has.
    fn sync(&mut self, doc: &Path, bytes: &[u8]);
    /// Records the write that turned `from` into `to`.
    fn record(&mut self, via: &Via, from: &[u8], to: &[u8]) -> io::Result<()>;
}

/// Points in a write a test can stop at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The doc was read under the lock.
    Read,
    /// The doc was replaced, and the record not yet written.
    Renamed,
}

/// Reads the target, checks `expect`, applies `edit`, and replaces the file atomically.
/// Output identical to the input writes nothing. With a recorder, the whole write runs
/// under its lock, and the record lands before the lock is let go.
pub(crate) fn write_doc(
    t: &WriteTarget,
    rec: Option<&dyn EditRecorder>,
    expect: Expect,
    via: Via,
    edit: impl FnOnce(&[u8]) -> Result<Vec<u8>, WriteErr>,
) -> Result<Written, WriteErr> {
    let mut held = match rec {
        Some(rec) => rec.begin()?,
        None => None,
    };
    let before = std::fs::read(&t.canonical).map_err(WriteErr::Io)?;
    let from = Rev::of(&before);
    if let Expect::Rev(expected) = expect
        && expected != from
    {
        return Err(WriteErr::ChangedSince);
    }
    if let Some(held) = held.as_mut() {
        held.sync(&t.canonical, &before);
    }
    let after = edit(&before)?;
    if after == before {
        return Ok(Written { rev: from });
    }
    replace(t, held.as_deref_mut(), &via, &before, &after, &mut |_| {})
}

/// Replaces `before`, read under `rec`'s lock, with `after`, then records the write.
pub(crate) fn replace(
    t: &WriteTarget,
    rec: Option<&mut (dyn Recording + '_)>,
    via: &Via,
    before: &[u8],
    after: &[u8],
    stage: &mut dyn FnMut(Stage),
) -> Result<Written, WriteErr> {
    // A rename would replace a read-only file, so check before writing.
    if !paths::is_writable(&t.canonical) {
        return Err(WriteErr::ReadOnly);
    }
    // Narrows the window for an editor that saves without taking any lock.
    let unchanged = || std::fs::read(&t.canonical).is_ok_and(|now| now == before);
    if !atomic_write::write_atomic_if(&t.canonical, after, unchanged).map_err(WriteErr::Io)? {
        return Err(WriteErr::ChangedSince);
    }
    stage(Stage::Renamed);
    if let Some(rec) = rec {
        rec.record(via, before, after).map_err(WriteErr::Io)?;
    }
    Ok(Written {
        rev: Rev::of(after),
    })
}

/// Flips the task marker of 1-based `line`, keeping every other byte (CRLF, BOM, missing
/// final newline). `None` unless comrak parses a task item starting on that line that is
/// currently in the opposite state.
pub(crate) fn toggle_task_line(bytes: &[u8], line: usize, checked: bool) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let arena = Arena::new();
    let root = parse_document(&arena, text, &anchor::parse_options());
    let task = root.descendants().find_map(|node| {
        let ast = node.data.borrow();
        match ast.value {
            NodeValue::TaskItem(task) if ast.sourcepos.start.line == line => Some(task),
            _ => None,
        }
    })?;
    if task.symbol.is_some() == checked {
        return None;
    }
    let symbol = &task.symbol_sourcepos.start;
    let at = line_start(bytes, symbol.line)? + symbol.column.checked_sub(1)?;
    // Only the one symbol byte between the brackets changes.
    if bytes.get(at.checked_sub(1)?) != Some(&b'[') || bytes.get(at + 1) != Some(&b']') {
        return None;
    }
    let mut out = bytes.to_vec();
    out[at] = if checked { b'x' } else { b' ' };
    Some(out)
}

/// Byte offset where 1-based `line` starts, counting `\n`, `\r\n` and a lone `\r` as
/// line ends the way CommonMark does.
fn line_start(bytes: &[u8], line: usize) -> Option<usize> {
    let mut current = 1;
    let mut i = 0;
    while current < line {
        match bytes.get(i)? {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                i += 2;
                current += 1;
            }
            b'\r' | b'\n' => {
                i += 1;
                current += 1;
            }
            _ => i += 1,
        }
    }
    Some(i)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs::File;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::review::chain::{CHECKPOINT_EVERY, Chain, REV_ONLY_OVER};
    use crate::review::hunk;
    use crate::review::log::Log;
    use crate::review::ops::{Act, ByKind, Line, Op};
    use crate::review::review_path_for;
    use crate::review::sync::sync_external;

    fn toggle(text: &str, line: usize, checked: bool) -> Option<String> {
        toggle_task_line(text.as_bytes(), line, checked).map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn toggles_task_marker() {
        let cases = [
            ("- [ ] a\n", 1, true, "- [x] a\n"),
            ("- [x] a\n", 1, false, "- [ ] a\n"),
            ("- [X] a\n", 1, false, "- [ ] a\n"),
            ("* [ ] a\n", 1, true, "* [x] a\n"),
            ("+ [ ] a\n", 1, true, "+ [x] a\n"),
            ("1. [ ] a\n", 1, true, "1. [x] a\n"),
            ("12) [ ] a\n", 1, true, "12) [x] a\n"),
            ("- a\n  - [ ] b\n", 2, true, "- a\n  - [x] b\n"),
            ("> - [ ] a\n", 1, true, "> - [x] a\n"),
            ("> > 1. [ ] a\n", 1, true, "> > 1. [x] a\n"),
            ("- a\n\t- [ ] b\n", 2, true, "- a\n\t- [x] b\n"),
            ("- [ ]\ta\n", 1, true, "- [x]\ta\n"),
            (
                "- [ ] a\n- [ ] b\n- [ ] c\n",
                2,
                true,
                "- [ ] a\n- [x] b\n- [ ] c\n",
            ),
        ];
        let mut bad = Vec::new();
        for (text, line, checked, want) in cases {
            let got = toggle(text, line, checked);
            if got.as_deref() != Some(want) {
                bad.push(format!("{text:?} line {line} -> {checked}: got {got:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn keeps_line_endings_bom_and_missing_final_newline() {
        let cases: [(&[u8], usize, &[u8]); 5] = [
            (b"- [ ] a\r\n- [ ] b\r\n", 2, b"- [ ] a\r\n- [x] b\r\n"),
            (b"- [ ] a\r\n- [ ] b", 2, b"- [ ] a\r\n- [x] b"),
            (b"\xef\xbb\xbf- [ ] a\n", 1, b"\xef\xbb\xbf- [x] a\n"),
            (b"\xef\xbb\xbf- [ ] a\r\n", 1, b"\xef\xbb\xbf- [x] a\r\n"),
            (b"- [ ] a", 1, b"- [x] a"),
        ];
        for (bytes, line, want) in cases {
            assert_eq!(
                toggle_task_line(bytes, line, true).as_deref(),
                Some(want),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn edits_only_the_marker_not_brackets_in_text() {
        assert_eq!(
            toggle("- [ ] use [ ] and [x]\n", 1, true).as_deref(),
            Some("- [x] use [ ] and [x]\n")
        );
        assert_eq!(
            toggle("- [x] use [ ] and [x]\n", 1, false).as_deref(),
            Some("- [ ] use [ ] and [x]\n")
        );
    }

    #[test]
    fn refuses_lines_that_are_not_tasks_in_the_expected_state() {
        let cases = [
            ("- [x] a\n", 1, true),
            ("- [ ] a\n", 1, false),
            ("- [x] a [ ] b\n", 1, true),
            ("- a [ ] b\n", 1, true),
            ("text [ ] here\n", 1, true),
            ("-[ ] a\n", 1, true),
            ("- [ ]a\n", 1, true),
            ("- [ ] a\n", 0, true),
            ("- [ ] a\n", 2, true),
            ("- [ ] a\n", usize::MAX, true),
            ("", 1, true),
            ("```\n- [ ] a\n```\n", 2, true),
            ("~~~md\n- [ ] a\n~~~\n", 2, true),
            ("    - [ ] a\n", 1, true),
            ("<div>\n- [ ] a\n</div>\n", 2, true),
            ("- [ ] a\n  b [ ]\n", 2, true),
        ];
        let mut bad = Vec::new();
        for (text, line, checked) in cases {
            if let Some(got) = toggle(text, line, checked) {
                bad.push(format!("{text:?} line {line} -> {checked}: got {got:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    fn doc(dir: &std::path::Path, bytes: &[u8]) -> WriteTarget {
        let canonical = dir.join("doc.md");
        std::fs::write(&canonical, bytes).unwrap();
        WriteTarget {
            canonical: canonical.canonicalize().unwrap(),
            rel: "doc.md".to_string(),
        }
    }

    const CHECKBOX: Via = Via::Checkbox { line: 1 };

    fn tick(bytes: &[u8]) -> Result<Vec<u8>, WriteErr> {
        toggle_task_line(bytes, 1, true).ok_or(WriteErr::NotApplicable)
    }

    #[test]
    fn writes_when_rev_matches() {
        let dir = tempfile::tempdir().unwrap();
        let t = doc(dir.path(), b"- [ ] a\n");

        let written =
            write_doc(&t, None, Expect::Rev(Rev::of(b"- [ ] a\n")), CHECKBOX, tick).unwrap();

        assert_eq!(std::fs::read(&t.canonical).unwrap(), b"- [x] a\n");
        assert_eq!(written.rev, Rev::of(b"- [x] a\n"));
    }

    #[test]
    fn stale_rev_is_changed_since_and_edit_never_runs() {
        let dir = tempfile::tempdir().unwrap();
        let t = doc(dir.path(), b"- [ ] a\n");

        let result = write_doc(
            &t,
            None,
            Expect::Rev(Rev::of(b"- [ ] older\n")),
            CHECKBOX,
            |_| panic!("edit ran against a stale rev"),
        );

        assert!(matches!(result, Err(WriteErr::ChangedSince)), "{result:?}");
        assert_eq!(std::fs::read(&t.canonical).unwrap(), b"- [ ] a\n");
    }

    #[test]
    fn not_applicable_edit_leaves_file() {
        let dir = tempfile::tempdir().unwrap();
        let t = doc(dir.path(), b"plain\n");
        let before = std::fs::metadata(&t.canonical).unwrap().ino();

        let result = write_doc(&t, None, Expect::Rev(Rev::of(b"plain\n")), CHECKBOX, tick);

        assert!(matches!(result, Err(WriteErr::NotApplicable)), "{result:?}");
        assert_eq!(std::fs::read(&t.canonical).unwrap(), b"plain\n");
        assert_eq!(std::fs::metadata(&t.canonical).unwrap().ino(), before);
    }

    #[test]
    fn identical_output_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let t = doc(dir.path(), b"same\n");
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&t.canonical)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let before = std::fs::metadata(&t.canonical).unwrap().ino();

        let written = write_doc(&t, None, Expect::Rev(Rev::of(b"same\n")), CHECKBOX, |b| {
            Ok(b.to_vec())
        })
        .unwrap();

        assert_eq!(written.rev, Rev::of(b"same\n"));
        let meta = std::fs::metadata(&t.canonical).unwrap();
        assert_eq!(meta.ino(), before);
        assert_eq!(meta.modified().unwrap(), old, "rewritten in place");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn change_during_edit_is_changed_since_and_keeps_the_outside_write() {
        // The second outside write has the same length as the original.
        for outside in [&b"- [ ] a\n- [ ] from editor\n"[..], b"- [ ] b\n"] {
            let dir = tempfile::tempdir().unwrap();
            let t = doc(dir.path(), b"- [ ] a\n");
            let path = t.canonical.clone();

            let result = write_doc(
                &t,
                None,
                Expect::Rev(Rev::of(b"- [ ] a\n")),
                CHECKBOX,
                |b| {
                    // An editor that doesn't take the lock saves while the edit runs.
                    std::fs::write(&path, outside).unwrap();
                    tick(b)
                },
            );

            assert!(matches!(result, Err(WriteErr::ChangedSince)), "{result:?}");
            assert_eq!(std::fs::read(&t.canonical).unwrap(), outside);
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                1,
                "temp file left behind"
            );
        }
    }

    #[test]
    fn read_only_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let t = doc(dir.path(), b"- [ ] a\n");
        std::fs::set_permissions(&t.canonical, std::fs::Permissions::from_mode(0o444)).unwrap();
        let before = std::fs::metadata(&t.canonical).unwrap().ino();

        let result = write_doc(&t, None, Expect::Rev(Rev::of(b"- [ ] a\n")), CHECKBOX, tick);

        assert!(matches!(result, Err(WriteErr::ReadOnly)), "{result:?}");
        assert_eq!(std::fs::read(&t.canonical).unwrap(), b"- [ ] a\n");
        assert_eq!(std::fs::metadata(&t.canonical).unwrap().ino(), before);
    }

    #[derive(Default)]
    struct Spy(RefCell<Vec<(Rev, Rev)>>);

    struct SpyHeld<'a>(&'a Spy);

    impl EditRecorder for Spy {
        fn begin(&self) -> Result<Option<Box<dyn Recording + '_>>, WriteErr> {
            Ok(Some(Box::new(SpyHeld(self))))
        }
    }

    impl Recording for SpyHeld<'_> {
        fn sync(&mut self, _: &Path, _: &[u8]) {}

        fn record(&mut self, _: &Via, from: &[u8], to: &[u8]) -> io::Result<()> {
            self.0.0.borrow_mut().push((Rev::of(from), Rev::of(to)));
            Ok(())
        }
    }

    #[test]
    fn recorder_sees_only_real_writes() {
        let dir = tempfile::tempdir().unwrap();
        let spy = Spy::default();
        let old = b"- [ ] a\n";

        let t = doc(dir.path(), old);
        write_doc(&t, Some(&spy), Expect::Rev(Rev::of(old)), CHECKBOX, tick).unwrap();
        assert_eq!(*spy.0.borrow(), [(Rev::of(old), Rev::of(b"- [x] a\n"))]);
        spy.0.borrow_mut().clear();

        let t = doc(dir.path(), old);
        let path = t.canonical.clone();
        let stale = write_doc(&t, Some(&spy), Expect::Rev(Rev::of(b"x")), CHECKBOX, tick);
        let not_applicable = write_doc(&t, Some(&spy), Expect::Rev(Rev::of(old)), CHECKBOX, |_| {
            Err(WriteErr::NotApplicable)
        });
        let identical = write_doc(&t, Some(&spy), Expect::Rev(Rev::of(old)), CHECKBOX, |b| {
            Ok(b.to_vec())
        });
        let race = write_doc(&t, Some(&spy), Expect::Rev(Rev::of(old)), CHECKBOX, |b| {
            std::fs::write(&path, b"- [ ] b\n").unwrap();
            tick(b)
        });

        assert!(stale.is_err() && not_applicable.is_err() && identical.is_ok() && race.is_err());
        assert!(spy.0.borrow().is_empty(), "{:?}", spy.0.borrow());
    }

    #[test]
    fn writes_through_to_symlink_target_and_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let t = doc(dir.path(), b"- [ ] a\n");
        let link = dir.path().join("link.md");
        std::os::unix::fs::symlink(&t.canonical, &link).unwrap();

        write_doc(&t, None, Expect::Rev(Rev::of(b"- [ ] a\n")), CHECKBOX, tick).unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&link).unwrap(), b"- [x] a\n");
    }

    const TASKS: &[u8] = b"- [ ] a\n- [ ] b\n";

    /// A doc and a review file created on its bytes.
    fn logged(dir: &Path, bytes: &[u8]) -> (WriteTarget, Log) {
        let t = doc(dir, bytes);
        let log = Log::open(&review_path_for(&t.canonical));
        let text = std::str::from_utf8(bytes).unwrap();
        assert!(
            log.create_if_missing("doc.md", text, None).unwrap(),
            "setup"
        );
        (t, log)
    }

    fn lines(log: &Log) -> Vec<Line> {
        let parsed = log.read().unwrap().expect("setup: a review file");
        assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
        parsed.lines
    }

    fn edits(log: &Log) -> Vec<Line> {
        lines(log)
            .into_iter()
            .filter(|l| matches!(l.op, Op::Edit { .. }))
            .collect()
    }

    fn head(log: &Log) -> Option<Rev> {
        Chain::build(&log.read().unwrap().unwrap()).head().cloned()
    }

    fn tick_line(line: usize) -> impl FnOnce(&[u8]) -> Result<Vec<u8>, WriteErr> {
        move |b| toggle_task_line(b, line, true).ok_or(WriteErr::NotApplicable)
    }

    /// Panics unless another process or fd holding the review file's lock would block.
    fn assert_locked(review: &Path) {
        let file = File::open(review).unwrap();
        match file.try_lock() {
            Err(std::fs::TryLockError::WouldBlock) => {}
            other => panic!("review file not locked while the doc is written: {other:?}"),
        }
    }

    /// The `edit` fields, checked against the texts they claim to join.
    fn assert_edit(line: &Line, act: Act, old: &[u8], new: &[u8]) {
        let Op::Edit {
            rev,
            to_rev,
            patch,
            of,
            act: got,
        } = &line.op
        else {
            panic!("not an edit: {line:?}");
        };
        assert_eq!(*got, act);
        assert_eq!(*of, None);
        assert_eq!(*rev, Rev::of(old));
        assert_eq!(*to_rev, Rev::of(new));
        let old = std::str::from_utf8(old).unwrap();
        let new = std::str::from_utf8(new).unwrap();
        assert_eq!(hunk::apply(old, patch.as_ref().unwrap()).unwrap(), new);
        assert_eq!(line.by.kind, ByKind::Human);
    }

    #[test]
    fn with_a_log_a_checkbox_appends_one_edit_under_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let (t, log) = logged(dir.path(), TASKS);
        let before = lines(&log).len();
        let review = log.path().to_path_buf();

        let written = write_doc(
            &t,
            Some(&log),
            Expect::Rev(Rev::of(TASKS)),
            Via::Checkbox { line: 2 },
            |b| {
                assert_locked(&review);
                tick_line(2)(b)
            },
        )
        .unwrap();

        let after = b"- [ ] a\n- [x] b\n";
        assert_eq!(std::fs::read(&t.canonical).unwrap(), after);
        assert_eq!(written.rev, Rev::of(after));
        let all = lines(&log);
        assert_eq!(all.len(), before + 1, "{all:#?}");
        assert_edit(all.last().unwrap(), Act::Checkbox, TASKS, after);
        assert_eq!(head(&log), Some(Rev::of(after)));
        let mut held = log.hold().unwrap();
        assert_eq!(sync_external(&mut held, &t.canonical, 0), None);
    }

    #[test]
    fn without_a_log_writes_and_creates_no_log() {
        let dir = tempfile::tempdir().unwrap();
        let t = doc(dir.path(), TASKS);
        let log = Log::open(&review_path_for(&t.canonical));

        write_doc(&t, Some(&log), Expect::Rev(Rev::of(TASKS)), CHECKBOX, tick).unwrap();

        assert_eq!(std::fs::read(&t.canonical).unwrap(), b"- [x] a\n- [ ] b\n");
        assert!(!log.path().exists(), "a checkbox created the review file");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn fresh_records_an_outside_write_before_the_edit() {
        let dir = tempfile::tempdir().unwrap();
        let (t, log) = logged(dir.path(), TASKS);
        let outside = b"- [ ] a\n- [ ] b\n- [ ] c from an editor\n";
        std::fs::write(&t.canonical, outside).unwrap();
        let mut seen = None;

        write_doc(
            &t,
            Some(&log),
            Expect::Fresh,
            Via::Checkbox { line: 3 },
            |b| {
                seen = Some(b.to_vec());
                tick_line(3)(b)
            },
        )
        .unwrap();

        let after = b"- [ ] a\n- [ ] b\n- [x] c from an editor\n";
        assert_eq!(seen.as_deref(), Some(&outside[..]));
        assert_eq!(std::fs::read(&t.canonical).unwrap(), after);
        let all = lines(&log);
        let [.., external, edit] = &all[..] else {
            panic!("{all:#?}");
        };
        assert!(
            matches!(&external.op, Op::External { rev, to_rev, .. }
                if *rev == Rev::of(TASKS) && *to_rev == Rev::of(outside)),
            "{external:?}"
        );
        assert_edit(edit, Act::Checkbox, outside, after);
        assert_eq!(head(&log), Some(Rev::of(after)));
    }

    #[test]
    fn a_rev_the_client_saw_but_the_log_missed_is_recorded_first() {
        let dir = tempfile::tempdir().unwrap();
        let (t, log) = logged(dir.path(), TASKS);
        // Pushed to the page before the watcher recorded it.
        let outside = b"- [ ] a\n- [ ] b\n- [ ] c\n";
        std::fs::write(&t.canonical, outside).unwrap();

        write_doc(
            &t,
            Some(&log),
            Expect::Rev(Rev::of(outside)),
            Via::Checkbox { line: 3 },
            tick_line(3),
        )
        .unwrap();

        let after = b"- [ ] a\n- [ ] b\n- [x] c\n";
        let all = lines(&log);
        let [.., external, edit] = &all[..] else {
            panic!("{all:#?}");
        };
        assert!(
            matches!(&external.op, Op::External { to_rev, .. } if *to_rev == Rev::of(outside)),
            "{external:?}"
        );
        assert_edit(edit, Act::Checkbox, outside, after);
        assert_eq!(head(&log), Some(Rev::of(after)));
    }

    #[test]
    fn with_a_log_a_refused_write_records_no_edit() {
        let dir = tempfile::tempdir().unwrap();
        let expect = || Expect::Rev(Rev::of(TASKS));
        let mut bad = Vec::new();
        let mut check = |name: &str, log: &Log, result: Result<Written, WriteErr>, ok: bool| {
            if result.is_ok() != ok || !edits(log).is_empty() {
                bad.push(format!("{name}: {result:?}, edits {:?}", edits(log)));
            }
        };

        let sub = |name: &str| {
            let d = dir.path().join(name);
            std::fs::create_dir(&d).unwrap();
            d
        };

        let d = sub("stale");
        let (t, log) = logged(&d, TASKS);
        let r = write_doc(&t, Some(&log), Expect::Rev(Rev::of(b"x")), CHECKBOX, tick);
        check("stale", &log, r, false);

        let d = sub("not-applicable");
        let (t, log) = logged(&d, TASKS);
        let r = write_doc(&t, Some(&log), expect(), CHECKBOX, |_| {
            Err(WriteErr::NotApplicable)
        });
        check("not applicable", &log, r, false);

        let d = sub("identical");
        let (t, log) = logged(&d, TASKS);
        let r = write_doc(&t, Some(&log), expect(), CHECKBOX, |b| Ok(b.to_vec()));
        check("identical", &log, r, true);

        let d = sub("race");
        let (t, log) = logged(&d, TASKS);
        let path = t.canonical.clone();
        let r = write_doc(&t, Some(&log), expect(), CHECKBOX, |b| {
            std::fs::write(&path, b"- [ ] a\n- [ ] B\n").unwrap();
            tick(b)
        });
        check("race", &log, r, false);
        assert_eq!(std::fs::read(&path).unwrap(), b"- [ ] a\n- [ ] B\n");

        let d = sub("read-only");
        let (t, log) = logged(&d, TASKS);
        std::fs::set_permissions(&t.canonical, std::fs::Permissions::from_mode(0o444)).unwrap();
        let r = write_doc(&t, Some(&log), expect(), CHECKBOX, tick);
        check("read only", &log, r, false);

        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn a_review_file_that_cannot_take_the_edit_refuses_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut bad = Vec::new();
        for name in ["read-only", "newer", "garbled"] {
            let d = dir.path().join(name);
            std::fs::create_dir(&d).unwrap();
            let (t, log) = logged(&d, TASKS);
            match name {
                "read-only" => {
                    std::fs::set_permissions(log.path(), std::fs::Permissions::from_mode(0o444))
                        .unwrap();
                }
                "newer" => {
                    let text = std::fs::read_to_string(log.path()).unwrap();
                    std::fs::write(log.path(), text.replacen("review/1", "review/2", 1)).unwrap();
                }
                _ => {
                    let text = std::fs::read_to_string(log.path()).unwrap();
                    std::fs::write(log.path(), text.replacen("{", "{{", 1)).unwrap();
                }
            }
            if name != "read-only" {
                assert!(log.read().unwrap().unwrap().read_only, "setup: {name}");
            }
            let review = std::fs::read(log.path()).unwrap();

            let result = write_doc(&t, Some(&log), Expect::Rev(Rev::of(TASKS)), CHECKBOX, tick);

            if !matches!(result, Err(WriteErr::ReadOnly))
                || std::fs::read(&t.canonical).unwrap() != TASKS
                || std::fs::read(log.path()).unwrap() != review
            {
                bad.push(format!("{name}: {result:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn a_rev_the_log_has_but_the_disk_left_is_changed_since() {
        let dir = tempfile::tempdir().unwrap();
        let (t, log) = logged(dir.path(), TASKS);
        let outside = b"- [ ] a\n- [ ] b\n- [ ] c\n";
        std::fs::write(&t.canonical, outside).unwrap();
        assert_eq!(head(&log), Some(Rev::of(TASKS)), "setup");

        // The tick applies to both texts, so only the rev check can refuse it.
        let result = write_doc(&t, Some(&log), Expect::Rev(Rev::of(TASKS)), CHECKBOX, tick);

        assert!(matches!(result, Err(WriteErr::ChangedSince)), "{result:?}");
        assert_eq!(std::fs::read(&t.canonical).unwrap(), outside);
        assert!(edits(&log).is_empty());
    }

    // Optional: a doc over the rev-only size, and the checkpoint cadence.

    #[test]
    fn a_rev_only_doc_records_the_edit_without_a_patch() {
        let dir = tempfile::tempdir().unwrap();
        let mut big = b"- [ ] a\n\n".to_vec();
        big.extend(b"filler line\n".repeat(REV_ONLY_OVER / 12 + 1));
        let (t, log) = logged(dir.path(), &big);

        write_doc(&t, Some(&log), Expect::Rev(Rev::of(&big)), CHECKBOX, tick).unwrap();

        let disk = std::fs::read(&t.canonical).unwrap();
        let all = edits(&log);
        let [edit] = &all[..] else { panic!("{all:#?}") };
        assert!(
            matches!(&edit.op, Op::Edit { rev, to_rev, patch: None, act: Act::Checkbox, .. }
                if *rev == Rev::of(&big) && *to_rev == Rev::of(&disk)),
            "{edit:?}"
        );
        assert_eq!(head(&log), Some(Rev::of(&disk)));
    }

    #[test]
    fn every_twentieth_patch_also_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let (t, log) = logged(dir.path(), TASKS);

        for i in 0..CHECKPOINT_EVERY {
            let bytes = std::fs::read(&t.canonical).unwrap();
            let checked = i % 2 == 0;
            write_doc(
                &t,
                Some(&log),
                Expect::Rev(Rev::of(&bytes)),
                CHECKBOX,
                |b| toggle_task_line(b, 1, checked).ok_or(WriteErr::NotApplicable),
            )
            .unwrap();
        }

        let all = lines(&log);
        let checkpoints: Vec<_> = all
            .windows(2)
            .filter_map(|w| match (&w[0].op, &w[1].op) {
                (
                    Op::Edit { to_rev, .. },
                    Op::Snap {
                        rev,
                        checkpoint: true,
                        ..
                    },
                ) => Some(to_rev == rev),
                _ => None,
            })
            .collect();
        assert_eq!(checkpoints, [true], "{all:#?}");
        let disk = std::fs::read(&t.canonical).unwrap();
        assert_eq!(head(&log), Some(Rev::of(&disk)));
    }
}
