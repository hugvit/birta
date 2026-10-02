use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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

/// A temp folder holding `home/` (so no user config is read) and the docs under test.
struct Tmp {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Tmp {
    fn new(files: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        std::fs::create_dir(base.join("home")).unwrap();
        let t = Self { _tmp: tmp, base };
        for rel in files {
            t.write(rel, &format!("# {rel}\n"));
        }
        t
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.base.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let path = self.base.join(rel);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn birta(&self) -> Command {
        let mut cmd = Command::cargo_bin("birta").unwrap();
        cmd.env("HOME", self.base.join("home")).arg("--no-open");
        cmd
    }
}

/// A served `birta` process, killed on drop so a failing test leaves nothing running.
struct Served {
    child: Child,
    port: u16,
    /// Stderr lines up to and including the `serving` line.
    stderr: Vec<String>,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve(tmp: &Tmp, cwd: &Path, target: &str) -> Served {
    spawn(tmp, cwd, target, None)
}

/// Starts `birta target`, feeding `stdin` when given, and waits for its `serving` line.
fn spawn(tmp: &Tmp, cwd: &Path, target: &str, stdin: Option<&str>) -> Served {
    let child = StdCommand::new(assert_cmd::cargo::cargo_bin("birta"))
        .args(["--no-open", "--port", "0", target])
        .current_dir(cwd)
        .env("HOME", tmp.base.join("home"))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Owned by the guard at once, so every panic below still reaps the process.
    let mut served = Served {
        child,
        port: 0,
        stderr: Vec::new(),
    };
    if let Some(text) = stdin {
        // Dropping the pipe closes it, so birta sees EOF.
        let mut pipe = served.child.stdin.take().unwrap();
        pipe.write_all(text.as_bytes()).unwrap();
    }
    let (tx, rx) = mpsc::channel();
    let stderr = served.child.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if tx.send(line.unwrap_or_default()).is_err() {
                break;
            }
        }
    });

    loop {
        let Ok(line) = rx.recv_timeout(Duration::from_secs(20)) else {
            panic!(
                "no `serving` line from birta {target}; stderr: {:#?}",
                served.stderr
            );
        };
        served.stderr.push(line.clone());
        if let Some(addr) = line.split("at http://").nth(1) {
            served.port = addr.rsplit(':').next().unwrap().trim().parse().unwrap();
            return served;
        }
    }
}

type Reply = Result<(u16, Option<String>), String>;

/// Status, `Location` and body of `GET path`, or the error text when nothing answers.
fn fetch(port: u16, path: &str) -> Result<(u16, Option<String>, String), String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nconnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| format!("bad response: {response:?}"))?;
    let location = head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case("location")
            .then(|| value.trim().to_string())
    });
    Ok((status, location, body.to_string()))
}

/// Status and `Location` of `GET path`.
fn get(port: u16, path: &str) -> Reply {
    fetch(port, path).map(|(status, location, _)| (status, location))
}

fn redirect(to: &str) -> Reply {
    Ok((307, Some(to.to_string())))
}

fn status(port: u16, path: &str) -> Result<u16, String> {
    get(port, path).map(|r| r.0)
}

#[test]
fn a_directory_redirects_to_its_nested_entry() {
    let tmp = Tmp::new(&["site/notes/a.md"]);
    let served = serve(&tmp, &tmp.base, "site");

    assert_eq!(get(served.port, "/"), redirect("/view/notes/a.md"));
    assert_eq!(status(served.port, "/view/notes/a.md"), Ok(200));
}

#[test]
fn a_directory_prefers_index_over_readme() {
    let tmp = Tmp::new(&["site/README.md", "site/index.md", "site/a.md"]);
    let served = serve(&tmp, &tmp.base, "site");

    assert_eq!(get(served.port, "/"), redirect("/view/index.md"));
}

#[test]
fn the_current_directory_can_be_served() {
    let tmp = Tmp::new(&["site/guide.md"]);
    let served = serve(&tmp, &tmp.base.join("site"), ".");

    assert_eq!(get(served.port, "/"), redirect("/view/guide.md"));
    assert_eq!(status(served.port, "/view/guide.md"), Ok(200));
}

