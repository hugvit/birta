use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use birta::agent::ExitCode;
use birta::agent::identity::{self, detect};
use predicates::prelude::*;

const GATE_OFF: &str = "review is off: set [review] enabled = true in ~/.config/birta/config.toml";

/// A temp folder holding `home/` and the docs under test. Every process runs with a
/// cleared env and `HOME` here, so neither the user's config nor their agent env leaks in.
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

    fn home(&self) -> PathBuf {
        self.base.join("home")
    }

    fn themes_dir(&self) -> PathBuf {
        self.home().join(".local/share/birta/themes")
    }

    fn enable_in_config(&self) {
        self.write(
            "home/.config/birta/config.toml",
            "[review]\nenabled = true\n",
        );
    }

    fn birta(&self) -> Command {
        let mut cmd = Command::cargo_bin("birta").unwrap();
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .current_dir(&self.base)
            .timeout(Duration::from_secs(20));
        cmd
    }

    /// Every review file under the temp folder.
    fn review_files(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.to_string_lossy().ends_with(".birta.jsonl") {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.base, &mut out);
        out
    }
}

/// A served `birta` process, killed on drop so a failing test leaves nothing running.
struct Served {
    child: Child,
    stderr: Vec<String>,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts `birta args…` and waits for its `serving` line. `--no-open` must be in `args`.
fn serve(tmp: &Tmp, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> Served {
    assert!(
        args.contains(&"--no-open"),
        "a test must never open a browser"
    );
    let child = StdCommand::new(assert_cmd::cargo::cargo_bin("birta"))
        .args(args)
        .current_dir(&tmp.base)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.home())
        .envs(envs.iter().copied())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut served = Served {
        child,
        stderr: Vec::new(),
    };
    if let Some(text) = stdin {
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
                "no `serving` line from birta {args:?}; stderr: {:#?}",
                served.stderr
            );
        };
        served.stderr.push(line.clone());
        if line.contains("serving") && line.contains("at http://") {
            return served;
        }
    }
}

fn verb_args() -> [(&'static str, Vec<&'static str>); 4] {
    [
        ("review", vec!["review", "doc.md"]),
        ("respond", vec!["respond", "doc.md"]),
        ("comment", vec!["comment", "doc.md", "--doc", "hello"]),
        ("show", vec!["show", "doc.md"]),
    ]
}

// --- the gate ---

#[test]
fn every_verb_with_the_gate_off_exits_1_before_any_setup() {
    let tmp = Tmp::new(&["doc.md"]);
    for (verb, args) in verb_args() {
        tmp.birta()
            .args(&args)
            .write_stdin("")
            .assert()
            .code(1)
            .stdout("")
            .stderr(format!("{GATE_OFF}\n"));
        assert!(tmp.review_files().is_empty(), "{verb} made a review file");
        assert!(
            !tmp.themes_dir().exists(),
            "{verb} ran the theme install before dispatch"
        );
    }
}

#[test]
fn the_gate_is_checked_before_any_verb_argument() {
    let tmp = Tmp::new(&["doc.md"]);
    for args in [
        vec!["review", "doc.md", "--as", "human"],
        vec!["review", "doc.md", "--as", ""],
        vec!["comment", "doc.md", "--no-such-flag"],
        vec!["show"],
        vec!["review", "--help"],
    ] {
        tmp.birta()
            .args(&args)
            .write_stdin("")
            .assert()
            .code(1)
            .stdout("")
            .stderr(format!("{GATE_OFF}\n"));
    }
}

#[test]
fn review_off_ignores_any_value_of_birta_review_but_1() {
    let tmp = Tmp::new(&["doc.md"]);
    for value in ["0", "", "true", " 1"] {
        tmp.birta()
            .env("BIRTA_REVIEW", value)
            .args(["show", "doc.md"])
            .assert()
            .code(1)
            .stderr(format!("{GATE_OFF}\n"));
    }
}

