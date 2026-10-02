use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use birta::rev::Rev;
use birta::review::Reject;
use birta::review::chain::Chain;
use birta::review::fold::scan;
use birta::review::log::{AppendCtx, AppendError, Log, Tail, Tailer, approve_ops};
use birta::review::ops::{self, At, By, ByKind, Header, Line, Op, Parsed, ParsedLine, Via};
use serde_json::Value;

const HEADER: &str = r#"{"birta":"review/1","doc":"pr.md","about":"ops after the last hand-back are drafts; don't act on them"}"#;
const BASELINE: &str = "# Title\n\nSome text.\n";

fn human() -> By {
    By {
        kind: ByKind::Human,
        name: None,
        via: Some(Via::Browser),
    }
}

fn agent(name: &str) -> By {
    By {
        kind: ByKind::Agent,
        name: Some(name.to_string()),
        via: Some(Via::Cli),
    }
}

fn ctx(now_ms: u64) -> AppendCtx {
    AppendCtx {
        now_ms,
        by: human(),
        cid: None,
    }
}

fn note(body: &str) -> Op {
    Op::Comment {
        at: At::Doc,
        verdict: None,
        body: Some(body.to_string()),
        to: None,
        kind: None,
        turn: None,
    }
}

fn anything(_: &Parsed, _: u64) -> Result<(), Reject> {
    Ok(())
}

fn review_path(dir: &Path) -> PathBuf {
    dir.join("pr.md.birta.jsonl")
}

/// A log handle that never runs git, for tests that aren't about the exclude.
fn open(path: &Path) -> Log {
    Log::open(path).git_exclude(false)
}

/// A fresh review file for `pr.md` with lines 1-3 written.
fn created(dir: &Path) -> Log {
    let log = open(&review_path(dir));
    assert!(log.create_if_missing("pr.md", BASELINE, None).unwrap());
    log
}

fn read(log: &Log) -> Parsed {
    log.read().unwrap().expect("the review file exists")
}

fn raw_lines(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap();
    let body = text.strip_suffix('\n').expect("the file ends in a newline");
    body.split('\n').map(str::to_string).collect()
}

fn raw_n(raw: &str) -> u64 {
    let value: Value = serde_json::from_str(raw).unwrap();
    value["n"].as_u64().expect("every written line carries n")
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn line_n(line: &ParsedLine) -> u32 {
    match line {
        ParsedLine::Ok(line) => line.n,
        ParsedLine::Unknown(unknown) => unknown.n,
        ParsedLine::Bad(bad) => bad.n,
    }
}

fn append_raw(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

// --- creation ---

#[test]
fn create_if_missing_writes_the_header_snap_and_owner() {
    let dir = tempfile::tempdir().unwrap();
    let before = epoch_ms();
    let log = created(dir.path());
    let after = epoch_ms();
    assert_eq!(log.path(), review_path(dir.path()));

    let raw = raw_lines(log.path());
    assert_eq!(raw.len(), 3);
    assert_eq!(raw[0], HEADER);
    assert_eq!(
        serde_json::from_str::<Header>(&raw[0]).unwrap(),
        Header::new("pr.md")
    );
    assert_eq!(raw_n(&raw[1]), 2);
    assert_eq!(raw_n(&raw[2]), 3);
    let owner: Value = serde_json::from_str(&raw[2]).unwrap();
    assert!(
        owner["drafted_by"].is_null(),
        "an unnamed drafter is null: {owner}"
    );

    let parsed = read(&log);
    assert!(!parsed.read_only && parsed.bad.is_empty() && !parsed.torn_tail);
    let [snap, owner] = &parsed.lines[..] else {
        panic!("two op lines: {:?}", parsed.lines)
    };
    assert_eq!(snap.n, 2);
    assert_eq!(snap.by.kind, ByKind::Birta);
    assert_eq!(
        snap.op,
        Op::Snap {
            rev: Rev::of(BASELINE.as_bytes()),
            text: BASELINE.to_string(),
            checkpoint: false,
        }
    );
    assert_eq!(owner.n, 3);
    assert_eq!(owner.by.kind, ByKind::Birta);
    assert_eq!(owner.op, Op::Owner { drafted_by: None });
    for line in [snap, owner] {
        let ts = line.ts.expect("creation lines carry ts");
        assert!(
            (before..=after).contains(&ts),
            "{ts} not in {before}..={after}"
        );
    }
}

#[test]
fn the_baseline_is_kept_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = "\u{feff}# Títle\r\n\r\nno final eol";
    let log = open(&review_path(dir.path()));
    assert!(log.create_if_missing("pr.md", baseline, None).unwrap());
    let parsed = read(&log);
    assert_eq!(
        parsed.lines[0].op,
        Op::Snap {
            rev: Rev::of(baseline.as_bytes()),
            text: baseline.to_string(),
            checkpoint: false,
        }
    );
}

#[test]
fn create_if_missing_records_a_named_drafter() {
    let dir = tempfile::tempdir().unwrap();
    let log = open(&review_path(dir.path()));
    assert!(
        log.create_if_missing("pr.md", BASELINE, Some("claude"))
            .unwrap()
    );
    assert_eq!(
        read(&log).lines[1].op,
        Op::Owner {
            drafted_by: Some("claude".to_string())
        }
    );
}

#[test]
fn create_if_missing_is_idempotent_and_the_first_drafter_wins() {
    let dir = tempfile::tempdir().unwrap();
    let log = open(&review_path(dir.path()));
    assert!(
        log.create_if_missing("pr.md", BASELINE, Some("claude"))
            .unwrap()
    );
    let first = fs::read(log.path()).unwrap();
    assert!(!log.create_if_missing("pr.md", "other\n", None).unwrap());
    assert!(
        !log.create_if_missing("other.md", BASELINE, Some("pi"))
            .unwrap()
    );
    assert_eq!(fs::read(log.path()).unwrap(), first);
}

#[test]
fn an_empty_file_counts_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = review_path(dir.path());
    fs::write(&path, "").unwrap();
    let log = open(&path);
    assert!(log.create_if_missing("pr.md", BASELINE, None).unwrap());
    assert_eq!(raw_lines(&path).len(), 3);
    assert!(!read(&log).read_only);
}

