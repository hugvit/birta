//! `birta show`: a doc's exact bytes, now or at a rev its review file recorded. Every
//! process runs with a cleared env, `HOME` in a temp folder and the gate on.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;

use assert_cmd::Command;
use birta::rev::Rev;
use birta::review::ops::{self, Op};
use serde_json::json;

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, Scenario, ts};

const LOG: &str = "pr.md.birta.jsonl";

/// BOM, CRLF and no final newline: nothing about it may be normalised.
const ODD: &str = "\u{FEFF}# Plan\r\n\r\nIntro paragraph here,\r\nwith a second line.";

/// A temp folder holding `home/`, a doc and, when given, its review file.
struct Tmp {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Tmp {
    fn new(doc: &[u8], log: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        fs::create_dir(base.join("home")).unwrap();
        fs::write(base.join("pr.md"), doc).unwrap();
        if let Some(log) = log {
            fs::write(base.join(LOG), log).unwrap();
        }
        Self { _tmp: tmp, base }
    }

    /// `pr.md` holding `disk`, with the review file `s` wrote.
    fn with(s: &Scenario, disk: &str) -> Self {
        Self::new(disk.as_bytes(), Some(&s.log()))
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn log_bytes(&self) -> Option<Vec<u8>> {
        fs::read(self.path(LOG)).ok()
    }

    /// `birta show target`: exit code, stdout's bytes and stderr.
    fn show(&self, target: &str) -> (i32, Vec<u8>, String) {
        let out = Command::cargo_bin("birta")
            .unwrap()
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path("home"))
            .env("TZ", "UTC")
            .env("BIRTA_REVIEW", "1")
            .args(["show", target])
            .current_dir(&self.base)
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        let code = out
            .status
            .code()
            .expect("birta was killed by the test timeout");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !stderr.contains("not available in this build"),
            "birta show {target} is still the stub: {stderr}"
        );
        (code, out.stdout, stderr)
    }

    /// A show that must print exactly `want`, with exit 0.
    fn prints(&self, target: &str, want: &[u8]) -> String {
        let (code, stdout, stderr) = self.show(target);
        assert_eq!(code, 0, "{target}: {stderr}");
        assert_eq!(
            stdout,
            want,
            "{target}: {:?}",
            String::from_utf8_lossy(&stdout)
        );
        stderr
    }

    /// A show that must fail with `code`, printing nothing on stdout.
    fn fails(&self, target: &str, code: i32) -> String {
        let (got, stdout, stderr) = self.show(target);
        assert_eq!(got, code, "{target}: {stderr}");
        assert!(stdout.is_empty(), "{target} printed {stdout:?}");
        stderr
    }
}

fn rev(text: &str) -> String {
    Rev::of(text.as_bytes()).to_string()
}

fn short(text: &str) -> String {
    rev(text)[..8].to_string()
}

/// `DOC`, then `ODD` by a patch, then `DOC` plus a line: `ODD` is neither the snap nor
/// the head, so only replaying the patch rebuilds it.
fn odd_between() -> (Scenario, String) {
    let mut s = Scenario::new(DOC);
    let odd = s.external(ODD);
    let last = format!("{DOC}\nlast line\n");
    s.external(&last);
    let parsed = ops::parse(s.log());
    let line = parsed.lines.iter().find(|l| l.n == odd).unwrap();
    assert!(
        matches!(&line.op, Op::External { patch: Some(_), .. }),
        "ODD must be reached by a patch: {:?}",
        line.op
    );
    assert!(
        parsed
            .lines
            .iter()
            .all(|l| !matches!(&l.op, Op::Snap { text, .. } if text == ODD)),
        "no snap may hold ODD"
    );
    (s, last)
}

// --- bytes ---

#[test]
fn a_rev_rebuilt_from_patches_prints_byte_for_byte() {
    let (s, last) = odd_between();
    let tmp = Tmp::with(&s, &last);
    // The 8 hex digits `review`'s approved `next:` line names
    tmp.prints(&format!("pr.md@{}", short(ODD)), ODD.as_bytes());
    tmp.prints(&format!("pr.md@{}", rev(ODD)), ODD.as_bytes());
    tmp.prints(&format!("pr.md@{}", &rev(ODD)[..6]), ODD.as_bytes());
    tmp.prints(&format!("pr.md@{}", short(DOC)), DOC.as_bytes());
    tmp.prints(&format!("pr.md@{}", short(&last)), last.as_bytes());
}