#[test]
fn the_gate_turns_on_by_env_or_by_config_and_skips_the_theme_install() {
    let by_env = Tmp::new(&["doc.md"]);
    let by_config = Tmp::new(&["doc.md"]);
    by_config.enable_in_config();
    for (tmp, envs) in [(&by_env, vec![("BIRTA_REVIEW", "1")]), (&by_config, vec![])] {
        for (verb, args) in verb_args() {
            tmp.birta()
                .envs(envs.iter().copied())
                .args(&args)
                .write_stdin("")
                .assert()
                .stderr(predicate::str::contains(GATE_OFF).not())
                .stderr(predicate::str::contains("Usage:").not());
            assert!(
                !tmp.themes_dir().exists(),
                "{verb} ran the theme install with the gate on"
            );
        }
    }
}

#[test]
fn help_with_the_gate_off_shows_no_review_surface() {
    let tmp = Tmp::new(&[]);
    for flag in ["--help", "-h"] {
        let out = tmp.birta().arg(flag).assert().success();
        let help = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        for hidden in [
            "Commands:",
            "<COMMAND>",
            "--drafted-by",
            "--as ",
            "respond",
            "comment",
        ] {
            assert!(!help.contains(hidden), "{flag} shows {hidden:?}:\n{help}");
        }
    }
}

// --- dispatch ---

#[test]
fn port_before_a_verb_is_a_usage_error() {
    let tmp = Tmp::new(&[]);
    for envs in [vec![], vec![("BIRTA_REVIEW", "1")]] {
        tmp.birta()
            .envs(envs)
            .args(["--port", "1", "review", "x"])
            .assert()
            .code(2);
    }
}

#[test]
fn a_bare_verb_wins_over_a_path_of_that_name() {
    let tmp = Tmp::new(&["review/notes.txt"]);

    tmp.birta()
        .arg("review")
        .assert()
        .code(1)
        .stderr(format!("{GATE_OFF}\n"));
}

#[test]
fn a_dotted_path_named_like_a_verb_is_a_file() {
    let tmp = Tmp::new(&[]);
    tmp.write("review", "# a doc named review\n");

    let served = serve(&tmp, &["./review", "--no-open", "--port", "0"], &[], None);
    assert!(
        served.stderr.iter().any(|l| l.contains("serving review")),
        "{:#?}",
        served.stderr
    );
}

#[test]
fn a_verb_with_an_extension_is_a_file() {
    let tmp = Tmp::new(&["review.md"]);

    let served = serve(&tmp, &["review.md", "--no-open", "--port", "0"], &[], None);
    assert!(
        served
            .stderr
            .iter()
            .any(|l| l.contains("serving review.md")),
        "{:#?}",
        served.stderr
    );
}

#[test]
fn help_is_a_file_name_not_a_verb() {
    let tmp = Tmp::new(&[]);
    tmp.birta()
        .arg("help")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("file not found: help"));

    tmp.write("help", "# a doc named help\n");
    let served = serve(&tmp, &["help", "--no-open", "--port", "0"], &[], None);
    assert!(
        served.stderr.iter().any(|l| l.contains("serving help")),
        "{:#?}",
        served.stderr
    );
}

#[test]
fn a_file_with_flags_after_it_still_serves() {
    let tmp = Tmp::new(&["README.md"]);

    let served = serve(&tmp, &["README.md", "--no-open", "--port", "0"], &[], None);
    assert!(
        served
            .stderr
            .iter()
            .any(|l| l.contains("serving README.md")),
        "{:#?}",
        served.stderr
    );
}

#[test]
fn stdin_still_serves() {
    let tmp = Tmp::new(&[]);

    let served = serve(
        &tmp,
        &["-", "--no-open", "--port", "0"],
        &[],
        Some("# from stdin\n"),
    );
    assert!(
        served.stderr.iter().any(|l| l.contains("serving stdin")),
        "{:#?}",
        served.stderr
    );
}

#[test]
fn serving_still_installs_themes_and_list_themes_still_lists_them() {
    let tmp = Tmp::new(&[]);

    tmp.birta()
        .arg("--list-themes")
        .assert()
        .success()
        .stdout(predicate::str::contains("github"))
        .stdout(predicate::str::contains("(bundled)"));
    assert!(tmp.themes_dir().join("github.toml").exists());
}