#[test]
fn a_file_with_a_header_or_any_second_line_is_never_rewritten() {
    let later = r#"{"birta":"review/2","doc":"pr.md","about":"x"}"#;
    let later_nl = format!("{later}\n");
    let then_op = |first: &str| format!("{first}\n{}\n", r#"{"op":"claim","round":2}"#);
    let (garbage, torn, blank) = (
        then_op("not json"),
        then_op("{\"birta\":\"rev"),
        then_op(""),
    );
    let torn_second = "not json\n{\"op\":\"cl";
    for existing in [
        later,
        &later_nl,
        &then_op(HEADER),
        &garbage,
        &torn,
        &blank,
        torn_second,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = review_path(dir.path());
        fs::write(&path, existing).unwrap();
        assert!(
            !open(&path)
                .create_if_missing("pr.md", BASELINE, None)
                .unwrap(),
            "{existing:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), existing);
    }
}

#[test]
fn a_creation_that_crashed_on_line_one_is_created_again() {
    let header_only = format!("{HEADER}\n");
    let minor = r#"{"birta":"review/1.4","doc":"pr.md","about":"x"}"#;
    for existing in [
        "{\"birta\":\"rev",
        "{\"birta\":\"review/1\",\"doc\"\n",
        "not json\n",
        "not json",
        "\n",
        "\r\n",
        HEADER,
        &header_only,
        minor,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = review_path(dir.path());
        fs::write(&path, existing).unwrap();
        let log = open(&path);
        assert!(
            log.create_if_missing("pr.md", BASELINE, Some("claude"))
                .unwrap(),
            "{existing:?}"
        );
        let raw = raw_lines(&path);
        assert_eq!(raw.len(), 3, "{existing:?}");
        assert_eq!(raw[0], HEADER);
        let parsed = read(&log);
        assert!(parsed.bad.is_empty() && !parsed.read_only, "{existing:?}");
        assert_eq!(
            parsed.lines[1].op,
            Op::Owner {
                drafted_by: Some("claude".to_string())
            }
        );
    }
}

#[test]
fn creation_leaves_nothing_else_in_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    assert!(!log.create_if_missing("pr.md", BASELINE, None).unwrap());
    let names: HashSet<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, HashSet::from(["pr.md.birta.jsonl".to_string()]));
}

#[test]
fn a_review_file_appears_whole_or_not_at_all() {
    // A large baseline widens the window a non-atomic write would leave open.
    let baseline = "word ".repeat(2 * 1024 * 1024) + "\n";
    for _ in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let path = review_path(dir.path());
        let watcher = {
            let path = path.clone();
            thread::spawn(move || {
                let start = Instant::now();
                loop {
                    assert!(start.elapsed() < Duration::from_secs(60), "never appeared");
                    if let Ok(bytes) = fs::read(&path) {
                        return bytes;
                    }
                }
            })
        };
        assert!(
            open(&path)
                .create_if_missing("pr.md", &baseline, None)
                .unwrap()
        );
        let first_seen = watcher.join().unwrap();
        assert_eq!(
            first_seen,
            fs::read(&path).unwrap(),
            "seen before it was whole"
        );
    }
}

#[cfg(unix)]
#[test]
fn the_review_file_takes_the_docs_permissions_without_exec() {
    use std::os::unix::fs::PermissionsExt;

    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    for (doc_mode, start, want) in [
        (0o640, None, 0o640),
        (0o600, None, 0o600),
        (0o755, None, 0o644),
        (0o640, Some("{\"birta\":\"rev"), 0o640),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("pr.md");
        fs::write(&doc, BASELINE).unwrap();
        fs::set_permissions(&doc, fs::Permissions::from_mode(doc_mode)).unwrap();
        let path = review_path(dir.path());
        if let Some(start) = start {
            fs::write(&path, start).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        }
        assert!(
            open(&path)
                .create_if_missing("pr.md", BASELINE, None)
                .unwrap()
        );
        assert_eq!(mode(&path), want, "doc {doc_mode:o}, start {start:?}");
    }

    // The folder file follows its folder.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o750)).unwrap();
    let folder_file = root.join(".birta.jsonl");
    assert!(
        open(&folder_file)
            .create_if_missing("root", BASELINE, None)
            .unwrap()
    );
    assert_eq!(mode(&folder_file), 0o640);
}

#[cfg(unix)]
#[test]
fn a_read_only_doc_still_takes_notes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("pr.md");
    fs::write(&doc, BASELINE).unwrap();
    fs::set_permissions(&doc, fs::Permissions::from_mode(0o444)).unwrap();
    let log = created(dir.path());
    let path = review_path(dir.path());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    let n = log
        .append(ctx(1_000), vec![note("hi")], anything)
        .expect("notes still work on a read-only doc");
    assert_eq!(n, [4]);
}

#[test]
fn create_if_missing_never_creates_folders() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone").join("pr.md.birta.jsonl");
    assert!(
        open(&path)
            .create_if_missing("pr.md", BASELINE, None)
            .is_err()
    );
    assert!(!dir.path().join("gone").exists());
}

// --- reading and appending ---

#[test]
fn a_missing_file_reads_as_none_and_append_never_creates_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = review_path(dir.path());
    let log = open(&path);
    assert!(log.read().unwrap().is_none());
    let err = log
        .append(ctx(1_000), vec![note("hi")], anything)
        .unwrap_err();
    assert!(
        matches!(&err, AppendError::Io(e) if e.kind() == std::io::ErrorKind::NotFound),
        "{err:?}"
    );
    assert!(!path.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn append_lands_at_the_next_line_with_the_envelope_filled() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let before = fs::read_to_string(log.path()).unwrap();
    let landed = log
        .append(
            AppendCtx {
                now_ms: 1_790_000_000_000,
                by: human(),
                cid: Some("c-1".to_string()),
            },
            vec![note("hi")],
            anything,
        )
        .unwrap();
    assert_eq!(landed, [4]);
    assert_eq!(
        fs::read_to_string(log.path()).unwrap(),
        format!(
            "{before}{}\n",
            r#"{"n":4,"ts":1790000000000,"by":{"type":"human","via":"browser"},"cid":"c-1","op":"comment","at":"doc","body":"hi"}"#
        )
    );
}

#[test]
fn one_append_lands_all_its_ops_in_order_on_consecutive_lines() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    log.append(ctx(1_000), vec![note("first")], anything)
        .unwrap();
    let landed = log
        .append(
            AppendCtx {
                now_ms: 2_000,
                by: agent("claude"),
                cid: Some("batch".to_string()),
            },
            vec![
                Op::Done {
                    to: 4,
                    turn: 9,
                    late: false,
                },
                Op::TurnNote {
                    body: "did it".to_string(),
                    turn: 9,
                    late: false,
                },
                Op::Yield {
                    turn: Some(9),
                    release: false,
                },
            ],
            anything,
        )
        .unwrap();
    assert_eq!(landed, [5, 6, 7]);
    let parsed = read(&log);
    let tail: Vec<&Line> = parsed.lines.iter().skip(3).collect();
    assert_eq!(tail.len(), 3);
    for (line, n) in tail.iter().zip(5..) {
        assert_eq!(line.n, n);
        assert_eq!(line.ts, Some(2_000));
        assert_eq!(line.by, agent("claude"));
        assert_eq!(line.cid.as_deref(), Some("batch"));
    }
    assert_eq!(
        tail[0].op,
        Op::Done {
            to: 4,
            turn: 9,
            late: false
        }
    );
    assert!(matches!(tail[2].op, Op::Yield { turn: Some(9), .. }));
    let raw = raw_lines(log.path());
    assert_eq!(
        raw.iter().skip(1).map(|l| raw_n(l)).collect::<Vec<_>>(),
        [2, 3, 4, 5, 6, 7]
    );
}

