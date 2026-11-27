use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn nonexistent_file_exits_with_error() {
    Command::cargo_bin("birta")
        .unwrap()
        .arg("nonexistent.md")
        .assert()
        .failure()
        .stderr(predicate::str::contains("file not found"));
}

#[test]
fn help_flag_exits_successfully() {
    Command::cargo_bin("birta")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
}

#[test]
fn version_flag_shows_version() {
    Command::cargo_bin("birta")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("birta"));
}

#[test]
fn diff_needs_two_files() {
    Command::cargo_bin("birta")
        .unwrap()
        .args(["--diff", "a.md"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("2 values required"));
}

#[test]
fn diff_reports_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join("b.md");
    std::fs::write(&new, "# B\n").unwrap();
    Command::cargo_bin("birta")
        .unwrap()
        .arg("--diff")
        .arg(dir.path().join("missing.md"))
        .arg(&new)
        .assert()
        .failure()
        .stderr(predicate::str::contains("file not found"));
}

#[test]
fn diff_rejects_a_third_file() {
    Command::cargo_bin("birta")
        .unwrap()
        .args(["--diff", "a.md", "b.md", "file.md"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn diff_layout_requires_diff() {
    Command::cargo_bin("birta")
        .unwrap()
        .args(["--diff-layout", "split", "file.md"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--diff-layout needs --diff"));
}

#[test]
fn diff_rejects_a_second_diff_flag() {
    Command::cargo_bin("birta")
        .unwrap()
        .args(["--diff", "a.md", "b.md", "--diff", "c.md", "d.md"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used multiple times"));
}

#[test]
fn diff_reports_an_unreadable_side_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let (bad, good) = (dir.path().join("bad.md"), dir.path().join("good.md"));
    std::fs::write(&bad, b"\xff\xfe not utf-8").unwrap();
    std::fs::write(&good, "# Good\n").unwrap();
    Command::cargo_bin("birta")
        .unwrap()
        .args(["--no-open", "--diff"])
        .arg(&bad)
        .arg(&good)
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not read").and(predicate::str::contains("bad.md")))
        .stderr(predicate::str::contains("serving").not());
}