#[test]
fn no_rev_prints_the_bytes_on_disk_exactly() {
    // Not UTF-8, so nothing may decode it on the way out
    let latin1: &[u8] = b"# caf\xe9\r\nno final newline";
    for bytes in [ODD.as_bytes(), latin1, b""] {
        let tmp = Tmp::new(bytes, None);
        tmp.prints("pr.md", bytes);
        assert!(tmp.log_bytes().is_none(), "show made a review file");
    }
    let (s, last) = odd_between();
    let tmp = Tmp::with(&s, ODD);
    tmp.prints("pr.md", ODD.as_bytes());
    // An unrecorded write shows too: the disk, not the log
    let tmp = Tmp::with(&s, &format!("{last}unrecorded\n"));
    tmp.prints("pr.md", format!("{last}unrecorded\n").as_bytes());
}

// --- which rev ---

#[test]
fn an_unknown_rev_exits_1_listing_the_last_five() {
    let mut s = Scenario::new(DOC);
    let mut texts = vec![DOC.to_string()];
    for i in 1..=6 {
        let text = format!("{DOC}\nversion {i}\n");
        s.external(&text);
        texts.push(text);
    }
    let tmp = Tmp::with(&s, texts.last().unwrap());
    let unknown = "0000000";
    assert!(texts.iter().all(|t| !rev(t).starts_with(unknown)));
    let stderr = tmp.fails(&format!("pr.md@{unknown}"), 1);
    for text in &texts[2..] {
        assert!(stderr.contains(&short(text)), "{} in {stderr}", short(text));
    }
    for text in &texts[..2] {
        assert!(
            !stderr.contains(&short(text)),
            "only the last 5: {} in {stderr}",
            short(text)
        );
    }
}

/// Two texts whose revs share their first 6 hex digits, and the digits they share.
fn twins() -> (String, String, usize) {
    let mut seen: HashMap<String, String> = HashMap::new();
    for i in 0..1_000_000 {
        let text = format!("# v{i}\n");
        let r = rev(&text);
        if let Some(other) = seen.insert(r[..6].to_string(), text.clone()) {
            let a = rev(&other);
            let common = a.bytes().zip(r.bytes()).take_while(|(x, y)| x == y).count();
            return (other, text, common);
        }
    }
    panic!("no two revs share 6 hex digits");
}

#[test]
fn an_ambiguous_prefix_exits_65() {
    let (a, b, common) = twins();
    let mut s = Scenario::new(&a);
    s.external(&b);
    let tmp = Tmp::with(&s, &b);
    let stderr = tmp.fails(&format!("pr.md@{}", &rev(&a)[..6]), 65);
    assert!(stderr.contains(&short(&a)), "{stderr}");
    assert!(stderr.contains(&short(&b)), "{stderr}");
    // One digit past what they share tells them apart
    tmp.prints(&format!("pr.md@{}", &rev(&a)[..=common]), a.as_bytes());
    tmp.prints(&format!("pr.md@{}", &rev(&b)[..=common]), b.as_bytes());
}

#[test]
fn a_prefix_under_six_hex_digits_is_refused() {
    let (s, last) = odd_between();
    let tmp = Tmp::with(&s, &last);
    for n in [1, 5] {
        let (code, stdout, stderr) = tmp.show(&format!("pr.md@{}", &rev(ODD)[..n]));
        assert_ne!(code, 0, "{n} digits: {stderr}");
        assert!(stdout.is_empty());
    }
}