#[test]
fn the_guard_sees_the_fresh_file_and_the_append_clock() {
    let dir = tempfile::tempdir().unwrap();
    let path = review_path(dir.path());
    let early = created(dir.path());
    // Another handle writes after `early` was opened.
    open(&path)
        .append(ctx(1_000), vec![note("from elsewhere")], anything)
        .unwrap();
    let mut seen = None;
    early
        .append(ctx(4_242), vec![note("mine")], |parsed, now| {
            seen = Some((parsed.lines.iter().map(|l| l.n).collect::<Vec<_>>(), now));
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, Some((vec![2, 3, 4], 4_242)));
}

#[test]
fn a_refused_guard_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    append_raw(log.path(), br#"{"n":4,"op":"comm"#);
    let before = fs::read(log.path()).unwrap();
    let err = log
        .append(ctx(1_000), vec![note("hi")], |_, _| {
            Err(Reject::AlreadyClaimed)
        })
        .unwrap_err();
    assert!(
        matches!(err, AppendError::Rejected(Reject::AlreadyClaimed)),
        "{err:?}"
    );
    // Not even the newline that would close the torn line.
    assert_eq!(fs::read(log.path()).unwrap(), before);
}

#[test]
fn append_refuses_a_log_it_cannot_read_whatever_the_guard_says() {
    let later = format!("{}\n", r#"{"birta":"review/2","doc":"pr.md","about":"x"}"#);
    let op = r#"{"op":"claim","round":2}"#;
    let later_with_op = format!("{later}{op}\n");
    let garbage = format!("garbage\n{op}\n");
    let no_header = format!("{op}\n{op}\n");
    for existing in [
        later.as_str(),
        later_with_op.as_str(),
        garbage.as_str(),
        no_header.as_str(),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = review_path(dir.path());
        fs::write(&path, existing).unwrap();
        let err = open(&path)
            .append(ctx(1_000), vec![note("hi")], anything)
            .unwrap_err();
        assert!(
            matches!(err, AppendError::Rejected(Reject::ReadOnly)),
            "{existing:?}: {err:?}"
        );
        let err = open(&path)
            .append_with(ctx(1_000), |_, _| Ok(vec![note("hi")]))
            .unwrap_err();
        assert!(
            matches!(err, AppendError::Rejected(Reject::ReadOnly)),
            "append_with {existing:?}: {err:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), existing);
    }
}

#[test]
fn a_file_left_by_a_crashed_creation_reads_and_appends_as_missing() {
    let header_only = format!("{HEADER}\n");
    for existing in [
        "",
        "{\"birta\":\"rev",
        "garbage\n",
        "\n",
        HEADER,
        &header_only,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = review_path(dir.path());
        fs::write(&path, existing).unwrap();
        let log = open(&path);
        assert!(log.read().unwrap().is_none(), "{existing:?}");
        let missing = |err: AppendError| matches!(&err, AppendError::Io(e) if e.kind() == std::io::ErrorKind::NotFound);
        let err = log
            .append(ctx(1_000), vec![note("hi")], anything)
            .unwrap_err();
        assert!(missing(err), "{existing:?}");
        let err = log
            .append_with(ctx(1_000), |_, _| Ok(vec![note("hi")]))
            .unwrap_err();
        assert!(missing(err), "{existing:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), existing);
    }
}

// --- torn tails ---

#[test]
fn torn_tail_is_an_unterminated_last_line_that_does_not_parse() {
    let torn = |text: &[u8]| ops::parse(text).torn_tail;
    assert!(torn(format!("{HEADER}\n{{\"op\":\"cl").as_bytes()));
    assert!(torn(b"{\"birta\":\"rev"));
    // A write cut inside a multi-byte character.
    let mut cut = format!("{HEADER}\n{{\"op\":\"comment\",\"body\":\"caf").into_bytes();
    cut.push(0xc3);
    assert!(torn(&cut));

    assert!(!torn(
        format!("{HEADER}\n{{\"op\":\"claim\",\"round\":2}}").as_bytes()
    ));
    assert!(!torn(format!("{HEADER}\n{{\"op\":\"cl\n").as_bytes()));
    assert!(!torn(format!("{HEADER}\n").as_bytes()));
    assert!(!torn(HEADER.as_bytes()));
    assert!(!torn(b""));
}

#[test]
fn a_torn_line_is_closed_kept_as_bad_and_the_append_lands_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    append_raw(log.path(), br#"{"n":4,"op":"comm"#);
    let before = fs::read_to_string(log.path()).unwrap();
    let parsed = read(&log);
    assert!(parsed.torn_tail);
    assert_eq!(parsed.bad.iter().map(|b| b.n).collect::<Vec<_>>(), [4]);

    assert_eq!(
        log.append(ctx(5_000), vec![note("hi")], anything).unwrap(),
        [5]
    );
    assert_eq!(
        fs::read_to_string(log.path()).unwrap(),
        format!(
            "{before}\n{}\n",
            r#"{"n":5,"ts":5000,"by":{"type":"human","via":"browser"},"op":"comment","at":"doc","body":"hi"}"#
        )
    );
    let parsed = read(&log);
    assert!(!parsed.torn_tail);
    assert_eq!(parsed.bad.iter().map(|b| b.n).collect::<Vec<_>>(), [4]);
    assert_eq!(parsed.lines.last().unwrap().n, 5);
}

#[test]
fn a_complete_last_line_without_a_newline_gets_one_before_the_append() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let hand = r#"{"op":"comment","at":"doc","body":"typed by hand"}"#;
    append_raw(log.path(), hand.as_bytes());
    let parsed = read(&log);
    assert!(!parsed.torn_tail);
    assert_eq!(parsed.lines.last().unwrap().n, 4);

    let before = fs::read_to_string(log.path()).unwrap();
    assert_eq!(
        log.append(ctx(5_000), vec![note("hi")], anything).unwrap(),
        [5]
    );
    let after = fs::read_to_string(log.path()).unwrap();
    assert_eq!(after.strip_prefix(&before).unwrap().as_bytes()[0], b'\n');
    let parsed = read(&log);
    assert!(parsed.bad.is_empty());
    assert_eq!(
        parsed.lines.iter().map(|l| l.n).collect::<Vec<_>>(),
        [2, 3, 4, 5]
    );
}

#[test]
fn a_file_ending_in_a_newline_gets_no_blank_line() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let before = fs::read_to_string(log.path()).unwrap();
    log.append(ctx(5_000), vec![note("hi")], anything).unwrap();
    let after = fs::read_to_string(log.path()).unwrap();
    let added = after.strip_prefix(&before).unwrap();
    assert!(added.starts_with(r#"{"n":4,"#), "{added:?}");
    assert_eq!(added.matches('\n').count(), 1);
    assert!(read(&log).bad.is_empty());
}

#[test]
fn invalid_utf8_on_disk_spoils_only_its_own_line() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    append_raw(
        log.path(),
        b"{\"op\":\"comment\",\"at\":\"doc\",\"body\":\"caf\xe9\"}\n",
    );
    append_raw(log.path(), br#"{"op":"comment","at":"doc","body":"fine"}"#);
    append_raw(log.path(), b"\n");
    let parsed = read(&log);
    assert_eq!(parsed.bad.iter().map(|b| b.n).collect::<Vec<_>>(), [4]);
    assert_eq!(
        parsed.lines.iter().map(|l| l.n).collect::<Vec<_>>(),
        [2, 3, 5]
    );
    assert!(!parsed.read_only);
    assert_eq!(
        log.append(ctx(5_000), vec![note("hi")], anything).unwrap(),
        [6]
    );
}

// --- file identity ---

#[test]
fn an_append_waiting_on_a_file_that_gets_replaced_lands_in_the_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let path = log.path().to_path_buf();
    let before = fs::read(&path).unwrap();

    let mut old = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&path)
        .unwrap();
    old.lock().unwrap();
    let appender = {
        let path = path.clone();
        thread::spawn(move || {
            open(&path).append(ctx(5_000), vec![note("after the swap")], anything)
        })
    };
    // Let the appender open the old file and block on its lock.
    thread::sleep(Duration::from_millis(300));

    // A sync tool replaces the file with a copy that has one more line.
    let mut replaced = before.clone();
    replaced.extend_from_slice(
        br#"{"n":4,"ts":1,"by":{"type":"human"},"op":"comment","at":"doc","body":"synced in"}"#,
    );
    replaced.push(b'\n');
    let tmp = dir.path().join("swap.tmp");
    fs::write(&tmp, &replaced).unwrap();
    fs::rename(&tmp, &path).unwrap();
    old.unlock().unwrap();

    assert_eq!(appender.join().unwrap().unwrap(), [5]);
    let parsed = read(&open(&path));
    let last = parsed.lines.last().unwrap();
    assert_eq!(last.n, 5);
    assert_eq!(last.op, note("after the swap"));
    let mut old_bytes = Vec::new();
    old.seek(SeekFrom::Start(0)).unwrap();
    old.read_to_end(&mut old_bytes).unwrap();
    assert_eq!(old_bytes, before, "nothing lands in the replaced file");
}

#[test]
fn after_a_swap_the_append_waits_for_the_new_files_lock() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let path = log.path().to_path_buf();
    let before = fs::read(&path).unwrap();

    let old = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&path)
        .unwrap();
    old.lock().unwrap();
    let appender = {
        let path = path.clone();
        thread::spawn(move || open(&path).append(ctx(5_000), vec![note("mine")], anything))
    };
    thread::sleep(Duration::from_millis(300));

    let tmp = dir.path().join("swap.tmp");
    fs::write(&tmp, &before).unwrap();
    fs::rename(&tmp, &path).unwrap();
    let mut new = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&path)
        .unwrap();
    new.lock().unwrap();
    old.unlock().unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "wrote without the new lock"
    );

    new.write_all(
        br#"{"n":4,"ts":1,"by":{"type":"human"},"op":"comment","at":"doc","body":"first"}"#,
    )
    .unwrap();
    new.write_all(b"\n").unwrap();
    new.unlock().unwrap();
    assert_eq!(appender.join().unwrap().unwrap(), [5]);
    let parsed = read(&open(&path));
    assert_eq!(
        parsed.lines.iter().map(|l| l.n).collect::<Vec<_>>(),
        [2, 3, 4, 5]
    );
    assert_eq!(parsed.lines[3].op, note("mine"));
}

