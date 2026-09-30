//! Whether `birta review` picks up depends on stdout being a terminal, not stdin.
#![cfg(unix)]

use std::fs;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, Scenario};

fn pty() -> (OwnedFd, OwnedFd) {
    let (mut leader, mut follower) = (0, 0);
    // SAFETY: out-pointers to two ints; the optional name, termios and winsize are null.
    let rc = unsafe {
        libc::openpty(
            &mut leader,
            &mut follower,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: openpty returned two fresh fds that nothing else owns.
    unsafe { (OwnedFd::from_raw_fd(leader), OwnedFd::from_raw_fd(follower)) }
}

/// Runs `birta review pr.md` with no identity. Returns the claim count and the tty's text.
fn review(stdout_is_tty: bool) -> (usize, String) {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let mut s = Scenario::new(DOC);
    s.note("TTY-NOTE");
    s.submit();
    fs::write(base.join("pr.md"), s.text()).unwrap();
    fs::write(base.join("pr.md.birta.jsonl"), s.log()).unwrap();
    let (leader, follower) = pty();
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("birta"));
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &base)
        .env("TZ", "UTC")
        .env("BIRTA_REVIEW", "1")
        .args(["review", "pr.md"])
        .current_dir(&base)
        .stderr(Stdio::null());
    if stdout_is_tty {
        cmd.stdin(Stdio::null()).stdout(Stdio::from(follower));
    } else {
        cmd.stdin(Stdio::from(follower)).stdout(Stdio::null());
    }
    let mut child = cmd.spawn().unwrap();
    drop(cmd);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = fs::File::from(leader).read_to_end(&mut text);
        let _ = tx.send(text);
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("birta review did not finish");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let log = fs::read_to_string(base.join("pr.md.birta.jsonl")).unwrap();
    let claims = log
        .lines()
        .filter(|l| l.contains(r#""op":"claim""#))
        .count();
    (claims, String::from_utf8_lossy(&text).into_owned())
}

#[test]
fn a_terminal_on_stdout_peeks_and_a_terminal_on_stdin_does_not() {
    let (claims, text) = review(true);
    assert_eq!(
        claims, 0,
        "a person at a terminal must not pick up:\n{text}"
    );
    assert!(text.contains("TTY-NOTE"), "{text}");
    assert!(!text.contains('\x1b'), "plain text on a tty too: {text:?}");
    let (claims, _) = review(false);
    assert_eq!(claims, 1, "stdout is a pipe, so this is an agent shell");
}

#[test]
fn respond_refuses_a_terminal_on_stdin() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let mut s = Scenario::new(DOC);
    s.note("TTY-NOTE");
    s.submit();
    s.claim("claude");
    fs::write(base.join("pr.md"), s.text()).unwrap();
    fs::write(base.join("pr.md.birta.jsonl"), s.log()).unwrap();
    let (leader, follower) = pty();
    let mut child = Command::new(assert_cmd::cargo::cargo_bin("birta"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &base)
        .env("TZ", "UTC")
        .env("BIRTA_REVIEW", "1")
        .args(["respond", "pr.md", "--as", "claude"])
        .current_dir(&base)
        .stdin(Stdio::from(follower))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Held open and never written, so a read of the terminal blocks.
    let _leader = leader;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("respond read the terminal instead of refusing it");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert_eq!(status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("respond reads stdin"), "{stderr}");
    assert_eq!(
        fs::read_to_string(base.join("pr.md.birta.jsonl")).unwrap(),
        s.log()
    );
}

#[test]
fn comment_without_a_body_refuses_a_terminal_on_stdin() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    fs::write(base.join("pr.md"), DOC).unwrap();
    let (leader, follower) = pty();
    let mut child = Command::new(assert_cmd::cargo::cargo_bin("birta"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &base)
        .env("BIRTA_REVIEW", "1")
        .args(["comment", "pr.md", "--doc", "--as", "claude"])
        .current_dir(&base)
        .stdin(Stdio::from(follower))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Held open and never written, so a read of the terminal blocks.
    let _leader = leader;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("comment read the terminal instead of refusing it");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!stderr.contains("not available in this build"), "{stderr}");
    assert_eq!(status.code(), Some(2), "{stderr}");
    assert!(!base.join("pr.md.birta.jsonl").exists());
}