#[test]
fn approved_prints_the_approved_bytes_even_after_the_doc_changed() {
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    s.approve();
    let last = format!("{DOC}\nafter approval\n");
    s.external(&last);
    let tmp = Tmp::with(&s, &last);
    let stderr = tmp.prints("pr.md@approved", ODD.as_bytes());
    assert!(stderr.contains("changed"), "warned on stderr: {stderr:?}");

    let mut s = Scenario::new(DOC);
    s.external(ODD);
    s.approve();
    let tmp = Tmp::with(&s, ODD);
    let stderr = tmp.prints("pr.md@approved", ODD.as_bytes());
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn approved_with_no_standing_approval_exits_1() {
    let mut s = Scenario::new(DOC);
    s.note("a note");
    s.submit();
    Tmp::with(&s, DOC).fails("pr.md@approved", 1);

    let approve = s.approve();
    s.withdraw(approve);
    Tmp::with(&s, DOC).fails("pr.md@approved", 1);

    Tmp::new(DOC.as_bytes(), None).fails("pr.md@approved", 1);
}

#[test]
fn latest_is_the_last_recorded_text() {
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    let tmp = Tmp::with(&s, ODD);
    tmp.prints("pr.md@latest", ODD.as_bytes());
    // A write nobody recorded yet isn't in the log
    let tmp = Tmp::with(&s, "# unrecorded\n");
    tmp.prints("pr.md@latest", ODD.as_bytes());

    Tmp::new(DOC.as_bytes(), None).fails("pr.md@latest", 1);
}

// --- read-only ---

#[test]
fn show_never_writes_the_review_file() {
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    s.approve();
    let tmp = Tmp::with(&s, "# unrecorded\n");
    let before = tmp.log_bytes();
    let targets = [
        "pr.md".to_string(),
        "pr.md@latest".to_string(),
        "pr.md@approved".to_string(),
        format!("pr.md@{}", short(DOC)),
        "pr.md@0000000".to_string(),
    ];
    for target in &targets {
        let _ = tmp.show(target);
        assert_eq!(tmp.log_bytes(), before, "{target} wrote the review file");
    }
    let bare = Tmp::new(DOC.as_bytes(), None);
    for target in &targets {
        let _ = bare.show(target);
        assert!(bare.log_bytes().is_none(), "{target} made a review file");
    }
}

// --- targets ---

#[test]
fn an_at_sign_in_the_file_name_is_part_of_the_name() {
    let tmp = Tmp::new(b"", None);
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    fs::write(tmp.path("notes@home.md"), ODD).unwrap();
    fs::write(tmp.path("notes@home.md.birta.jsonl"), s.log()).unwrap();
    tmp.prints("notes@home.md", ODD.as_bytes());
    tmp.prints(&format!("notes@home.md@{}", short(DOC)), DOC.as_bytes());
    tmp.prints("notes@home.md@latest", ODD.as_bytes());
}

#[test]
fn a_missing_file_exits_1() {
    let tmp = Tmp::new(DOC.as_bytes(), None);
    let stderr = tmp.fails("gone.md", 1);
    assert!(stderr.contains("gone.md"), "{stderr}");
    tmp.fails("gone.md@latest", 1);
}

#[test]
fn the_approved_next_line_from_review_runs_as_is() {
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    s.approve();
    let tmp = Tmp::with(&s, ODD);
    let out = Command::cargo_bin("birta")
        .unwrap()
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.path("home"))
        .env("TZ", "UTC")
        .env("BIRTA_REVIEW", "1")
        .args(["review", "pr.md", "--as", "claude"])
        .current_dir(&tmp.base)
        .timeout(Duration::from_secs(20))
        .output()
        .unwrap();
    let view = String::from_utf8(out.stdout).unwrap();
    let target = view
        .split_whitespace()
        .skip_while(|w| *w != "show")
        .nth(1)
        .unwrap_or_else(|| panic!("a `birta show` in the view: {view}"));
    tmp.prints(target, ODD.as_bytes());
}

// --- rulings ---

#[test]
fn approved_is_the_latest_approval_not_withdrawn() {
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    s.approve();
    let last = format!("{DOC}\nsecond approval\n");
    s.external(&last);
    let second = s.approve();
    s.withdraw(second);
    let tmp = Tmp::with(&s, &last);
    tmp.prints("pr.md@approved", ODD.as_bytes());

    let mut s = Scenario::new(DOC);
    s.approve();
    s.external(ODD);
    s.approve();
    let tmp = Tmp::with(&s, ODD);
    tmp.prints("pr.md@approved", ODD.as_bytes());
}

#[test]
fn a_prefix_of_the_disk_rev_prints_the_disk_when_the_log_lacks_it() {
    let tmp = Tmp::new(ODD.as_bytes(), None);
    tmp.prints(&format!("pr.md@{}", short(ODD)), ODD.as_bytes());
    tmp.fails(&format!("pr.md@{}", short(DOC)), 1);

    let (s, _) = odd_between();
    let unrecorded = "# unrecorded\n";
    let tmp = Tmp::with(&s, unrecorded);
    tmp.prints(
        &format!("pr.md@{}", short(unrecorded)),
        unrecorded.as_bytes(),
    );
    tmp.prints(&format!("pr.md@{}", short(ODD)), ODD.as_bytes());
}

#[test]
fn a_gone_doc_still_shows_what_its_log_recorded() {
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    let tmp = Tmp::new(b"", None);
    fs::write(tmp.path("gone.md.birta.jsonl"), s.log()).unwrap();
    tmp.prints("gone.md@latest", ODD.as_bytes());
    tmp.prints(&format!("gone.md@{}", short(DOC)), DOC.as_bytes());
    tmp.fails("gone.md", 1);
}