#[test]
fn an_append_waiting_on_a_deleted_file_fails_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let path = log.path().to_path_buf();
    let old = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&path)
        .unwrap();
    old.lock().unwrap();
    let appender = {
        let path = path.clone();
        thread::spawn(move || open(&path).append(ctx(5_000), vec![note("lost")], anything))
    };
    thread::sleep(Duration::from_millis(300));
    fs::remove_file(&path).unwrap();
    old.unlock().unwrap();
    let err = appender.join().unwrap().unwrap_err();
    assert!(
        matches!(&err, AppendError::Io(e) if e.kind() == std::io::ErrorKind::NotFound),
        "{err:?}"
    );
    assert!(!path.exists());
}

// --- approve on a rev-only chain ---

#[test]
fn approving_a_doc_known_only_by_rev_writes_a_snap_first() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let big = "x".repeat(1024 * 1024 + 1);
    let big_rev = Rev::of(big.as_bytes());
    log.append(
        AppendCtx {
            now_ms: 2_000,
            by: By {
                kind: ByKind::Birta,
                name: None,
                via: None,
            },
            cid: None,
        },
        vec![Op::External {
            rev: Rev::of(BASELINE.as_bytes()),
            to_rev: big_rev.clone(),
            patch: None,
            cause: None,
            head: None,
        }],
        anything,
    )
    .unwrap();
    assert!(Chain::build(&read(&log)).text(&big_rev).is_none());

    let landed = log
        .append_with(ctx(9_000), |parsed, now| {
            Ok(approve_ops(&Chain::build(parsed), &big, now + 5_000, 5_000))
        })
        .unwrap();
    assert_eq!(landed, [5, 6]);
    let parsed = read(&log);
    assert_eq!(
        parsed.lines[3].op,
        Op::Snap {
            rev: big_rev.clone(),
            text: big.clone(),
            checkpoint: true,
        }
    );
    assert_eq!(
        parsed.lines[4].op,
        Op::Approve {
            rev: big_rev.clone(),
            not_before: 14_000,
            hold_ms: 5_000,
        }
    );
    assert_eq!(
        Chain::build(&parsed).text(&big_rev).as_deref(),
        Some(big.as_str())
    );
}

#[test]
fn approving_a_doc_the_chain_can_rebuild_writes_only_the_approve() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let chain = Chain::build(&read(&log));
    assert_eq!(
        approve_ops(&chain, BASELINE, 7_000, 5_000),
        [Op::Approve {
            rev: Rev::of(BASELINE.as_bytes()),
            not_before: 7_000,
            hold_ms: 5_000,
        }]
    );
    let landed = log
        .append_with(ctx(2_000), |parsed, now| {
            Ok(approve_ops(
                &Chain::build(parsed),
                BASELINE,
                now + 5_000,
                5_000,
            ))
        })
        .unwrap();
    assert_eq!(landed, [4]);
}

#[test]
fn a_refused_builder_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let before = fs::read(log.path()).unwrap();
    let err = log
        .append_with(ctx(1_000), |_, _| Err(Reject::StaleRev))
        .unwrap_err();
    assert!(
        matches!(err, AppendError::Rejected(Reject::StaleRev)),
        "{err:?}"
    );
    assert_eq!(fs::read(log.path()).unwrap(), before);
}

// --- Tailer ---

fn appended_ns(tail: Tail) -> Vec<u32> {
    match tail {
        Tail::Appended(lines) => lines.iter().map(line_n).collect(),
        other => panic!("expected Appended, got {other:?}"),
    }
}

#[test]
fn a_tailer_resets_on_first_sight_then_reports_exactly_the_appended_lines() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let mut tailer = Tailer::new();
    assert!(matches!(tailer.poll(&log), Tail::Reset));
    assert!(matches!(tailer.poll(&log), Tail::Unchanged));
    log.append(ctx(1_000), vec![note("a"), note("b")], anything)
        .unwrap();
    assert_eq!(appended_ns(tailer.poll(&log)), [4, 5]);
    assert!(matches!(tailer.poll(&log), Tail::Unchanged));
    log.append(ctx(2_000), vec![note("c")], anything).unwrap();
    let Tail::Appended(lines) = tailer.poll(&log) else {
        panic!("expected Appended")
    };
    assert!(matches!(&lines[..], [ParsedLine::Ok(line)] if line.n == 6 && line.op == note("c")));
}

