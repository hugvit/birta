//! The one strict write path for document bytes.

use std::io;
use std::path::PathBuf;

use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document};

use crate::rev::Rev;
use crate::{atomic_write, paths, render};

/// A document already resolved and authorised by `Roots::resolve_doc(.., Access::Write)`.
pub(crate) struct WriteTarget {
    pub(crate) canonical: PathBuf,
    #[expect(dead_code, reason = "for edit recorders, which arrive with reviews")]
    pub(crate) rel: String,
}

/// What the caller believes is on disk.
pub(crate) enum Expect {
    Rev(Rev),
}

/// Why the document is being written.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Via {
    #[expect(dead_code, reason = "for edit recorders, which arrive with reviews")]
    Checkbox { line: u32 },
}

#[derive(Debug)]
pub(crate) enum WriteErr {
    /// Disk bytes differ from the expected rev, or changed while the edit ran.
    ChangedSince,
    /// The edit no longer applies to the current bytes.
    NotApplicable,
    ReadOnly,
    Io(io::Error),
}

#[derive(Debug)]
pub(crate) struct Written {
    pub(crate) rev: Rev,
}

/// Records a human edit in the review file; nothing implements it until reviews exist.
pub(crate) trait EditRecorder {
    fn record(&self, target: &WriteTarget, via: Via, from: &Rev, to: &Rev) -> io::Result<()>;
}

/// Reads the target, checks `expect`, applies `edit`, and replaces the file atomically.
/// Output identical to the input writes nothing.
pub(crate) fn write_doc(
    t: &WriteTarget,
    rec: Option<&dyn EditRecorder>,
    expect: Expect,
    via: Via,
    edit: impl FnOnce(&[u8]) -> Result<Vec<u8>, WriteErr>,
) -> Result<Written, WriteErr> {
    let before = std::fs::read(&t.canonical).map_err(WriteErr::Io)?;
    let from = Rev::of(&before);
    let Expect::Rev(expected) = expect;
    if expected != from {
        return Err(WriteErr::ChangedSince);
    }
    let after = edit(&before)?;
    if after == before {
        return Ok(Written { rev: from });
    }
    // A rename would replace a read-only file, so check before writing.
    if !paths::is_writable(&t.canonical) {
        return Err(WriteErr::ReadOnly);
    }
    // Narrows the window for an editor that saves without taking any lock.
    let unchanged = || std::fs::read(&t.canonical).is_ok_and(|now| now == before);
    if !atomic_write::write_atomic_if(&t.canonical, &after, unchanged).map_err(WriteErr::Io)? {
        return Err(WriteErr::ChangedSince);
    }
    let to = Rev::of(&after);
    if let Some(rec) = rec {
        rec.record(t, via, &from, &to).map_err(WriteErr::Io)?;
    }
    Ok(Written { rev: to })
}

/// Flips the task marker of 1-based `line`, keeping every other byte (CRLF, BOM, missing
/// final newline). `None` unless comrak parses a task item starting on that line that is
/// currently in the opposite state.
pub(crate) fn toggle_task_line(bytes: &[u8], line: usize, checked: bool) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let arena = Arena::new();
    let root = parse_document(&arena, text, &render::parse_options());
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
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::time::{Duration, SystemTime};

    use super::*;

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

    impl EditRecorder for Spy {
        fn record(&self, _: &WriteTarget, _: Via, from: &Rev, to: &Rev) -> io::Result<()> {
            self.0.borrow_mut().push((from.clone(), to.clone()));
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
}