#[test]
fn a_parent_directory_can_be_served() {
    let tmp = Tmp::new(&["site/guide.md"]);
    let served = serve(&tmp, &tmp.dir("site/sub"), "..");

    assert_eq!(get(served.port, "/"), redirect("/view/guide.md"));
    assert_eq!(status(served.port, "/view/guide.md"), Ok(200));
}

#[test]
fn a_linked_directory_is_served() {
    let tmp = Tmp::new(&["real/guide.md"]);
    symlink("real", tmp.base.join("site")).unwrap();
    let served = serve(&tmp, &tmp.base, "site");

    assert_eq!(get(served.port, "/"), redirect("/view/guide.md"));
    assert_eq!(status(served.port, "/view/guide.md"), Ok(200));
}

#[test]
fn a_directory_gets_no_extension_warning() {
    let tmp = Tmp::new(&["site.v2/index.md"]);
    let served = serve(&tmp, &tmp.base, "site.v2");

    assert!(
        !served.stderr.iter().any(|l| l.contains("extension")),
        "{:#?}",
        served.stderr
    );
    assert_eq!(get(served.port, "/"), redirect("/view/index.md"));
}

#[test]
fn a_non_markdown_file_still_warns() {
    let tmp = Tmp::new(&["notes.txt"]);
    let served = serve(&tmp, &tmp.base, "notes.txt");

    assert!(
        served
            .stderr
            .iter()
            .any(|l| l.contains("does not have a .md or .markdown extension")),
        "{:#?}",
        served.stderr
    );
}

#[test]
fn a_file_arg_opens_that_file() {
    let tmp = Tmp::new(&["site/index.md", "site/b.md"]);
    let served = serve(&tmp, &tmp.base, "site/b.md");

    assert_eq!(get(served.port, "/"), redirect("/view/b.md"));
}

#[test]
fn stdin_still_serves() {
    let tmp = Tmp::new(&[]);
    let served = spawn(&tmp, &tmp.base, "-", Some("# hi from stdin\n"));

    let (status, _, body) = fetch(served.port, "/").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("hi from stdin"), "{body}");
}

#[test]
fn a_directory_without_docs_exits_1_with_a_message() {
    let tmp = Tmp::new(&["empty/notes.txt", "empty/node_modules/x.md"]);
    let dir = tmp.dir("empty");

    for args in [vec![], vec!["--static", "--out"]] {
        let mut cmd = tmp.birta();
        cmd.args(&args);
        if !args.is_empty() {
            cmd.arg(tmp.base.join("out"));
        }
        cmd.arg(&dir)
            .timeout(Duration::from_secs(20))
            .assert()
            .code(1)
            .stderr(predicate::str::contains(format!(
                "no markdown files under {}",
                dir.display()
            )));
    }
    assert!(!tmp.base.join("out").join("index.html").exists());

    tmp.birta()
        .current_dir(&tmp.base)
        .arg("empty")
        .timeout(Duration::from_secs(20))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no markdown files under empty\n"))
        .stderr(predicate::str::contains("serving").not());
}

#[test]
fn static_directory_writes_the_entry_page() {
    let tmp = Tmp::new(&[]);
    tmp.write("site/a/entry.md", "# Entry\n\n[other](../b/other.md)\n");
    tmp.write("site/b/other.md", "# Other\n");
    let out = tmp.base.join("out");

    tmp.birta()
        .args(["--static", "--out"])
        .arg(&out)
        .arg(tmp.base.join("site"))
        .timeout(Duration::from_secs(20))
        .assert()
        .success()
        .stderr(predicate::str::contains("wrote bundle"));

    let entry = std::fs::read_to_string(out.join("a/entry.html")).expect("entry page");
    assert!(entry.contains("href=\"../b/other.html\""), "{entry}");
    assert!(
        out.join("b/other.html").exists(),
        "the folder, not the entry's parent, bounds the crawl"
    );
}

#[test]
fn static_directory_prefers_index_over_readme() {
    let tmp = Tmp::new(&["site/README.md", "site/index.md"]);
    let out = tmp.base.join("out");

    tmp.birta()
        .args(["--static", "--out"])
        .arg(&out)
        .arg(tmp.base.join("site"))
        .timeout(Duration::from_secs(20))
        .assert()
        .success();

    assert!(out.join("index.html").exists());
    assert!(
        !out.join("README.html").exists(),
        "only the entry's link graph"
    );
}