// --- --drafted-by ---

#[test]
fn drafted_by_is_accepted_when_serving_with_the_gate_on_or_off() {
    let tmp = Tmp::new(&["pr.md"]);
    let with_flag: &[&str] = &[
        "--drafted-by",
        "claude",
        "pr.md",
        "--no-open",
        "--port",
        "0",
    ];
    let without: &[&str] = &["pr.md", "--no-open", "--port", "0"];
    for args in [with_flag, without] {
        for envs in [&[("BIRTA_REVIEW", "1")][..], &[]] {
            let served = serve(&tmp, args, envs, None);
            assert!(
                served.stderr.iter().any(|l| l.contains("serving pr.md")),
                "{args:?} {envs:?}: {:#?}",
                served.stderr
            );
        }
    }
    assert!(
        tmp.review_files().is_empty(),
        "serving never creates a review file"
    );
}

#[test]
fn as_human_or_you_is_refused_on_the_command_line() {
    let tmp = Tmp::new(&["doc.md"]);
    for name in ["human", "you", "Human", "", "  "] {
        for verb in ["review", "respond", "comment"] {
            tmp.birta()
                .env("BIRTA_REVIEW", "1")
                .args([verb, "doc.md", "--as", name])
                .write_stdin("")
                .assert()
                .code(2)
                .stderr(predicate::str::contains(format!(
                    "error: invalid value '{name}'"
                )));
        }
        assert!(tmp.review_files().is_empty());
    }
    tmp.birta()
        .env("BIRTA_REVIEW", "1")
        .args(["review", "doc.md", "--as", "claude"])
        .assert()
        .stderr(predicate::str::contains("invalid value").not())
        .stderr(predicate::str::contains("Usage:").not());
}

// --- exit codes ---

#[test]
fn exit_codes_keep_their_frozen_values() {
    assert_eq!(ExitCode::Ok as i32, 0);
    assert_eq!(ExitCode::Error as i32, 1);
    assert_eq!(ExitCode::Usage as i32, 2);
    assert_eq!(ExitCode::ApprovedChanged as i32, 3);
    assert_eq!(ExitCode::Stale as i32, 4);
    assert_eq!(ExitCode::Quote as i32, 65);
    assert_eq!(ExitCode::Timeout as i32, 75);
}

// --- identity ---

/// `--as`, the env, and the name expected.
type Case<'a> = (Option<&'a str>, &'a [(&'a str, &'a str)], Option<&'a str>);

fn name(as_flag: Option<&str>, vars: &[(&str, &str)]) -> Option<String> {
    let env = |key: &str| {
        vars.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    };
    detect(as_flag, &env).map(|id| id.name)
}