#[test]
fn a_tailer_waits_for_a_partial_line_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let mut tailer = Tailer::new();
    assert!(matches!(tailer.poll(&log), Tail::Reset));
    append_raw(log.path(), br#"{"op":"claim","#);
    assert!(matches!(tailer.poll(&log), Tail::Unchanged));
    append_raw(log.path(), b"\"round\":5}\n");
    let Tail::Appended(lines) = tailer.poll(&log) else {
        panic!("expected Appended")
    };
    assert!(
        matches!(&lines[..], [ParsedLine::Ok(line)] if line.n == 4 && line.op == Op::Claim { round: 5, target: None }),
        "{lines:?}"
    );
}

#[test]
fn a_tailer_never_reports_the_repair_newline() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let mut tailer = Tailer::new();
    assert!(matches!(tailer.poll(&log), Tail::Reset));
    append_raw(
        log.path(),
        br#"{"op":"comment","at":"doc","body":"typed by hand"}"#,
    );
    let first = tailer.poll(&log);
    log.append(ctx(5_000), vec![note("hi")], anything).unwrap();
    let mut ns = match first {
        Tail::Unchanged => Vec::new(),
        other => appended_ns(other),
    };
    let Tail::Appended(lines) = tailer.poll(&log) else {
        panic!("expected Appended")
    };
    assert!(
        lines.iter().all(|l| matches!(l, ParsedLine::Ok(_))),
        "{lines:?}"
    );
    ns.extend(lines.iter().map(line_n));
    assert_eq!(ns, [4, 5]);
    assert!(matches!(tailer.poll(&log), Tail::Unchanged));
}

#[test]
fn a_tailer_resets_when_the_file_shrinks() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    log.append(ctx(1_000), vec![note("a")], anything).unwrap();
    let mut tailer = Tailer::new();
    tailer.poll(&log);
    let text = fs::read_to_string(log.path()).unwrap();
    let cut = text.trim_end_matches('\n').rfind('\n').unwrap() + 1;
    OpenOptions::new()
        .write(true)
        .open(log.path())
        .unwrap()
        .set_len(cut as u64)
        .unwrap();
    assert!(matches!(tailer.poll(&log), Tail::Reset));
    assert!(matches!(tailer.poll(&log), Tail::Unchanged));
}

#[test]
fn a_tailer_resets_when_the_file_is_replaced_even_with_the_same_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let mut tailer = Tailer::new();
    tailer.poll(&log);
    let tmp = dir.path().join("swap.tmp");
    fs::copy(log.path(), &tmp).unwrap();
    fs::rename(&tmp, log.path()).unwrap();
    assert!(matches!(tailer.poll(&log), Tail::Reset));
    assert!(matches!(tailer.poll(&log), Tail::Unchanged));
}

#[test]
fn a_tailer_resets_when_bytes_before_its_offset_change_under_an_append() {
    let dir = tempfile::tempdir().unwrap();
    let log = created(dir.path());
    let mut tailer = Tailer::new();
    tailer.poll(&log);
    let text = fs::read_to_string(log.path()).unwrap();
    let at = text.rfind("Title").unwrap();
    let mut file = OpenOptions::new().write(true).open(log.path()).unwrap();
    file.seek(SeekFrom::Start(at as u64)).unwrap();
    file.write_all(b"Tidle").unwrap();
    drop(file);
    append_raw(
        log.path(),
        b"{\"op\":\"comment\",\"at\":\"doc\",\"body\":\"x\"}\n",
    );
    assert!(matches!(tailer.poll(&log), Tail::Reset));
}

#[test]
fn a_tailer_resets_when_the_file_goes_and_again_when_it_returns() {
    let dir = tempfile::tempdir().unwrap();
    let path = review_path(dir.path());
    let log = open(&path);
    let mut tailer = Tailer::new();
    assert!(matches!(tailer.poll(&log), Tail::Unchanged));
    assert!(log.create_if_missing("pr.md", BASELINE, None).unwrap());
    assert!(matches!(tailer.poll(&log), Tail::Reset));
    fs::remove_file(&path).unwrap();
    assert!(matches!(tailer.poll(&log), Tail::Reset));
    assert!(matches!(tailer.poll(&log), Tail::Unchanged));
    assert!(log.create_if_missing("pr.md", BASELINE, None).unwrap());
    assert!(matches!(tailer.poll(&log), Tail::Reset));
}

// --- processes ---

const CHILD: &str = "BIRTA_TEST_LOG_CHILD";
const APPENDERS: usize = 4;
const APPENDS: usize = 200;
const CLAIMERS: usize = 4;
const CLAIM_LOGS: usize = 25;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is set by the parent"))
}

/// Waits until every sibling is ready, so the work below starts at once.
fn barrier(dir: &Path, id: &str) {
    fs::write(dir.join(format!("ready-{id}")), "").unwrap();
    let start = Instant::now();
    while !dir.join("go").exists() {
        assert!(start.elapsed() < Duration::from_secs(60), "no go");
        thread::sleep(Duration::from_millis(1));
    }
}

fn claim_log(dir: &Path, k: usize) -> PathBuf {
    dir.join(format!("c{k}.birta.jsonl"))
}

/// Runs in a child process for the multi-process tests; a no-op in the normal run.
#[test]
fn log_child() {
    let Ok(role) = std::env::var(CHILD) else {
        return;
    };
    let dir = PathBuf::from(env("BIRTA_TEST_LOG_DIR"));
    let id = env("BIRTA_TEST_LOG_ID");
    let result = match role.as_str() {
        "append" => {
            let log = open(&review_path(&dir));
            barrier(&dir, &id);
            for i in 0..APPENDS {
                let ctx = AppendCtx {
                    now_ms: i as u64,
                    by: agent(&format!("p{id}")),
                    cid: Some(format!("{id}-{i}")),
                };
                log.append(ctx, vec![note("a"), note("b")], anything)
                    .unwrap();
            }
            String::new()
        }
        "claim" => {
            barrier(&dir, &id);
            let mut out = String::new();
            for k in 0..CLAIM_LOGS {
                let ctx = AppendCtx {
                    now_ms: 10_000,
                    by: agent(&format!("p{id}")),
                    cid: None,
                };
                let result = open(&claim_log(&dir, k)).append(
                    ctx,
                    vec![Op::Claim {
                        round: 5,
                        target: None,
                    }],
                    |parsed, now| {
                        // Widens the race a lock taken too late would lose.
                        thread::sleep(Duration::from_millis(3));
                        scan(parsed, now).check_claim(5, now)
                    },
                );
                let word = match result {
                    Ok(_) => "won".to_string(),
                    Err(AppendError::Rejected(Reject::AlreadyClaimed)) => "lost".to_string(),
                    Err(AppendError::Rejected(Reject::Invalid(_))) => "withdrawn".to_string(),
                    Err(e) => format!("error {e:?}"),
                };
                out.push_str(&format!("{k} {word}\n"));
            }
            out
        }
        "withdraw" => {
            barrier(&dir, &id);
            let mut out = String::new();
            for k in 0..CLAIM_LOGS {
                let result = open(&claim_log(&dir, k)).append(
                    ctx(10_000),
                    vec![Op::Withdraw { to: 5 }],
                    |parsed, now| {
                        thread::sleep(Duration::from_millis(3));
                        scan(parsed, now).check_withdraw(5)
                    },
                );
                let word = match result {
                    Ok(_) => "won".to_string(),
                    Err(AppendError::Rejected(Reject::NotWithdrawable)) => "lost".to_string(),
                    Err(e) => format!("error {e:?}"),
                };
                out.push_str(&format!("{k} {word}\n"));
            }
            out
        }
        "create" => {
            let log = open(&review_path(&dir));
            barrier(&dir, &id);
            let created = log
                .create_if_missing("pr.md", BASELINE, Some(&format!("p{id}")))
                .unwrap();
            created.to_string()
        }
        "create-quietly" => {
            let log = open(&review_path(&dir));
            log.create_if_missing("pr.md", BASELINE, None)
                .unwrap()
                .to_string()
        }
        "git" => {
            let path = PathBuf::from(env("BIRTA_TEST_LOG_PATH"));
            let log = match env("BIRTA_TEST_LOG_EXCLUDE").as_str() {
                "default" => Log::open(&path),
                mode => Log::open(&path).git_exclude(mode == "on"),
            };
            match log.create_if_missing(&env("BIRTA_TEST_LOG_DOC"), BASELINE, None) {
                Ok(true) => "created".to_string(),
                Ok(false) => "exists".to_string(),
                Err(e) => format!("error {:?}", e.kind()),
            }
        }
        "git-race" => {
            let path = dir.join("repo").join(format!("doc{id}.md.birta.jsonl"));
            let log = Log::open(&path);
            barrier(&dir, &id);
            log.create_if_missing(&format!("doc{id}.md"), BASELINE, None)
                .unwrap()
                .to_string()
        }
        other => panic!("unknown child role {other}"),
    };
    fs::write(dir.join(format!("result-{id}")), result).unwrap();
}