#[test]
fn static_directory_with_a_linked_entry() {
    let tmp = Tmp::new(&["site/b.md"]);
    tmp.write("outside/doc.md", "# Linked\n\n[b](b.md)\n");
    symlink("../outside/doc.md", tmp.base.join("site/index.md")).unwrap();
    let out = tmp.base.join("out");

    tmp.birta()
        .args(["--static", "--out"])
        .arg(&out)
        .arg(tmp.base.join("site"))
        .timeout(Duration::from_secs(20))
        .assert()
        .success();

    let entry = std::fs::read_to_string(out.join("index.html")).expect("entry page");
    assert!(entry.contains("Linked"), "{entry}");
    assert!(
        out.join("b.html").exists(),
        "links resolve from the entry's place"
    );
}

#[test]
fn static_directory_with_an_entry_in_a_linked_folder() {
    let tmp = Tmp::new(&[]);
    tmp.write("outside/doc.md", "# Linked folder\n");
    std::fs::create_dir(tmp.base.join("site")).unwrap();
    symlink("../outside", tmp.base.join("site/docs")).unwrap();
    let out = tmp.base.join("out");

    tmp.birta()
        .args(["--static", "--out"])
        .arg(&out)
        .arg(tmp.base.join("site"))
        .timeout(Duration::from_secs(20))
        .assert()
        .success();

    let entry = std::fs::read_to_string(out.join("docs/doc.html")).expect("entry page");
    assert!(entry.contains("Linked folder"), "{entry}");
}

// --- keybindings: config and --bind, and what happens when two actions want one key ---

/// Runs `birta --static` on a doc with `config` as the user's config file and `binds` as
/// `--bind` flags. Returns the exit code, the page's keybindings (null on failure) and stderr.
fn run_bound(config: &str, binds: &[&str]) -> (i32, serde_json::Value, String) {
    let tmp = Tmp::new(&["doc.md"]);
    tmp.write("home/.config/birta/config.toml", config);
    let out = tmp.base.join("out");
    let mut cmd = tmp.birta();
    for bind in binds {
        cmd.args(["--bind", bind]);
    }
    let run = cmd
        .args(["--static", "--out"])
        .arg(&out)
        .arg(tmp.base.join("doc.md"))
        .timeout(Duration::from_secs(20))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    let code = run.status.code().unwrap_or(-1);
    let Ok(page) = std::fs::read_to_string(out.join("doc.html")) else {
        return (code, serde_json::Value::Null, stderr);
    };
    let id = page.find("id=\"birta-boot\"").expect("no boot block");
    let body = &page[id + page[id..].find('>').unwrap() + 1..];
    let boot: serde_json::Value =
        serde_json::from_str(&body[..body.find("</script>").unwrap()]).expect("boot JSON");
    (code, boot["keybindings"].clone(), stderr)
}

/// The keybindings of a run that must succeed, and its stderr.
fn bound(config: &str, binds: &[&str]) -> (serde_json::Value, String) {
    let (code, keys, stderr) = run_bound(config, binds);
    assert_eq!(code, 0, "{stderr}");
    (keys, stderr)
}

/// The stderr of a run that must refuse to start with exit 2 and write no page.
fn refused(config: &str, binds: &[&str]) -> String {
    let (code, keys, stderr) = run_bound(config, binds);
    assert_eq!(code, 2, "{stderr}");
    assert!(keys.is_null(), "a page was written: {keys}");
    stderr
}

fn warnings(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains("warning")).collect()
}

fn names_both(stderr: &str, a: &str, b: &str) -> bool {
    stderr.lines().any(|l| l.contains(a) && l.contains(b))
}

#[test]
fn a_config_key_taken_by_another_action_is_an_error() {
    let stderr = refused("[keybindings]\nyes = \"j\"\n", &[]);
    assert!(names_both(&stderr, "yes", "focus_next"), "{stderr}");
}

#[test]
fn a_viewer_key_rebound_onto_a_review_key_is_an_error() {
    let stderr = refused("[keybindings]\ntoggle_reading = \"j\"\n", &[]);
    assert!(
        names_both(&stderr, "toggle_reading", "focus_next"),
        "{stderr}"
    );
}