#[test]
fn a_whole_argument_naming_a_file_is_that_file() {
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    let tmp = Tmp::with(&s, ODD);
    fs::write(tmp.path("pr.md@latest"), "# a file with an at\n").unwrap();
    tmp.prints("pr.md@latest", b"# a file with an at\n");
}

#[test]
fn a_rev_recorded_without_its_text_is_not_recoverable() {
    let mut s = Scenario::new(DOC);
    let lost = "# lost\n";
    let n = s.next();
    s.raw(json!({
        "op": "external", "rev": rev(DOC), "to_rev": rev(lost), "patch": null,
        "by": {"type": "birta"}, "ts": ts(n),
    }));
    assert!(ops::parse(s.log()).bad.is_empty(), "the line must parse");
    let tmp = Tmp::with(&s, DOC);
    let stderr = tmp.fails(&format!("pr.md@{}", short(lost)), 1);
    assert!(stderr.contains("is not recoverable"), "{stderr}");
    assert!(stderr.contains(&short(lost)), "{stderr}");
}

// --- adversary ---

#[test]
fn a_rev_the_doc_returned_to_is_one_rev_not_two() {
    let mut s = Scenario::new(DOC);
    s.external(ODD);
    s.external(DOC);
    let tmp = Tmp::with(&s, DOC);
    tmp.prints(&format!("pr.md@{}", &rev(DOC)[..6]), DOC.as_bytes());
    tmp.prints(&format!("pr.md@{}", short(ODD)), ODD.as_bytes());
}

#[test]
fn a_reader_that_goes_away_is_not_a_crash() {
    // Far past a pipe's buffer, so the write meets the closed end
    let big = "line of text\n".repeat(400_000);
    let mut s = Scenario::new(DOC);
    s.external(&big);
    let tmp = Tmp::with(&s, &big);
    for target in ["pr.md".to_string(), format!("pr.md@{}", short(&big))] {
        let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("birta"))
            .args(["show", &target])
            .current_dir(&tmp.base)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", tmp.path("home"))
            .env("BIRTA_REVIEW", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("not available in this build"), "{stderr}");
        assert!(!stderr.contains("panicked"), "{target}: {stderr}");
        assert_ne!(out.status.code(), Some(101), "{target}: {stderr}");
    }
}

#[test]
fn a_patch_that_does_not_give_its_rev_prints_nothing() {
    let mut s = Scenario::new(DOC);
    let forged = "# forged\n";
    let n = s.next();
    s.raw(json!({
        "op": "external", "rev": rev(DOC), "to_rev": rev(forged),
        "patch": [{"a": 0, "d": ["# Plan\n"], "i": ["# Other\n"]}],
        "by": {"type": "birta"}, "ts": ts(n),
    }));
    // A Bad line never enters the chain, which would make this pass for nothing
    assert!(
        ops::parse(s.log()).bad.is_empty(),
        "the forged line must parse"
    );
    let tmp = Tmp::with(&s, DOC);
    tmp.fails(&format!("pr.md@{}", short(forged)), 1);
}

#[test]
fn a_rev_held_only_by_a_checkpoint_prints() {
    let mut s = Scenario::new(DOC);
    s.external_rev_only(ODD);
    s.approve();
    let tmp = Tmp::with(&s, ODD);
    tmp.prints("pr.md@approved", ODD.as_bytes());
    tmp.prints(&format!("pr.md@{}", short(ODD)), ODD.as_bytes());
    tmp.prints("pr.md@latest", ODD.as_bytes());
}

#[test]
fn a_target_with_an_empty_or_non_hex_rev_is_refused() {
    let (s, last) = odd_between();
    let tmp = Tmp::with(&s, &last);
    for target in [
        "pr.md@",
        "pr.md@zzzzzzzz",
        "pr.md@approve",
        "pr.md@-12345678",
    ] {
        let (code, stdout, stderr) = tmp.show(target);
        assert_ne!(code, 0, "{target}: {stderr}");
        assert!(stdout.is_empty(), "{target} printed {stdout:?}");
    }
}

#[test]
fn a_rev_prefix_is_read_in_either_case() {
    let (s, last) = odd_between();
    let tmp = Tmp::with(&s, &last);
    tmp.prints(
        &format!("pr.md@{}", short(ODD).to_uppercase()),
        ODD.as_bytes(),
    );
}