fn child(role: &str, dir: &Path, id: &str) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "log_child", "--test-threads=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env(CHILD, role)
        .env("BIRTA_TEST_LOG_DIR", dir)
        .env("BIRTA_TEST_LOG_ID", id);
    cmd
}

fn finish(out: std::process::Output) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "child failed: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Starts one child per id, releases them together, and returns each one's result.
fn run_together(dir: &Path, racers: &[(&str, String)]) -> Vec<String> {
    run_together_with(dir, racers, |_| {})
}

fn run_together_with(
    dir: &Path,
    racers: &[(&str, String)],
    tweak: impl Fn(&mut Command),
) -> Vec<String> {
    let children: Vec<_> = racers
        .iter()
        .map(|(role, id)| {
            let mut cmd = child(role, dir, id);
            tweak(&mut cmd);
            cmd.spawn().unwrap()
        })
        .collect();
    let ids: Vec<&String> = racers.iter().map(|(_, id)| id).collect();
    let start = Instant::now();
    while !ids
        .iter()
        .all(|id| dir.join(format!("ready-{id}")).exists())
    {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "children never got ready"
        );
        thread::sleep(Duration::from_millis(5));
    }
    fs::write(dir.join("go"), "").unwrap();
    for c in children {
        finish(c.wait_with_output().unwrap());
    }
    ids.iter()
        .map(|id| fs::read_to_string(dir.join(format!("result-{id}"))).unwrap())
        .collect()
}

fn racers(role: &'static str, n: usize) -> Vec<(&'static str, String)> {
    (0..n).map(|i| (role, i.to_string())).collect()
}

#[test]
fn appends_from_racing_processes_land_on_unique_contiguous_lines() {
    let dir = tempfile::tempdir().unwrap();
    created(dir.path());
    run_together(dir.path(), &racers("append", APPENDERS));
    assert_contiguous_batches(&review_path(dir.path()), APPENDERS, APPENDS);
}

#[test]
fn appends_from_racing_threads_in_one_process_land_on_unique_contiguous_lines() {
    const THREADS: usize = 8;
    // Fewer than the processes' 200: every append re-reads the whole file.
    const THREAD_APPENDS: usize = 50;
    let dir = tempfile::tempdir().unwrap();
    let path = created(dir.path()).path().to_path_buf();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Reads and polls on other fds must not drop an appender's lock.
    let reader = {
        let (path, done) = (path.clone(), done.clone());
        thread::spawn(move || {
            let log = open(&path);
            let mut tailer = Tailer::new();
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                log.read().unwrap();
                tailer.poll(&log);
            }
        })
    };
    let appenders: Vec<_> = (0..THREADS)
        .map(|t| {
            let path = path.clone();
            thread::spawn(move || {
                let log = open(&path);
                for i in 0..THREAD_APPENDS {
                    let ctx = AppendCtx {
                        now_ms: i as u64,
                        by: agent(&format!("t{t}")),
                        cid: Some(format!("{t}-{i}")),
                    };
                    log.append(ctx, vec![note("a"), note("b")], anything)
                        .unwrap();
                }
            })
        })
        .collect();
    for a in appenders {
        a.join().unwrap();
    }
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    reader.join().unwrap();
    assert_contiguous_batches(&path, THREADS, THREAD_APPENDS);
}

/// Every raw `n` is its physical line, and each 2-op batch sits together in its writer's order.
fn assert_contiguous_batches(path: &Path, writers: usize, appends: usize) {
    let raw = raw_lines(path);
    let total = 3 + writers * appends * 2;
    assert_eq!(raw.len(), total);
    for (i, line) in raw.iter().enumerate().skip(1) {
        assert_eq!(raw_n(line), i as u64 + 1, "line {}: {line}", i + 1);
    }
    let parsed = ops::parse(fs::read(path).unwrap());
    assert!(parsed.bad.is_empty() && parsed.unknown.is_empty() && !parsed.torn_tail);
    assert_eq!(parsed.lines.len(), total - 1);
    let mut at: HashMap<String, Vec<(u32, Op)>> = HashMap::new();
    for line in &parsed.lines[2..] {
        let cid = line
            .cid
            .clone()
            .expect("every appended line carries its cid");
        at.entry(cid).or_default().push((line.n, line.op.clone()));
    }
    assert_eq!(at.len(), writers * appends);
    for (cid, lines) in &at {
        assert_eq!(lines.len(), 2, "{cid}");
        assert_eq!(lines[1].0, lines[0].0 + 1, "{cid} was split");
        assert_eq!(
            (&lines[0].1, &lines[1].1),
            (&note("a"), &note("b")),
            "{cid}"
        );
    }
    for w in 0..writers {
        let firsts: Vec<u32> = (0..appends).map(|i| at[&format!("{w}-{i}")][0].0).collect();
        assert!(
            firsts.windows(2).all(|w| w[0] < w[1]),
            "writer {w} out of order"
        );
    }
}

/// Logs whose line 5 is a due, unclaimed submit.
fn claim_logs(dir: &Path) {
    for k in 0..CLAIM_LOGS {
        let log = open(&claim_log(dir, k));
        assert!(log.create_if_missing("pr.md", BASELINE, None).unwrap());
        append_raw(
            log.path(),
            concat!(
                r#"{"n":4,"ts":1000,"by":{"type":"human"},"op":"comment","at":"doc","body":"x"}"#,
                "\n",
                r#"{"n":5,"ts":2000,"by":{"type":"human"},"op":"submit","upto":4,"not_before":7000,"hold_ms":5000}"#,
                "\n"
            )
            .as_bytes(),
        );
    }
}