#[test]
fn two_config_keys_on_one_free_key_are_an_error() {
    let stderr = refused("[keybindings]\nyes = \"g\"\nlater = \"g\"\n", &[]);
    assert!(names_both(&stderr, "yes", "later"), "{stderr}");
}

#[test]
fn keys_that_match_as_another_collide() {
    // Letters match case-blind, and Mod is Meta or Ctrl
    let stderr = refused("[keybindings]\nyes = \"J\"\n", &[]);
    assert!(names_both(&stderr, "yes", "focus_next"), "{stderr}");
    let stderr = refused("[keybindings]\nopen = \"Ctrl+Enter\"\n", &[]);
    assert!(names_both(&stderr, "open", "turn"), "{stderr}");
}

#[test]
fn two_bind_flags_on_one_key_are_an_error() {
    let stderr = refused("", &["yes=g", "later=g"]);
    assert!(names_both(&stderr, "yes", "later"), "{stderr}");
}

#[test]
fn a_bind_flag_takes_its_key_from_the_config_and_says_so() {
    let (keys, stderr) = bound("", &["yes=r"]);
    assert_eq!(keys["yes"], "r", "{keys}");
    // Reading mode's own default is r, so it is left unbound
    assert_eq!(keys["toggle_reading"], "", "{keys}");
    let warned = warnings(&stderr);
    assert!(
        warned.len() == 1 && warned[0].contains("yes") && warned[0].contains("toggle_reading"),
        "{stderr}"
    );
}

#[test]
fn a_config_key_a_bind_flag_takes_falls_back_to_its_default() {
    let (keys, stderr) = bound("[keybindings]\nyes = \"g\"\n", &["later=g"]);
    assert_eq!(
        (&keys["later"], &keys["yes"]),
        (&"g".into(), &"y".into()),
        "{keys}"
    );
    assert!(names_both(&stderr, "later", "yes"), "{stderr}");
}