#[test]
fn identity_follows_the_chain() {
    let cases: &[Case] = &[
        (None, &[], None),
        (Some("gemini"), &[], Some("gemini")),
        (Some("Gemini"), &[], Some("gemini")),
        // Each source beats the next one down.
        (Some("gemini"), &[("BIRTA_AS", "aider")], Some("gemini")),
        (
            None,
            &[("BIRTA_AS", "aider"), ("AI_AGENT", "cursor/1.2")],
            Some("aider"),
        ),
        (
            None,
            &[("AI_AGENT", "cursor/1.2"), ("CLAUDECODE", "1")],
            Some("cursor"),
        ),
        (
            None,
            &[("CLAUDECODE", "1"), ("CODEX_THREAD_ID", "t1")],
            Some("claude"),
        ),
        (None, &[("BIRTA_AS", "Aider")], Some("aider")),
        // AI_AGENT is cut at the first `_` or `/`.
        (
            None,
            &[("AI_AGENT", "claude-code_2-1-284_agent")],
            Some("claude"),
        ),
        (None, &[("AI_AGENT", "claude-code")], Some("claude")),
        (None, &[("AI_AGENT", "cursor/1.2")], Some("cursor")),
        (None, &[("AI_AGENT", "aider")], Some("aider")),
        (None, &[("CLAUDECODE", "1")], Some("claude")),
        (None, &[("CODEX_THREAD_ID", "t1")], Some("codex")),
        (None, &[("CODEX_SANDBOX", "seatbelt")], Some("codex")),
        // Empty values are absent, so the next source decides.
        (
            None,
            &[("BIRTA_AS", ""), ("AI_AGENT", "cursor/1.2")],
            Some("cursor"),
        ),
        (
            None,
            &[("AI_AGENT", ""), ("CLAUDECODE", "1")],
            Some("claude"),
        ),
        (None, &[("CLAUDECODE", "")], None),
        (None, &[("BIRTA_AS", "")], None),
        (None, &[("BIRTA_AS", "  ")], None),
        (
            None,
            &[
                ("BIRTA_AS", "aider"),
                ("AI_AGENT", "cursor/1.2"),
                ("CLAUDECODE", "1"),
            ],
            Some("aider"),
        ),
        (
            None,
            &[
                ("AI_AGENT", "cursor/1.2"),
                ("CLAUDECODE", "1"),
                ("CODEX_SANDBOX", "x"),
            ],
            Some("cursor"),
        ),
        (None, &[("AI_AGENT", "claude-code_x")], Some("claude")),
        (None, &[("AI_AGENT", "Claude-Code/2.1")], Some("claude")),
        // An env naming the human is no name at all, so the next source decides.
        (
            None,
            &[("BIRTA_AS", "human"), ("CLAUDECODE", "1")],
            Some("claude"),
        ),
        (None, &[("BIRTA_AS", "You")], None),
        // Unrelated agent-looking vars say nothing.
        (None, &[("CLAUDE_CODE_ENTRYPOINT", "cli")], None),
    ];
    for (flag, vars, want) in cases {
        assert_eq!(
            name(*flag, vars).as_deref(),
            *want,
            "flag {flag:?}, env {vars:?}"
        );
    }
}

#[test]
fn as_refuses_the_human_names() {
    for refused in ["human", "you", "Human", "YOU", "", "  ", " human "] {
        assert!(identity::parse_as(refused).is_err(), "{refused}");
    }
    for ok in ["claude", "codex", "aider"] {
        assert!(identity::parse_as(ok).is_ok(), "{ok}");
    }
}

// --- adversary ---

#[test]
fn help_with_the_gate_on_shows_the_verbs_and_drafted_by() {
    let by_env = Tmp::new(&[]);
    let by_config = Tmp::new(&[]);
    by_config.enable_in_config();
    for (tmp, envs) in [(&by_env, vec![("BIRTA_REVIEW", "1")]), (&by_config, vec![])] {
        let out = tmp
            .birta()
            .envs(envs.iter().copied())
            .arg("--help")
            .assert()
            .success();
        let help = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        // Line-anchored: `Preview` contains "review" and "show source" contains "show".
        for shown in [
            "Commands:",
            "\n  review ",
            "\n  respond ",
            "\n  comment ",
            "\n  show ",
            "--drafted-by",
        ] {
            assert!(help.contains(shown), "gate on hides {shown:?}:\n{help}");
        }
        assert!(!help.contains("\n  help "), "no help verb:\n{help}");
    }
}

#[test]
fn the_gate_is_checked_before_the_verb_args_are_validated() {
    let tmp = Tmp::new(&["doc.md"]);
    for args in [
        vec!["respond"],
        vec!["comment"],
        vec!["show"],
        vec!["review", "doc.md", "--bogus"],
        vec!["show", "doc.md", "--bogus"],
        vec!["review", "doc.md", "--as", "human"],
        vec!["review", "doc.md", "--as", "you"],
    ] {
        tmp.birta()
            .args(&args)
            .write_stdin("")
            .assert()
            .code(1)
            .stdout("")
            .stderr(format!("{GATE_OFF}\n"));
    }
}

#[test]
fn with_the_gate_on_bad_verb_args_are_usage_errors() {
    let tmp = Tmp::new(&["doc.md"]);
    for (_, mut args) in verb_args() {
        args.push("--bogus");
        tmp.birta()
            .env("BIRTA_REVIEW", "1")
            .args(&args)
            .write_stdin("")
            .assert()
            .code(2)
            .stderr(predicate::str::contains("Usage:"));
    }
    tmp.birta()
        .env("BIRTA_REVIEW", "1")
        .arg("show")
        .assert()
        .code(2);
}