#[test]
fn exactly_one_of_racing_claimers_wins_each_round() {
    let dir = tempfile::tempdir().unwrap();
    claim_logs(dir.path());
    let results = run_together(dir.path(), &racers("claim", CLAIMERS));

    for k in 0..CLAIM_LOGS {
        let words: Vec<(usize, &str)> = results
            .iter()
            .enumerate()
            .map(|(p, out)| {
                let line = out.lines().nth(k).unwrap();
                (p, line.strip_prefix(&format!("{k} ")).unwrap())
            })
            .collect();
        let winners: Vec<usize> = words
            .iter()
            .filter(|(_, w)| *w == "won")
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(winners.len(), 1, "log {k}: {words:?}");
        assert!(
            words.iter().all(|(_, w)| *w == "won" || *w == "lost"),
            "log {k}: {words:?}"
        );
        let parsed = read(&open(&claim_log(dir.path(), k)));
        let claims: Vec<&Line> = parsed
            .lines
            .iter()
            .filter(|l| matches!(l.op, Op::Claim { .. }))
            .collect();
        assert_eq!(claims.len(), 1, "log {k}");
        assert_eq!(claims[0].by, agent(&format!("p{}", winners[0])));
    }
}

#[test]
fn a_racing_claim_and_withdraw_never_both_land() {
    let dir = tempfile::tempdir().unwrap();
    claim_logs(dir.path());
    let mut all = racers("claim", CLAIMERS - 1);
    all.push(("withdraw", "w".to_string()));
    let results = run_together(dir.path(), &all);

    let mut withdrawals_won = 0;
    for k in 0..CLAIM_LOGS {
        let words: Vec<&str> = results
            .iter()
            .map(|out| {
                let line = out.lines().nth(k).unwrap();
                line.strip_prefix(&format!("{k} ")).unwrap()
            })
            .collect();
        let (claimers, withdrawer) = words.split_at(CLAIMERS - 1);
        let claims_won = claimers.iter().filter(|w| **w == "won").count();
        if withdrawer[0] == "won" {
            withdrawals_won += 1;
            assert!(
                claimers.iter().all(|w| *w == "withdrawn"),
                "log {k}: {words:?}"
            );
        } else {
            assert_eq!(withdrawer[0], "lost", "log {k}: {words:?}");
            assert_eq!(claims_won, 1, "log {k}: {words:?}");
            assert!(
                claimers.iter().all(|w| *w == "won" || *w == "lost"),
                "log {k}: {words:?}"
            );
        }
        let parsed = read(&open(&claim_log(dir.path(), k)));
        let landed = parsed
            .lines
            .iter()
            .filter(|l| matches!(l.op, Op::Claim { .. } | Op::Withdraw { .. }))
            .count();
        assert_eq!(landed, 1, "log {k}");
    }
    eprintln!("withdraw won {withdrawals_won} of {CLAIM_LOGS}");
}

#[test]
fn racing_creators_leave_one_header_and_the_first_drafter() {
    race_to_create(None);
}

#[test]
fn racing_creators_over_an_empty_file_leave_one_winner() {
    race_to_create(Some(""));
}

#[test]
fn racing_creators_over_a_torn_header_leave_one_winner() {
    race_to_create(Some("{\"birta\":\"rev"));
}

#[test]
fn racing_creators_over_a_header_only_file_leave_one_winner() {
    race_to_create(Some(&format!("{HEADER}\n")));
}

/// Races 4 creators, from no file or from `start` left by a crashed creation.
fn race_to_create(start: Option<&str>) {
    let dir = tempfile::tempdir().unwrap();
    if let Some(start) = start {
        fs::write(review_path(dir.path()), start).unwrap();
    }
    let results = run_together(dir.path(), &racers("create", 4));
    let winners: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| *r == "true")
        .map(|(p, _)| p)
        .collect();
    assert_eq!(winners.len(), 1, "{start:?}: {results:?}");
    assert!(results.iter().all(|r| r == "true" || r == "false"));
    let path = review_path(dir.path());
    assert_eq!(raw_lines(&path).len(), 3, "{start:?}");
    let stray: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| {
            name != "pr.md.birta.jsonl"
                && name != "go"
                && !name.starts_with("ready-")
                && !name.starts_with("result-")
        })
        .collect();
    assert!(stray.is_empty(), "{start:?}: left behind {stray:?}");
    assert_eq!(
        read(&open(&path)).lines[1].op,
        Op::Owner {
            drafted_by: Some(format!("p{}", winners[0]))
        }
    );
}

#[test]
fn the_drafter_is_never_read_from_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let out = child("create-quietly", dir.path(), "0")
        .env("CLAUDECODE", "1")
        .env("AI_AGENT", "claude")
        .env("BIRTA_AS", "claude")
        .output()
        .unwrap();
    finish(out);
    assert_eq!(
        fs::read_to_string(dir.path().join("result-0")).unwrap(),
        "true"
    );
    assert_eq!(
        read(&open(&review_path(dir.path()))).lines[1].op,
        Op::Owner { drafted_by: None }
    );
}

// --- git exclude ---

/// A scratch home and a scratch repo, with git blind to the machine's own config.
struct Git {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Git {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir(root.join("home")).unwrap();
        Self { _tmp: tmp, root }
    }

    fn isolate(&self, cmd: &mut Command) {
        let home = self.root.join("home");
        cmd.env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", &self.root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
    }

    fn git(&self, dir: &Path, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new("git");
        cmd.current_dir(dir).args(args);
        self.isolate(&mut cmd);
        cmd.output().expect("git must be installed for these tests")
    }

    fn repo(&self) -> PathBuf {
        let repo = self.root.join("repo");
        fs::create_dir(&repo).unwrap();
        let out = self.git(&repo, &["init", "-q"]);
        assert!(out.status.success(), "{out:?}");
        repo
    }

    fn exclude(&self, repo: &Path) -> String {
        fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default()
    }

    fn ignored(&self, dir: &Path, name: &str) -> bool {
        self.git(dir, &["check-ignore", "-q", name])
            .status
            .success()
    }

    /// Creates the review file at `path` in a child that sees only this git setup.
    fn create(&self, path: &Path, doc: &str, exclude: bool) -> String {
        self.create_as(path, doc, if exclude { "on" } else { "off" }, |_| {})
    }

    /// `mode` is `on`, `off`, or `default` for a `Log` whose flag was never set.
    fn create_as(
        &self,
        path: &Path,
        doc: &str,
        mode: &str,
        tweak: impl FnOnce(&mut Command),
    ) -> String {
        let mut cmd = child("git", &self.root, "git");
        // Away from this repo, so only the review file's own folder can lead to git.
        cmd.current_dir(&self.root)
            .env("BIRTA_TEST_LOG_PATH", path)
            .env("BIRTA_TEST_LOG_DOC", doc)
            .env("BIRTA_TEST_LOG_EXCLUDE", mode);
        self.isolate(&mut cmd);
        tweak(&mut cmd);
        finish(cmd.output().unwrap());
        fs::read_to_string(self.root.join("result-git")).unwrap()
    }
}