#[test]
fn swapped_keys_are_not_a_collision() {
    let (keys, stderr) = bound("[keybindings]\nyes = \"x\"\nno = \"y\"\n", &[]);
    assert_eq!(
        (&keys["yes"], &keys["no"]),
        (&"x".into(), &"y".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

#[test]
fn a_swap_made_with_two_bind_flags_is_not_a_collision() {
    let (keys, stderr) = bound("", &["yes=x", "no=y"]);
    assert_eq!(
        (&keys["yes"], &keys["no"]),
        (&"x".into(), &"y".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

#[test]
fn a_key_freed_by_another_rebind_can_be_taken() {
    let (keys, stderr) = bound("[keybindings]\ntoggle_reading = \"g\"\n", &["yes=r"]);
    assert_eq!(
        (&keys["toggle_reading"], &keys["yes"]),
        (&"g".into(), &"r".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

#[test]
fn disabled_keys_never_collide() {
    let (keys, stderr) = bound(
        "[keybindings]\nyes = \"\"\nno = \"\"\n",
        &["later=none", "undo=none"],
    );
    assert_eq!(
        (&keys["yes"], &keys["no"]),
        (&"".into(), &"".into()),
        "{keys}"
    );
    assert_eq!(
        (&keys["later"], &keys["undo"]),
        (&"".into(), &"".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

#[test]
fn a_review_key_bound_to_a_free_key_takes_it() {
    let (keys, stderr) = bound("[keybindings]\nhide_marks = \"g\"\n", &["yes=f"]);
    assert_eq!(
        (&keys["hide_marks"], &keys["yes"]),
        (&"g".into(), &"f".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

// --- revert_change: on u with the raw view, which only that pair may share ---

#[test]
fn the_change_revert_shares_u_with_the_raw_view_by_default() {
    let (keys, stderr) = bound("", &[]);
    assert_eq!(
        (&keys["revert_change"], &keys["toggle_raw"]),
        (&"u".into(), &"u".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

#[test]
fn the_change_revert_may_share_a_key_with_the_raw_view_only() {
    let (keys, stderr) = bound(
        "[keybindings]\ntoggle_raw = \"s\"\nrevert_change = \"s\"\n",
        &[],
    );
    assert_eq!(
        (&keys["revert_change"], &keys["toggle_raw"]),
        (&"s".into(), &"s".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
    let stderr = refused("[keybindings]\nrevert_change = \"x\"\n", &[]);
    assert!(names_both(&stderr, "revert_change", "no"), "{stderr}");
    let stderr = refused("[keybindings]\nrevert_change = \"r\"\n", &[]);
    assert!(
        names_both(&stderr, "revert_change", "toggle_reading"),
        "{stderr}"
    );
    // The raw view itself still collides with everything else
    let stderr = refused("[keybindings]\ntoggle_raw = \"x\"\n", &[]);
    assert!(names_both(&stderr, "toggle_raw", "no"), "{stderr}");
}

#[test]
fn bind_flags_may_put_the_revert_and_the_raw_view_on_one_key() {
    let (keys, stderr) = bound("", &["toggle_raw=s", "revert_change=s"]);
    assert_eq!(
        (&keys["revert_change"], &keys["toggle_raw"]),
        (&"s".into(), &"s".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
    let (keys, stderr) = bound("[keybindings]\ntoggle_raw = \"s\"\n", &["revert_change=s"]);
    assert_eq!(
        (&keys["revert_change"], &keys["toggle_raw"]),
        (&"s".into(), &"s".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

#[test]
fn moving_the_raw_view_leaves_the_revert_on_u() {
    let (keys, stderr) = bound("", &["toggle_raw=s"]);
    assert_eq!(
        (&keys["revert_change"], &keys["toggle_raw"]),
        (&"u".into(), &"s".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

#[test]
fn a_bind_flag_that_takes_u_takes_it_from_the_revert_and_the_raw_view() {
    let (keys, stderr) = bound("", &["no=u"]);
    assert_eq!(keys["no"], "u", "{keys}");
    assert_eq!(
        (&keys["revert_change"], &keys["toggle_raw"]),
        (&"".into(), &"".into()),
        "{keys}"
    );
    assert_eq!(warnings(&stderr).len(), 2, "{stderr}");
    assert!(names_both(&stderr, "no", "revert_change"), "{stderr}");
    assert!(names_both(&stderr, "no", "toggle_raw"), "{stderr}");
}

#[test]
fn a_bind_flag_moves_the_revert_onto_another_actions_key_and_takes_it() {
    let (keys, stderr) = bound("", &["revert_change=x"]);
    assert_eq!(
        (&keys["revert_change"], &keys["no"], &keys["toggle_raw"]),
        (&"x".into(), &"".into(), &"u".into()),
        "{keys}"
    );
    assert!(names_both(&stderr, "revert_change", "no"), "{stderr}");
}

#[test]
fn either_one_bound_onto_the_others_key_is_no_clash() {
    for binds in [
        &["toggle_raw=u"][..],
        &["revert_change=u"],
        &["revert_change=s", "toggle_raw=s"],
    ] {
        let (keys, stderr) = bound("", binds);
        let want = binds[0].split('=').nth(1).unwrap();
        assert_eq!(
            (&keys["revert_change"], &keys["toggle_raw"]),
            (&want.into(), &want.into()),
            "{binds:?}: {keys}"
        );
        assert!(warnings(&stderr).is_empty(), "{binds:?}: {stderr}");
    }
    let (keys, stderr) = bound("[keybindings]\nrevert_change = \"s\"\n", &["toggle_raw=s"]);
    assert_eq!(
        (&keys["revert_change"], &keys["toggle_raw"]),
        (&"s".into(), &"s".into()),
        "{keys}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}

#[test]
fn a_flag_that_takes_their_shared_key_sends_both_back_to_u() {
    let (keys, stderr) = bound("[keybindings]\nrevert_change = \"s\"\n", &["yes=s"]);
    assert_eq!(keys["revert_change"], "u", "{keys}");
    let warned = warnings(&stderr);
    assert!(
        warned.len() == 1 && warned[0].contains("on \"u\""),
        "{stderr}"
    );
    let (keys, stderr) = bound(
        "[keybindings]\ntoggle_raw = \"s\"\nrevert_change = \"s\"\n",
        &["yes=s"],
    );
    assert_eq!(
        (&keys["revert_change"], &keys["toggle_raw"], &keys["yes"]),
        (&"u".into(), &"u".into(), &"s".into()),
        "{keys}"
    );
    assert_eq!(warnings(&stderr).len(), 2, "{stderr}");
}