#[test]
fn respond_and_comment_with_the_gate_off_never_read_stdin() {
    let tmp = Tmp::new(&["doc.md"]);
    for args in [
        vec!["respond", "doc.md"],
        vec!["comment", "doc.md", "--doc"],
    ] {
        let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("birta"))
            .args(&args)
            .current_dir(&tmp.base)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", tmp.home())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Held open, so a read before the gate blocks instead of seeing EOF.
        let _stdin = child.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{args:?} read stdin before the gate check");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(status.code(), Some(1), "{args:?}");
    }
}

#[test]
fn a_verb_name_anywhere_but_first_is_not_a_verb() {
    let tmp = Tmp::new(&["doc.md"]);
    // After `--` it is a path.
    tmp.birta()
        .args(["--", "review"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("file not found: review"))
        .stderr(predicate::str::contains(GATE_OFF).not());
    // A later positional is an extra arg.
    tmp.birta()
        .args(["doc.md", "review"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(GATE_OFF).not());
    // A flag value.
    let served = serve(
        &tmp,
        &[
            "--font-body",
            "review",
            "doc.md",
            "--no-open",
            "--port",
            "0",
        ],
        &[],
        None,
    );
    assert!(
        served.stderr.iter().any(|l| l.contains("serving doc.md")),
        "{:#?}",
        served.stderr
    );
}

#[test]
fn a_flag_before_a_verb_name_makes_it_a_file() {
    let tmp = Tmp::new(&[]);
    tmp.write("review", "# a doc named review\n");
    for envs in [vec![], vec![("BIRTA_REVIEW", "1")]] {
        let served = serve(&tmp, &["--no-open", "--port", "0", "review"], &envs, None);
        assert!(
            served.stderr.iter().any(|l| l.contains("serving review")),
            "{:#?}",
            served.stderr
        );
    }
}

#[test]
fn verbs_match_exactly_not_by_case_or_prefix() {
    // A fresh folder: macOS paths are case-insensitive, so no `review` may exist here.
    let tmp = Tmp::new(&[]);
    for word in ["Review", "SHOW", "rev", "s", "resp"] {
        tmp.birta()
            .arg(word)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(format!("file not found: {word}")))
            .stderr(predicate::str::contains(GATE_OFF).not());
    }
}

#[test]
fn birta_review_only_forces_the_gate_on() {
    let off = Tmp::new(&["doc.md"]);
    for value in ["1 ", "01", "yes", "1\n"] {
        off.birta()
            .env("BIRTA_REVIEW", value)
            .args(["show", "doc.md"])
            .assert()
            .code(1)
            .stderr(format!("{GATE_OFF}\n"));
    }
    let on = Tmp::new(&["doc.md"]);
    on.enable_in_config();
    on.birta()
        .env("BIRTA_REVIEW", "0")
        .args(["show", "doc.md"])
        .assert()
        .stderr(predicate::str::contains(GATE_OFF).not());
}

#[test]
fn a_config_that_fails_to_parse_leaves_the_gate_off() {
    let tmp = Tmp::new(&["doc.md"]);
    tmp.write(
        "home/.config/birta/config.toml",
        "[review]\nenabled = \"true\"\n",
    );
    tmp.birta()
        .args(["show", "doc.md"])
        .assert()
        .code(1)
        // The config's parse warning comes first.
        .stderr(predicate::str::contains("failed to parse config"))
        .stderr(predicate::str::ends_with(format!("\n{GATE_OFF}\n")));
}

#[test]
fn drafted_by_is_accepted_and_ignored_with_the_gate_off() {
    let tmp = Tmp::new(&["pr.md"]);
    let _served = serve(
        &tmp,
        &[
            "--drafted-by",
            "claude",
            "pr.md",
            "--no-open",
            "--port",
            "0",
        ],
        &[],
        None,
    );
    // Stdin and --static are always off, even with the env set.
    let _stdin = serve(
        &tmp,
        &["--drafted-by", "claude", "-", "--no-open", "--port", "0"],
        &[("BIRTA_REVIEW", "1")],
        Some("# from stdin\n"),
    );
    let out = tmp.base.join("out");
    tmp.birta()
        .env("BIRTA_REVIEW", "1")
        .args(["--static", "--no-open", "--drafted-by", "claude", "--out"])
        .arg(&out)
        .arg("pr.md")
        .assert()
        .success();
    assert!(tmp.review_files().is_empty());
}

#[test]
fn identity_precedence_holds_across_the_whole_chain() {
    let all: &[(&str, &str)] = &[
        ("BIRTA_AS", "aider"),
        ("AI_AGENT", "cursor/1.2"),
        ("CLAUDECODE", "1"),
        ("CODEX_THREAD_ID", "t1"),
    ];
    assert_eq!(name(Some("gemini"), all).as_deref(), Some("gemini"));
    assert_eq!(name(None, all).as_deref(), Some("aider"));
    assert_eq!(name(None, &all[1..]).as_deref(), Some("cursor"));
    assert_eq!(
        name(
            None,
            &[("AI_AGENT", "cursor"), ("CODEX_SANDBOX", "seatbelt")]
        )
        .as_deref(),
        Some("cursor")
    );
    assert_eq!(
        name(Some("codex"), &[("CLAUDECODE", "1")]).as_deref(),
        Some("codex")
    );
}

#[test]
fn ai_agent_is_lowercased_then_cut_only_at_underscore_or_slash() {
    let cases: &[(&str, &str)] = &[
        ("Claude-Code_2-1-284_agent", "claude"),
        ("CLAUDE-CODE", "claude"),
        ("claude-code/1.0", "claude"),
        ("cursor-1.2", "cursor-1.2"),
        ("Cursor/1.2", "cursor"),
    ];
    for (raw, want) in cases {
        assert_eq!(
            name(None, &[("AI_AGENT", raw)]).as_deref(),
            Some(*want),
            "{raw}"
        );
    }
}

#[test]
fn a_detected_name_is_never_empty() {
    for raw in ["_x", "/x", "_", "/"] {
        let got = name(None, &[("AI_AGENT", raw), ("CLAUDECODE", "1")]);
        assert_eq!(got.as_deref(), Some("claude"), "AI_AGENT={raw:?}");
    }
    assert!(identity::parse_as("").is_err());
}

#[test]
fn as_refuses_the_human_names_with_padding() {
    for refused in [" human", "human ", "you\n", "\tYou"] {
        assert!(identity::parse_as(refused).is_err(), "{refused:?}");
    }
}

// --- rulings ---

#[test]
fn every_source_is_trimmed_lowercased_and_maps_claude_code() {
    let cases: &[Case] = &[
        (Some(" aider "), &[], Some("aider")),
        (Some("claude-code"), &[], Some("claude")),
        (Some("Claude-Code"), &[], Some("claude")),
        (None, &[("BIRTA_AS", " Aider ")], Some("aider")),
        (None, &[("BIRTA_AS", "CLAUDE-CODE")], Some("claude")),
        (None, &[("AI_AGENT", " cursor/1.2")], Some("cursor")),
        (
            None,
            &[("BIRTA_AS", "you"), ("AI_AGENT", "aider")],
            Some("aider"),
        ),
        (None, &[("CLAUDECODE", "0")], Some("claude")),
        (None, &[("CLAUDECODE", "  ")], None),
        (None, &[("CODEX_THREAD_ID", " ")], None),
    ];
    for (flag, vars, want) in cases {
        assert_eq!(
            name(*flag, vars).as_deref(),
            *want,
            "flag {flag:?}, env {vars:?}"
        );
    }
    assert_eq!(identity::parse_as(" Claude-Code ").as_deref(), Ok("claude"));
}

#[test]
fn drafted_by_on_review_needs_wait() {
    let tmp = Tmp::new(&["doc.md"]);
    tmp.birta()
        .env("BIRTA_REVIEW", "1")
        .args(["review", "doc.md", "--drafted-by", "claude"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--wait"));
}