#[test]
fn creating_a_review_file_in_a_repo_excludes_review_files_once() {
    let g = Git::new();
    let repo = g.repo();
    let before = g.exclude(&repo);
    assert!(before.is_empty() || before.ends_with('\n'));
    fs::write(repo.join("pr.md"), BASELINE).unwrap();
    assert_eq!(
        g.create(&repo.join("pr.md.birta.jsonl"), "pr.md", true),
        "created"
    );
    assert_eq!(g.exclude(&repo), format!("{before}*.birta.jsonl\n"));

    let sub = repo.join("docs/deep");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("notes.md"), BASELINE).unwrap();
    assert_eq!(
        g.create(&sub.join("notes.md.birta.jsonl"), "notes.md", true),
        "created"
    );
    assert_eq!(
        g.create(&repo.join(".birta.jsonl"), "repo", true),
        "created"
    );
    assert_eq!(g.exclude(&repo), format!("{before}*.birta.jsonl\n"));

    assert!(g.ignored(&repo, "pr.md.birta.jsonl"));
    assert!(g.ignored(&repo, "docs/deep/notes.md.birta.jsonl"));
    assert!(g.ignored(&repo, ".birta.jsonl"));
    let status = g.git(&repo, &["status", "--porcelain", "--untracked-files=all"]);
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(!status.contains("birta.jsonl"), "{status}");
    assert!(status.contains("pr.md"), "{status}");
}

#[test]
fn an_already_ignored_review_file_leaves_the_exclude_alone() {
    let g = Git::new();
    let repo = g.repo();
    fs::write(repo.join(".gitignore"), "*.jsonl\n").unwrap();
    let before = g.exclude(&repo);
    assert_eq!(
        g.create(&repo.join("pr.md.birta.jsonl"), "pr.md", true),
        "created"
    );
    assert_eq!(g.exclude(&repo), before);
}

#[test]
fn git_exclude_off_leaves_the_exclude_alone() {
    let g = Git::new();
    let repo = g.repo();
    let before = g.exclude(&repo);
    assert_eq!(
        g.create(&repo.join("pr.md.birta.jsonl"), "pr.md", false),
        "created"
    );
    assert_eq!(g.exclude(&repo), before);
    assert!(!g.ignored(&repo, "pr.md.birta.jsonl"));
}

#[test]
fn an_existing_review_file_does_not_touch_the_exclude() {
    let g = Git::new();
    let repo = g.repo();
    let path = repo.join("pr.md.birta.jsonl");
    assert_eq!(g.create(&path, "pr.md", true), "created");
    fs::write(repo.join(".git/info/exclude"), "# mine\n").unwrap();
    assert_eq!(g.create(&path, "pr.md", true), "exists");
    assert_eq!(g.exclude(&repo), "# mine\n");
}

#[test]
fn the_exclude_pattern_always_gets_its_own_line() {
    let g = Git::new();
    let repo = g.repo();
    fs::write(repo.join(".git/info/exclude"), "*.log").unwrap();
    assert_eq!(
        g.create(&repo.join("pr.md.birta.jsonl"), "pr.md", true),
        "created"
    );
    assert_eq!(g.exclude(&repo), "*.log\n*.birta.jsonl\n");
}

#[test]
fn a_missing_exclude_file_is_created() {
    let g = Git::new();
    let repo = g.repo();
    fs::remove_dir_all(repo.join(".git/info")).unwrap();
    assert_eq!(
        g.create(&repo.join("pr.md.birta.jsonl"), "pr.md", true),
        "created"
    );
    assert_eq!(g.exclude(&repo), "*.birta.jsonl\n");
}

#[test]
fn git_exclude_is_on_by_default() {
    let g = Git::new();
    let repo = g.repo();
    let before = g.exclude(&repo);
    assert_eq!(
        g.create_as(&repo.join("pr.md.birta.jsonl"), "pr.md", "default", |_| {}),
        "created"
    );
    assert_eq!(g.exclude(&repo), format!("{before}*.birta.jsonl\n"));
}

#[test]
fn without_git_on_the_path_the_review_file_is_still_created() {
    let g = Git::new();
    let repo = g.repo();
    let before = g.exclude(&repo);
    let empty = g.root.join("empty-bin");
    fs::create_dir(&empty).unwrap();
    let result = g.create_as(&repo.join("pr.md.birta.jsonl"), "pr.md", "on", |cmd| {
        cmd.env("PATH", &empty);
    });
    assert_eq!(result, "created");
    assert!(repo.join("pr.md.birta.jsonl").exists());
    assert_eq!(g.exclude(&repo), before);
}

#[cfg(unix)]
#[test]
fn an_exclude_that_cannot_be_written_is_skipped_silently() {
    use std::os::unix::fs::PermissionsExt;

    let g = Git::new();
    let repo = g.repo();
    let exclude = repo.join(".git/info/exclude");
    let before = g.exclude(&repo);
    fs::set_permissions(&exclude, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(repo.join(".git/info"), fs::Permissions::from_mode(0o555)).unwrap();
    let result = g.create(&repo.join("pr.md.birta.jsonl"), "pr.md", true);
    fs::set_permissions(repo.join(".git/info"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&exclude, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(result, "created");
    assert!(repo.join("pr.md.birta.jsonl").exists());
    assert_eq!(g.exclude(&repo), before);
}

#[test]
fn a_linked_worktree_excludes_its_review_files() {
    let g = Git::new();
    let repo = g.repo();
    fs::write(repo.join("pr.md"), BASELINE).unwrap();
    for args in [
        &["add", "pr.md"][..],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "doc",
        ],
        &["worktree", "add", "-q", "../wt"],
    ] {
        let out = g.git(&repo, args);
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
    let wt = g.root.join("wt");
    assert_eq!(
        g.create(&wt.join("pr.md.birta.jsonl"), "pr.md", true),
        "created"
    );
    assert!(g.ignored(&wt, "pr.md.birta.jsonl"));
    let status = g.git(&wt, &["status", "--porcelain", "--untracked-files=all"]);
    assert_eq!(String::from_utf8(status.stdout).unwrap(), "");
}

#[test]
fn racing_creators_in_one_repo_add_the_pattern_once() {
    let g = Git::new();
    let repo = g.repo();
    let before = g.exclude(&repo);
    let results = run_together_with(&g.root, &racers("git-race", 4), |cmd| {
        cmd.current_dir(&g.root);
        g.isolate(cmd);
    });
    assert!(results.iter().all(|r| r == "true"), "{results:?}");
    assert_eq!(g.exclude(&repo), format!("{before}*.birta.jsonl\n"));
}

#[test]
fn outside_git_the_review_file_is_created_and_nothing_else() {
    let g = Git::new();
    let plain = g.root.join("plain");
    fs::create_dir(&plain).unwrap();
    assert_eq!(
        g.create(&plain.join("pr.md.birta.jsonl"), "pr.md", true),
        "created"
    );
    let names: HashSet<_> = fs::read_dir(&plain)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, HashSet::from(["pr.md.birta.jsonl".to_string()]));
}

#[cfg(unix)]
#[test]
fn an_unwritable_folder_gets_no_review_file_and_no_exclude() {
    use std::os::unix::fs::PermissionsExt;

    let g = Git::new();
    let repo = g.repo();
    let locked = repo.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("pr.md"), BASELINE).unwrap();
    let before = g.exclude(&repo);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let result = g.create(&locked.join("pr.md.birta.jsonl"), "pr.md", true);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(result, "error PermissionDenied");
    assert!(!locked.join("pr.md.birta.jsonl").exists());
    assert_eq!(g.exclude(&repo), before);
}
