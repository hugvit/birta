//! The agent contract: what `birta review` and `birta respond` tell an agent to do next,
//! the answers to its asks, and the grammar `respond --help` documents. The test plays
//! the browser by writing the review file and the agent's Edit by writing the doc. Every
//! process runs with a cleared env, `HOME` in a temp folder, the gate on and `TZ=UTC`.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::{Duration, UNIX_EPOCH};

use assert_cmd::Command;
use birta::agent::respond::{self, Entry};
use birta::review::chain::Chain;
use birta::review::ops::{self, Op};
use serde_json::{Value, json};

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, Scenario, human, ts};

/// The template's lines for the reviewer after `EOF`, for a round holding a `no`.
const AFTER_EOF: [&str; 4] = [
    "      Per note you can write instead:  #N didnt: <why>   or   #N answer: <reply>",
    "      To ask the reviewer:             ask \"<exact words in the doc>\": <question>",
    "      For a change no note asked for:  unasked \"<exact new words>\": <why>",
    "      `no` means the reviewer wants that text gone; cut it, or say why not with didnt.",
];
const TURN_NOTE: &str =
    "@: <one or two lines to the reviewer in first person (\"I cut…\"); no questions here>";
const DOC_DONE: &str = "<one line: what you changed for this whole-doc note>";

/// `HH:MM` of epoch ms in UTC.
fn hhmm(ms: u64) -> String {
    let secs = ms / 1000 % 86_400;
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// A temp folder holding `home/` and the docs under test.
struct Tmp {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Tmp {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        fs::create_dir(base.join("home")).unwrap();
        Self { _tmp: tmp, base }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    /// The doc at `rel` and its review file, as `s` left them.
    fn put_doc(&self, rel: &str, s: &Scenario) {
        let name = Path::new(rel).file_name().unwrap().to_str().unwrap();
        self.write(rel, s.text());
        let log = s
            .log()
            .replacen("\"doc\":\"pr.md\"", &format!("\"doc\":\"{name}\""), 1);
        self.write(&format!("{rel}.birta.jsonl"), &log);
    }

    /// The agent's Edit of the doc at `rel`, dated `mtime_ms`.
    fn edit(&self, rel: &str, text: &str, mtime_ms: u64) {
        let path = self.path(rel);
        fs::write(&path, text).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_millis(mtime_ms))
            .unwrap();
    }

    fn ops(&self, rel: &str) -> Vec<Op> {
        let parsed = ops::parse(self.read(&format!("{rel}.birta.jsonl")));
        assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
        parsed.lines.into_iter().map(|l| l.op).collect()
    }

    fn claims(&self, rel: &str) -> usize {
        self.ops(rel)
            .iter()
            .filter(|op| matches!(op, Op::Claim { .. }))
            .count()
    }

    /// `birta args…` with `stdin`: exit code, stdout and stderr.
    fn run(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        self.run_in("", args, stdin)
    }

    /// [`Tmp::run`] with the working folder `dir`, relative to the base.
    fn run_in(&self, dir: &str, args: &[&str], stdin: &str) -> (i32, String, String) {
        let out = Command::cargo_bin("birta")
            .unwrap()
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path("home"))
            .env("TZ", "UTC")
            .env("BIRTA_REVIEW", "1")
            .args(args)
            .current_dir(self.path(dir))
            .write_stdin(stdin)
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        let code = out
            .status
            .code()
            .expect("birta was killed by the test timeout");
        let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        (code, stdout, stderr)
    }

    /// `review args… --as claude`, which must exit 0: its stdout.
    fn review(&self, args: &[&str]) -> String {
        let mut all = vec!["review"];
        all.extend_from_slice(args);
        all.extend_from_slice(&["--as", "claude"]);
        let (code, out, err) = self.run(&all, "");
        assert_eq!(code, 0, "{all:?}: {out}{err}");
        out
    }

    fn review_json(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let out = self.review(&all);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    fn respond(&self, rel: &str, heredoc: &str) -> (i32, String, String) {
        self.run(&["respond", rel, "--as", "claude"], heredoc)
    }

    /// `script` run by `sh`, as an agent's shell tool would, with `birta` on the `PATH`
    /// and `BIRTA_AS=claude`.
    fn sh(&self, script: &str) -> (i32, String, String) {
        let bin = assert_cmd::cargo::cargo_bin("birta");
        let mut path = std::ffi::OsString::from(bin.parent().unwrap());
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let out = StdCommand::new("sh")
            .arg("-c")
            .arg(script)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.path("home"))
            .env("TZ", "UTC")
            .env("BIRTA_REVIEW", "1")
            .env("BIRTA_AS", "claude")
            .current_dir(&self.base)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

/// The index of the first line starting with `prefix`.
fn find(lines: &[&str], prefix: &str, out: &str) -> usize {
    lines
        .iter()
        .position(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starts with {prefix:?}:\n{out}"))
}

/// The lines between `<<'EOF'` and `EOF`, as an agent pastes them.
fn heredoc(out: &str) -> Vec<&str> {
    out.lines()
        .skip_while(|l| !l.ends_with("<<'EOF'"))
        .skip(1)
        .take_while(|l| l.trim() != "EOF")
        .collect()
}

// --- N1: answers to the agent's asks ---

/// The ux-a try's review file up to line `last`, and the doc as the log left it then.
fn ux_a_until(last: usize) -> (String, String) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/real_logs/ux_a");
    let full = fs::read_to_string(dir.join("pr-draft.md.birta.jsonl")).unwrap();
    let log: String = full.lines().take(last).map(|l| format!("{l}\n")).collect();
    let parsed = ops::parse(&log);
    assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
    let chain = Chain::build(&parsed);
    let text = chain.text(chain.head().unwrap()).unwrap().to_string();
    (text, log)
}

/// The ux-a try's second hand-back: note 27, and the reply on line 28 to the agent's
/// ask on line 17. Nothing past the submit on line 29.
fn ux_a_round_two() -> Tmp {
    let (text, log) = ux_a_until(29);
    let tmp = Tmp::new();
    tmp.write("pr-draft.md", &text);
    tmp.write("pr-draft.md.birta.jsonl", &log);
    tmp
}

#[test]
fn a_reply_to_the_agents_ask_reaches_the_round_view() {
    let tmp = ux_a_round_two();
    let out = tmp.review(&["pr-draft.md"]);
    let lines: Vec<&str> = out.lines().collect();
    let ans = find(&lines, "ANS ", &out);
    assert_eq!(
        lines[ans],
        "ANS   your ask on \"Integration suite (not run yet)\" — Keep it open until the integration suite has run.",
        "{out}"
    );
    assert!(find(&lines, "#27 ", &out) < ans, "{out}");
    assert!(ans < find(&lines, "next:", &out), "{out}");
    assert!(lines[0].contains(" · 1 note · 1 answer"), "{out}");
    let (_, target) = lines[find(&lines, "#27 ", &out)]
        .split_once("note  ")
        .unwrap();
    assert!(!target.contains("  "), "a double space in {target:?}");

    // An answer asks for no outcome: the heredoc names the note only
    let asked = heredoc(&out);
    assert!(asked.contains(&"#27 done"), "{out}");
    assert!(
        !asked
            .iter()
            .any(|l| l.starts_with("#17") || l.starts_with("#28")),
        "{out}"
    );
}

#[test]
fn json_lists_the_answers_beside_the_items() {
    let tmp = ux_a_round_two();
    let v = tmp.review_json(&["pr-draft.md"]);
    let items: Vec<u64> = v["items"]
        .as_array()
        .expect("an items array")
        .iter()
        .map(|i| i["n"].as_u64().unwrap())
        .collect();
    assert_eq!(items, [27], "{v}");
    let answers = v["answers"].as_array().expect("an answers array");
    assert_eq!(answers.len(), 1, "{v}");
    assert_eq!(answers[0]["to"], 17, "{v}");
    assert_eq!(answers[0]["n"], 28, "{v}");
    assert_eq!(
        answers[0]["quote"], "Integration suite (not run yet)",
        "{v}"
    );
    assert_eq!(
        answers[0]["body"], "Keep it open until the integration suite has run.",
        "{v}"
    );
}

/// A round settled by claude, holding claude's ask on "detail two". Returns the ask.
fn asked_in_round_one(s: &mut Scenario) -> u32 {
    let first = s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let turn = s.claim("claude");
    s.done("claude", first, turn);
    let ask = s.blank("claude", s.quote("detail two"), "keep this?", turn);
    s.yield_turn("claude", turn);
    ask
}

#[test]
fn a_verdict_only_answer_reads_as_its_verdict() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    let reply = s.human_reply(ask, Some("yes"), None);
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    assert!(
        out.lines()
            .any(|l| l == "ANS   your ask on \"detail two\" — yes"),
        "{out}"
    );
    let v = tmp.review_json(&["pr.md"]);
    let answers = v["answers"].as_array().expect("an answers array");
    assert_eq!(answers.len(), 1, "{v}");
    assert_eq!(answers[0]["to"], ask, "{v}");
    assert_eq!(answers[0]["n"], reply, "{v}");
    assert_eq!(answers[0]["verdict"], "yes", "{v}");
}

#[test]
fn a_reply_on_a_human_note_is_not_an_answer() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let note = s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let turn = s.claim("claude");
    s.agent_reply("claude", note, "answer", "S3 flakes");
    s.yield_turn("claude", turn);
    s.human_reply(note, None, Some("THREAD-REPLY"));
    s.note("round two");
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    assert!(!out.lines().any(|l| l.starts_with("ANS")), "{out}");
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(v["answers"], json!([]), "{v}");
}

#[test]
fn an_answer_is_listed_in_its_own_round_only_and_a_draft_answer_never() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    s.human_reply(ask, None, Some("FIRST-ANSWER"));
    s.submit();
    let second = s.claim("claude");
    s.yield_turn("claude", second);
    s.comment(s.quote("third item"), None, Some("round three"));
    s.submit();
    s.human_reply(ask, None, Some("DRAFT-ANSWER"));
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    assert!(out.contains("round three"), "{out}");
    assert!(!out.contains("FIRST-ANSWER"), "shown in round two:\n{out}");
    assert!(
        !out.contains("DRAFT-ANSWER"),
        "written after the hand-back:\n{out}"
    );
    assert!(!out.lines().any(|l| l.starts_with("ANS")), "{out}");
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(v["answers"], json!([]), "{v}");
}

#[test]
fn an_approve_lists_the_asks_left_unanswered() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let first = s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let turn = s.claim("claude");
    s.done("claude", first, turn);
    let answered = s.blank("claude", s.quote("detail one"), "keep this?", turn);
    let open = s.blank("claude", s.quote("detail two"), "and this?", turn);
    s.yield_turn("claude", turn);
    s.human_reply(answered, Some("yes"), None);
    s.approve();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    assert!(out.starts_with("pr.md · approved "), "{out}");
    let unanswered: Vec<&str> = out.lines().filter(|l| l.contains("unanswered")).collect();
    assert_eq!(unanswered.len(), 1, "{out}");
    assert!(
        unanswered[0].contains("your ask on \"detail two\""),
        "{out}"
    );
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(v["status"], "approved", "{v}");
    let listed = v["unanswered"].as_array().expect("an unanswered array");
    assert_eq!(listed.len(), 1, "{v}");
    assert_eq!(listed[0]["to"], open, "{v}");
}

// --- the `next:` template ---

#[test]
fn the_next_block_is_the_template_with_a_flush_left_heredoc() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.comment(s.quote("Intro paragraph here"), Some("stet"), None);
    let doc = s.note("too long overall");
    let note = s.comment(s.quote("second item"), None, Some("say why"));
    let no = s.comment(s.section("## Details"), Some("no"), None);
    let ask = s.comment(s.quote("detail one"), Some("ask"), Some("is this right?"));
    let submit = s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        format!(
            "pr.md · handed back {} by andres · 5 notes · drafted by claude",
            hhmm(ts(submit))
        ),
        "{out}"
    );
    let next = find(&lines, "next:", &out);
    let mut want = vec![
        "next: 1) Edit pr.md with your Edit tool (not Write). Change only what the notes ask."
            .to_string(),
        "      2) Record what you did, one line per note, then run:".to_string(),
        "birta respond pr.md <<'EOF'".to_string(),
        format!("#{note} done"),
        format!("#{no} done"),
        format!("#{ask} answer: <your answer>"),
        format!("#{doc} done: {DOC_DONE}"),
        format!("#{doc} covers \"<a sentence you wrote for #{doc}>\""),
        TURN_NOTE.to_string(),
        "EOF".to_string(),
    ];
    want.extend(AFTER_EOF.map(str::to_string));
    want.push("      3) Then wait again: birta review pr.md --wait".to_string());
    assert_eq!(lines[next..], want, "{out}");
}

#[test]
fn the_heredoc_pasted_into_sh_records_and_ends_at_eof() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.note("too long overall");
    s.comment(s.quote("second item"), None, Some("say why"));
    s.comment(s.quote("detail one"), Some("ask"), Some("is this right?"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let claim = s.next();

    let text = s.replaced("- second item", "- second item, because S3 flakes");
    tmp.edit(
        "pr.md",
        &format!("{text}\nRetries now use jitter.\n"),
        ts(claim) + 1_000,
    );
    pasted(&tmp, &out, "pr.md", "Retries now use jitter");
}

/// Fills the first heredoc in `out` as an agent would, citing `covers` for a doc note,
/// and runs it through `sh` with an `echo` after it. The log of `rel` must end yielded.
fn pasted(tmp: &Tmp, out: &str, rel: &str, covers: &str) -> String {
    let lines: Vec<&str> = out.lines().collect();
    let start = find(&lines, "birta respond ", out);
    let end = start
        + lines[start..]
            .iter()
            .position(|l| *l == "EOF")
            .unwrap_or_else(|| panic!("no flush-left EOF:\n{out}"));
    let filled: Vec<String> = lines[start..=end]
        .iter()
        .map(|l| match l.split_once(" covers ") {
            Some((n, _)) => format!("{n} covers \"{covers}\""),
            None if l.starts_with("@:") => "@: I did what the notes asked.".to_string(),
            None => l
                .replace("<your answer>", "yes, five is plenty")
                .replace(DOC_DONE, "I added a line"),
        })
        .collect();
    assert!(
        filled
            .iter()
            .all(|l| !l.contains('<') || l.contains("<<'EOF'")),
        "a placeholder the test doesn't know: {filled:#?}"
    );
    let script = format!("{}\necho AFTER-EOF\n", filled.join("\n"));
    let (code, stdout, stderr) = tmp.sh(&script);
    assert_eq!(code, 0, "{script}\n{stdout}{stderr}");
    assert!(stdout.contains("recorded: "), "{stdout}{stderr}");
    assert!(
        stdout.contains("AFTER-EOF"),
        "the heredoc never ended: {stdout}"
    );
    assert!(
        matches!(tmp.ops(rel).last(), Some(Op::Yield { .. })),
        "{stdout}{stderr}"
    );
    stdout
}

#[test]
fn the_heredoc_pastes_for_quoted_paths_and_a_folder_claim() {
    for name in ["it's.md", "my notes.md"] {
        let mut s = Scenario::drafted_by(DOC, "claude");
        s.comment(s.quote("second item"), None, Some("say why"));
        s.submit();
        let tmp = Tmp::new();
        tmp.put_doc(name, &s);
        let out = tmp.review(&[name]);
        let stdout = pasted(&tmp, &out, name, "");
        let quoted = format!("'{}'", name.replace('\'', r"'\''"));
        assert!(
            stdout.contains(&format!("next: birta review {quoted} --wait")),
            "{stdout}"
        );
    }
    let tmp = Tmp::new();
    for rel in ["docs/a.md", "docs/b.md"] {
        let (s, _) = batched(&[(Some("second item"), None, "say why")], 7);
        tmp.put_doc(rel, &s);
    }
    let out = tmp.review(&["docs"]);
    let stdout = pasted(&tmp, &out, "docs/a.md", "");
    assert!(
        stdout.contains("next: birta review docs --wait"),
        "{stdout}"
    );
}

#[test]
fn the_unfilled_heredoc_is_refused_once_per_placeholder() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.note("too long overall");
    s.comment(s.quote("second item"), None, Some("say why"));
    s.comment(s.quote("detail one"), Some("ask"), Some("is this right?"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let log = tmp.read("pr.md.birta.jsonl");
    let piped: String = heredoc(&out).iter().map(|l| format!("{l}\n")).collect();
    let (code, stdout, stderr) = tmp.respond("pr.md", &piped);
    assert_eq!(code, 2, "{piped}\n{stdout}{stderr}");
    assert_eq!(
        stderr.matches("placeholder").count(),
        4,
        "done:, covers, answer and @: each:\n{stderr}"
    );
    assert_eq!(tmp.read("pr.md.birta.jsonl"), log);
}

#[test]
fn every_placeholder_the_template_prints_is_refused() {
    for line in [
        format!("#5 done: {DOC_DONE}"),
        "#5 covers \"<a sentence you wrote for #5>\"".to_string(),
        "#6 answer: <your answer>".to_string(),
        TURN_NOTE.to_string(),
        "#6 didnt: <why>".to_string(),
        "#6 answer: <reply>".to_string(),
        "ask \"<exact words in the doc>\": <question>".to_string(),
        "unasked \"<exact new words>\": <why>".to_string(),
    ] {
        match respond::parse(&line) {
            Ok(entries) => panic!("{line:?} was accepted: {entries:?}"),
            Err(errors) => assert!(
                errors
                    .iter()
                    .any(|e| e.msg.starts_with("placeholder left in")),
                "{line:?}: {errors:?}"
            ),
        }
    }
}

// --- N15: notes, and block targets on one line ---

#[test]
fn the_header_counts_notes() {
    let mut s = Scenario::new(DOC);
    s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let header = out.lines().next().unwrap();
    assert!(header.ends_with(" · 1 note"), "{out}");
}

#[test]
fn a_hard_wrapped_block_target_is_joined_with_single_spaces() {
    let text = concat!(
        "# Plan\n\n",
        "Retries back off with jitter, so a burst \n",
        "of 503s spreads out over a minute.\n\n",
        "The timer was chosen in 2023 because writes were rare. They \n",
        "now happen every minute.\n",
    );
    let mut s = Scenario::new(text);
    let burst = s.comment(s.block("Retries back off"), None, Some("say more"));
    let timer = s.comment(s.block("The timer was"), None, Some("fold this"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let lines: Vec<&str> = out.lines().collect();
    let target = |n: u32| {
        let line = lines[find(&lines, &format!("#{n} "), &out)];
        let (_, text) = line
            .split_once("note  ")
            .unwrap_or_else(|| panic!("{line}"));
        assert!(!text.contains("  "), "a double space in {line:?}");
        text.to_string()
    };
    assert!(target(burst).contains("so a burst of 503s"), "{out}");
    let timer = target(timer);
    assert!(
        timer.starts_with("The timer was chosen in 2023 because writes were rare. They"),
        "{out}"
    );
    assert!(timer.ends_with(" — fold this"), "{out}");
}

// --- `respond --help` ---

#[test]
fn respond_help_shows_a_parseable_example_of_every_keyword() {
    let tmp = Tmp::new();
    let (code, help, err) = tmp.run(&["respond", "--help"], "");
    assert_eq!(code, 0, "{help}{err}");
    let mut kinds = std::collections::BTreeSet::new();
    for line in help.lines().map(str::trim) {
        let numbered = line
            .strip_prefix('#')
            .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()));
        let example = numbered
            || line.starts_with("ask \"")
            || line.starts_with("unasked \"")
            || line.starts_with("@:");
        if !example {
            continue;
        }
        let entries = respond::parse(line)
            .unwrap_or_else(|e| panic!("the example {line:?} doesn't parse: {e:?}"));
        assert_eq!(entries.len(), 1, "{line:?}");
        kinds.insert(match entries[0].1 {
            Entry::Done { .. } => "done",
            Entry::Answer { .. } => "answer",
            Entry::Didnt { .. } => "didnt",
            Entry::Covers { .. } => "covers",
            Entry::Ask { .. } => "ask",
            Entry::Unasked { .. } => "unasked",
            Entry::TurnNote { .. } => "@:",
        });
    }
    assert_eq!(
        kinds.into_iter().collect::<Vec<_>>(),
        ["@:", "answer", "ask", "covers", "didnt", "done", "unasked"],
        "{help}"
    );
}

// --- N16: what `respond` says after recording ---

/// Claude holds a round on `pr.md` with a doc note and a note on "second item".
fn held() -> (Scenario, u32, u32, u32) {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let doc = s.note("too long overall");
    let note = s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let claim = s.claim("claude");
    (s, doc, note, claim)
}

#[test]
fn respond_warns_about_the_one_change_that_cites_no_note_and_points_onward() {
    let (s, doc, note, claim) = held();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    // At the quoted spot, uncited, flagged unasked, and cited for the doc note
    let text = s
        .replaced("- second item", "- second item, because S3 flakes")
        .replacen("- detail two\n", "- detail 2\n", 1)
        .replacen("- more item\n", "- more items\n", 1);
    tmp.edit(
        "pr.md",
        &format!("{text}\nRetries now use jitter.\n"),
        ts(claim) + 1_000,
    );
    let heredoc = format!(
        "#{note} done
#{doc} done: I added a line on jitter
#{doc} covers \"Retries now use jitter\"
unasked \"more items\": fixed a typo
@: I said why and added a line on jitter.
"
    );
    let (code, stdout, stderr) = tmp.respond("pr.md", &heredoc);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let warned: Vec<&str> = stderr.lines().filter(|l| l.contains("no note")).collect();
    assert_eq!(warned.len(), 1, "{stderr}");
    let warning = warned[0];
    assert!(
        warning.starts_with("warning: 1 change cites no note"),
        "{warning}"
    );
    assert!(warning.contains("detail 2"), "{warning}");
    assert!(warning.contains("unasked \""), "{warning}");
    for listed in ["more items", "jitter", "S3 flakes"] {
        assert!(!warning.contains(listed), "{listed:?} in {warning}");
    }
    assert_eq!(
        stdout.lines().last(),
        Some("next: birta review pr.md --wait"),
        "{stdout}"
    );
}

#[test]
fn a_change_from_an_earlier_turn_is_not_warned_about_again() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let first = s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let one = s.claim("claude");
    let text = s
        .replaced("- second item", "- second item, because S3 flakes")
        .replacen("- detail two\n", "- detail 2\n", 1);
    s.external(&text);
    s.done("claude", first, one);
    s.yield_turn("claude", one);
    let second = s.comment(s.quote("third item"), None, Some("and here"));
    s.submit();
    let two = s.claim("claude");
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    tmp.edit(
        "pr.md",
        &s.replaced("- third item", "- third item, too"),
        ts(two) + 1_000,
    );
    let (code, stdout, stderr) =
        tmp.respond("pr.md", &format!("#{second} done\n@: I did that too.\n"));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!stderr.contains("no note"), "{stderr}");
    assert_eq!(
        stdout.lines().last(),
        Some("next: birta review pr.md --wait"),
        "{stdout}"
    );
}

#[test]
fn a_change_that_renders_the_same_is_not_warned_about() {
    let (s, _, note, claim) = held();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let text = s
        .replaced("- second item", "- second item, because S3 flakes")
        .replacen("## More\n\n", "## More\n\n\n", 1);
    tmp.edit("pr.md", &text, ts(claim) + 1_000);
    let (code, stdout, stderr) = tmp.respond(
        "pr.md",
        &format!("#{note} done\n@: I said why; the doc note needs nothing.\n"),
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!stderr.contains("no note"), "{stderr}");
    assert_eq!(
        stdout.lines().last(),
        Some("next: birta review pr.md --wait"),
        "{stdout}"
    );
}

// --- folders ---

/// A review file for `DOC` whose `notes` are handed back in hand-back `batch`. Returns
/// their lines.
fn batched(notes: &[(Option<&str>, Option<&str>, &str)], batch: u64) -> (Scenario, Vec<u32>) {
    let mut s = Scenario::new(DOC);
    let ns: Vec<u32> = notes
        .iter()
        .map(|(quote, verdict, body)| {
            let at = match quote {
                Some(q) if q.starts_with('#') => s.section(q),
                Some(q) => s.quote(q),
                None => json!("doc"),
            };
            let body = (!body.is_empty()).then_some(*body);
            s.comment(at, *verdict, body)
        })
        .collect();
    let upto = *ns.last().unwrap();
    s.push(
        human(),
        json!({"op": "submit", "upto": upto, "not_before": ts(s.next()), "hold_ms": 0,
               "to": "*", "batch": batch}),
    );
    (s, ns)
}

#[test]
fn a_folder_peek_is_one_batch_with_its_notes_by_file_and_no_next() {
    let (a, _) = batched(
        &[
            (Some("second item"), None, "say why"),
            (Some("## Details"), Some("later"), ""),
        ],
        7,
    );
    let (b, _) = batched(
        &[
            (None, None, "FROM-B"),
            (Some("detail one"), Some("yes"), ""),
        ],
        7,
    );
    let tmp = Tmp::new();
    tmp.put_doc("docs/a.md", &a);
    tmp.put_doc("docs/b.md", &b);
    let (code, out, err) = tmp.run(&["review", "docs", "--peek"], "");
    assert_eq!(code, 0, "{out}{err}");
    for rel in ["docs/a.md", "docs/b.md"] {
        assert_eq!(tmp.claims(rel), 0, "{rel}");
    }
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        format!("docs/ · handed back {} · 2 files · 4 notes", hhmm(ts(6))),
        "{out}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("next:") || l.contains("<<'EOF'") || l.contains("--wait")),
        "{out}"
    );
    let later = find(
        &lines,
        "LATER ## Details (whole section) — parked by the reviewer; leave it",
        &out,
    );
    let yes = find(&lines, "YES   \"detail one\" — keep as is", &out);
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with('#') && (l.contains(" later ") || l.contains(" yes "))),
        "{out}"
    );
    let at = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in\n{out}"))
    };
    let (a_head, b_head) = (at("docs/a.md"), at("docs/b.md"));
    assert!(a_head < at("say why") && at("say why") < b_head, "{out}");
    assert!(a_head < later && later < b_head, "{out}");
    assert!(b_head < at("FROM-B") && b_head < yes, "{out}");
}

#[test]
fn a_folder_claim_waits_once_on_the_folder_and_respond_points_there() {
    let tmp = Tmp::new();
    let mut notes = Vec::new();
    for (rel, body) in [("docs/a.md", "FROM-A"), ("docs/b.md", "FROM-B")] {
        let (s, ns) = batched(&[(None, None, body)], 7);
        tmp.put_doc(rel, &s);
        notes.push(ns[0]);
    }
    let out = tmp.review(&["docs"]);
    for rel in ["docs/a.md", "docs/b.md"] {
        assert_eq!(tmp.claims(rel), 1, "{rel}");
    }
    let waits: Vec<&str> = out.lines().filter(|l| l.contains("--wait")).collect();
    assert_eq!(waits.len(), 1, "{out}");
    assert!(
        out.lines()
            .last()
            .is_some_and(|l| l.ends_with("birta review docs --wait")),
        "{out}"
    );

    let (code, stdout, stderr) = tmp.respond(
        "docs/a.md",
        &format!("#{} done: nothing to change\n@: I left it.\n", notes[0]),
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        stdout.lines().last(),
        Some("next: birta review docs --wait"),
        "{stdout}"
    );
}

// --- N2: approved ---

#[test]
fn the_approved_next_line_shows_the_text_and_asks_before_publishing() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.note("first");
    s.submit();
    let turn = s.claim("claude");
    s.yield_turn("claude", turn);
    s.approve();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let short = &birta::rev::Rev::of(DOC.as_bytes()).to_string()[..8];
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.get(2).copied(),
        Some(
            format!(
                "next: birta show pr.md@{short} prints the approved text. Use exactly that text; ask your user before publishing it anywhere."
            )
            .as_str()
        ),
        "{out}"
    );
    assert!(!out.split_whitespace().any(|w| w == "gh"), "{out}");
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(v["unanswered"], json!([]), "{v}");
}

// --- killers from the adversarial review ---

#[test]
fn a_file_peek_prints_no_next() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let (code, out, err) = tmp.run(&["review", "pr.md", "--peek"], "");
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(tmp.claims("pr.md"), 0);
    assert!(out.contains("say why"), "{out}");
    assert!(
        !out.lines()
            .any(|l| l.starts_with("next:") || l.contains("<<'EOF'") || l.contains("--wait")),
        "{out}"
    );
}

/// A round with a note, a `later` on `## Details` and a `yes` on "detail one".
fn labelled() -> (Scenario, u32, u32, u32) {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let note = s.comment(s.quote("second item"), None, Some("say why"));
    let later = s.comment(s.section("## Details"), Some("later"), None);
    let yes = s.comment(s.quote("detail one"), Some("yes"), None);
    s.submit();
    (s, note, later, yes)
}

const LATER_LINE: &str = "LATER ## Details (whole section) — parked by the reviewer; leave it";
const YES_LINE: &str = "YES   \"detail one\" — keep as is";

#[test]
fn a_claimed_round_labels_later_and_yes_and_has_no_no_line() {
    let (s, note, later, yes) = labelled();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let lines: Vec<&str> = out.lines().collect();
    find(&lines, LATER_LINE, &out);
    find(&lines, YES_LINE, &out);
    for n in [later, yes] {
        assert!(
            !lines.iter().any(|l| l.starts_with(&format!("#{n} "))),
            "{out}"
        );
    }
    let note_line = format!("#{note} done");
    assert_eq!(heredoc(&out), [note_line.as_str(), TURN_NOTE], "{out}");
    let eof = lines.iter().position(|l| *l == "EOF").expect("a flush EOF");
    let mut want: Vec<String> = AFTER_EOF[..3].iter().map(|l| l.to_string()).collect();
    want.push("      3) Then wait again: birta review pr.md --wait".to_string());
    assert_eq!(lines[eof + 1..], want, "{out}");
}

#[test]
fn a_folder_claim_labels_later_and_yes_too() {
    let (s, ..) = labelled();
    let tmp = Tmp::new();
    let log = s
        .log()
        .replacen("\"op\":\"submit\"", "\"op\":\"submit\",\"batch\":7", 1);
    tmp.write("docs/pr.md", s.text());
    tmp.write("docs/pr.md.birta.jsonl", &log);
    let out = tmp.review(&["docs"]);
    let lines: Vec<&str> = out.lines().collect();
    find(&lines, LATER_LINE, &out);
    find(&lines, YES_LINE, &out);
}

#[test]
fn a_round_of_answers_only_counts_answers_in_the_header() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    s.human_reply(ask, None, Some("keep it"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let header = out.lines().next().unwrap();
    assert!(header.contains(" · 1 answer"), "{out}");
    assert!(!header.contains("0 notes"), "{out}");
    assert_eq!(heredoc(&out), [TURN_NOTE], "{out}");
}

#[test]
fn two_replies_to_one_ask_give_one_ans_line_with_the_latest() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    s.human_reply(ask, None, Some("nn"));
    let latest = s.human_reply(ask, None, Some("yes, 15 minutes"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let ans: Vec<&str> = out.lines().filter(|l| l.starts_with("ANS")).collect();
    assert_eq!(
        ans,
        ["ANS   your ask on \"detail two\" — yes, 15 minutes"],
        "{out}"
    );
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(v["answers"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["answers"][0]["n"], latest, "{v}");
}

#[test]
fn an_ask_answered_a_round_late_still_reaches_the_agent() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    s.comment(s.quote("third item"), None, Some("round two"));
    s.submit();
    let two = s.claim("claude");
    s.yield_turn("claude", two);
    s.human_reply(ask, None, Some("LATE-ANSWER"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let ans: Vec<&str> = out.lines().filter(|l| l.starts_with("ANS")).collect();
    assert_eq!(
        ans,
        ["ANS   your ask on \"detail two\" — LATE-ANSWER"],
        "{out}"
    );
}

#[test]
fn an_answer_in_a_submit_merged_into_a_later_claim_reaches_the_agent() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    s.human_reply(ask, None, Some("EARLY"));
    s.submit();
    s.comment(s.quote("third item"), None, Some("round two"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    assert!(
        out.lines()
            .any(|l| l.starts_with("ANS") && l.ends_with("— EARLY")),
        "{out}"
    );
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(v["answers"][0]["body"], "EARLY", "{v}");
}

#[test]
fn an_ask_answered_in_an_earlier_round_is_not_unanswered_on_approve() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    s.human_reply(ask, None, Some("keep"));
    s.submit();
    let two = s.claim("claude");
    s.yield_turn("claude", two);
    s.approve();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    assert!(!out.contains("unanswered"), "{out}");
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(v["unanswered"], json!([]), "{v}");
}

#[test]
fn a_reply_to_the_agents_plain_note_or_turn_note_reaches_it_as_re() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let first = s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let turn = s.claim("claude");
    s.done("claude", first, turn);
    let at = s.quote("detail two");
    let note = s.push(
        support::scenario::agent("claude"),
        json!({"op": "comment", "at": at, "body": "I left this", "turn": turn}),
    );
    let turn_note = s.turn_note("claude", "I kept the table.", turn);
    s.yield_turn("claude", turn);
    s.human_reply(note, None, Some("fine by me"));
    s.human_reply(turn_note, None, Some("good call"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let replies: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("RE ") || l.starts_with("ANS "))
        .collect();
    assert_eq!(
        replies,
        [
            "RE    your note on \"detail two\" — fine by me",
            "RE    your note on the whole doc — good call",
        ],
        "{out}"
    );
    assert!(
        out.lines()
            .next()
            .unwrap()
            .ends_with(" · 2 answers · drafted by claude"),
        "{out}"
    );
    let v = tmp.review_json(&["pr.md"]);
    let answers = v["answers"].as_array().unwrap();
    assert_eq!(answers.len(), 2, "{v}");
    assert_eq!(answers[0]["to"], note, "{v}");
    assert_eq!(answers[0]["kind"], "re", "{v}");
    assert_eq!(answers[1]["to"], turn_note, "{v}");
    assert_eq!(answers[1]["kind"], "re", "{v}");
    assert!(answers[1].get("quote").is_none(), "{v}");
}

/// Claude's ask on "detail two" in round one, then `answer` ops by the reviewer, then a
/// hand-back. Returns the scenario and the ask.
fn answered_with(answer: impl Fn(&mut Scenario, u32)) -> (Scenario, u32) {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    answer(&mut s, ask);
    s.submit();
    (s, ask)
}

fn ans_lines(s: &Scenario) -> (Vec<String>, Value) {
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", s);
    let out = tmp.review(&["pr.md"]);
    let ans = out
        .lines()
        .filter(|l| l.starts_with("ANS"))
        .map(str::to_string)
        .collect();
    (ans, tmp.review_json(&["pr.md"]))
}

#[test]
fn an_amended_answer_reads_as_amended() {
    let (s, _) = answered_with(|s, ask| {
        let r = s.human_reply(ask, None, Some("nn"));
        s.amend(r, Some("yes, 15"), None);
    });
    let (ans, v) = ans_lines(&s);
    assert_eq!(ans, ["ANS   your ask on \"detail two\" — yes, 15"]);
    assert_eq!(v["answers"][0]["body"], "yes, 15", "{v}");
    assert_eq!(v["answers"][0]["kind"], "answer", "{v}");

    let (s, _) = answered_with(|s, ask| {
        let r = s.human_reply(ask, Some("yes"), None);
        s.amend(r, None, Some("no"));
    });
    assert_eq!(ans_lines(&s).0, ["ANS   your ask on \"detail two\" — no"]);
}

#[test]
fn a_verdict_put_over_a_typed_answer_reads_as_the_verdict_alone() {
    // The page empties the words when a key answers over them
    let (s, _) = answered_with(|s, ask| {
        let r = s.human_reply(ask, None, Some("yes, 15"));
        s.amend(r, Some(""), Some("yes"));
    });
    let (ans, v) = ans_lines(&s);
    assert_eq!(ans, ["ANS   your ask on \"detail two\" — yes"]);
    assert_eq!(v["answers"][0]["verdict"], "yes", "{v}");
}

#[test]
fn an_amend_after_the_hand_back_is_a_draft_and_changes_nothing_sent() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask = asked_in_round_one(&mut s);
    let r = s.human_reply(ask, None, Some("nn"));
    s.submit();
    s.amend(r, Some("yes, 15"), None);
    assert_eq!(ans_lines(&s).0, ["ANS   your ask on \"detail two\" — nn"]);
}

#[test]
fn a_retracted_answer_drops_out_and_the_ask_is_unanswered_again() {
    let (s, ask) = answered_with(|s, ask| {
        let r = s.human_reply(ask, Some("yes"), None);
        s.retract(r);
    });
    let (ans, v) = ans_lines(&s);
    assert!(ans.is_empty(), "{ans:?}");
    assert_eq!(v["answers"], json!([]), "{v}");

    let mut s = Scenario::drafted_by(DOC, "claude");
    let ask2 = asked_in_round_one(&mut s);
    assert_eq!(ask, ask2);
    let r = s.human_reply(ask, Some("yes"), None);
    s.retract(r);
    s.approve();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(v["unanswered"][0]["to"], ask, "{v}");
    let state = s.fold();
    let item = state.items.iter().find(|i| i.n == ask).unwrap();
    assert!(
        item.replies.iter().all(|reply| reply.n != r),
        "{:?}",
        item.replies
    );
    assert_eq!(state.counts.drafts, 0, "a retracted reply is no draft");
}

#[test]
fn answers_read_as_their_verdict_words_and_doc_asks_name_the_doc() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    let first = s.comment(s.quote("second item"), None, Some("say why"));
    s.submit();
    let turn = s.claim("claude");
    s.done("claude", first, turn);
    let no = s.blank("claude", s.quote("detail one"), "cut it?", turn);
    let call = s.blank("claude", s.quote("detail two"), "which?", turn);
    let both = s.blank("claude", s.quote("third item"), "keep?", turn);
    let doc = s.blank("claude", json!("doc"), "ship it?", turn);
    s.yield_turn("claude", turn);
    s.human_reply(no, Some("no"), None);
    s.human_reply(call, Some("your_call"), None);
    s.human_reply(both, Some("yes"), Some("but shorter"));
    s.human_reply(doc, None, Some("after review"));
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let ans: Vec<&str> = out.lines().filter(|l| l.starts_with("ANS")).collect();
    for want in [
        "ANS   your ask on \"detail one\" — no",
        "ANS   your ask on \"detail two\" — your call",
        "ANS   your ask on \"third item\" — yes: but shorter",
        "ANS   your ask on the whole doc — after review",
    ] {
        assert!(ans.contains(&want), "{want:?} in\n{out}");
    }
    assert!(
        !out.contains("`no` means"),
        "an answer of no isn't a no note:\n{out}"
    );
    let v = tmp.review_json(&["pr.md"]);
    let answers = v["answers"].as_array().unwrap();
    let of = |to: u32| {
        answers
            .iter()
            .find(|a| a["to"] == to)
            .unwrap_or_else(|| panic!("no answer to {to}: {v}"))
    };
    assert_eq!(of(no)["verdict"], "no", "{v}");
    assert_eq!(of(call)["verdict"], "your_call", "{v}");
    assert!(of(doc).get("quote").is_none(), "{v}");
}

/// Claude's round on `pr.md` with `extra` applied beside the edit at the quoted spot.
fn respond_with(extra: impl Fn(&str) -> String) -> (String, String) {
    let (s, doc, note, claim) = held();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let text = extra(&s.replaced("- second item", "- second item, because S3 flakes"));
    tmp.edit("pr.md", &text, ts(claim) + 1_000);
    let (code, stdout, stderr) = tmp.respond(
        "pr.md",
        &format!("#{note} done\n#{doc} done\n@: I said why.\n"),
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    (stdout, stderr)
}

fn warned(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains("no note")).collect()
}

#[test]
fn n16_follows_what_renders_and_counts_every_uncited_change() {
    // Whitespace alone, but it renders a line break
    let (_, err) =
        respond_with(|t| t.replacen("Intro paragraph here,\n", "Intro paragraph here,  \n", 1));
    let w = warned(&err);
    assert_eq!(w.len(), 1, "{err}");
    assert!(w[0].starts_with("warning: 1 change cites no note"), "{err}");

    // A reflow renders the same
    let (out, err) = respond_with(|t| {
        t.replacen(
            "Intro paragraph here,\nwith a second line.",
            "Intro paragraph here, with a second line.",
            1,
        )
    });
    assert!(warned(&err).is_empty(), "{err}");
    assert_eq!(out.lines().last(), Some("next: birta review pr.md --wait"));

    // A pure deletion
    let (_, err) = respond_with(|t| t.replacen("- detail two\n", "", 1));
    let w = warned(&err);
    assert_eq!(w.len(), 1, "{err}");
    assert!(w[0].starts_with("warning: 1 change cites no note"), "{err}");
    assert!(w[0].contains("detail two"), "{err}");

    // Two
    let (_, err) = respond_with(|t| {
        t.replacen("- detail two\n", "- detail 2\n", 1).replacen(
            "- more item\n",
            "- more items\n",
            1,
        )
    });
    let w = warned(&err);
    assert_eq!(w.len(), 1, "{err}");
    assert!(w[0].starts_with("warning: 2 changes cite no note"), "{err}");
    assert!(
        w[0].contains("detail 2") && w[0].contains("more items"),
        "{err}"
    );
}

#[test]
fn n16_ignores_the_humans_own_edits_during_the_claim() {
    let (mut s, _, note, claim) = held();
    s.edit(&s.replaced("- more item", "- more items"));
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    tmp.edit(
        "pr.md",
        &s.replaced("- second item", "- second item, because S3 flakes"),
        ts(claim) + 60_000,
    );
    let (code, stdout, stderr) = tmp.respond("pr.md", &format!("#{note} done\n@: I said why.\n"));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(warned(&stderr).is_empty(), "{stderr}");
    assert_eq!(
        stdout.lines().last(),
        Some("next: birta review pr.md --wait"),
        "{stdout}"
    );
}

#[test]
fn a_late_respond_stays_one_line() {
    let (s, doc, note, claim) = held();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let (code, stdout, stderr) =
        tmp.respond("pr.md", &format!("#{note} done\n#{doc} done\n@: done.\n"));
    assert_eq!(code, 0, "{stdout}{stderr}");
    tmp.edit(
        "pr.md",
        &s.replaced("- detail two", "- detail 2"),
        ts(claim) + 600_000,
    );
    let (code, stdout, stderr) = tmp.respond("pr.md", "@: I also fixed a typo.\n");
    assert_eq!(code, 0, "{stdout}{stderr}");
    let said = format!("{stdout}{stderr}");
    assert!(said.contains("recorded late"), "{said}");
    assert_eq!(said.trim().lines().count(), 1, "one line: {said:?}");
}

/// The claim lines' `target` field in the review file of `rel`.
fn claim_targets(tmp: &Tmp, rel: &str) -> Vec<Value> {
    tmp.read(&format!("{rel}.birta.jsonl"))
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["op"] == "claim")
        .map(|v| v["target"].clone())
        .collect()
}

#[test]
fn a_folder_claim_records_its_folder_and_respond_points_there_from_any_cwd() {
    let tmp = Tmp::new();
    for rel in ["top.md", "docs/sub/c.md"] {
        let (s, _) = batched(&[(Some("second item"), None, "say why")], 7);
        tmp.put_doc(rel, &s);
    }
    let out = tmp.review(&["."]);
    let waits: Vec<&str> = out.lines().filter(|l| l.contains("--wait")).collect();
    assert_eq!(waits.len(), 1, "{out}");
    assert!(waits[0].ends_with("birta review . --wait"), "{out}");
    assert_eq!(claim_targets(&tmp, "top.md"), [json!(".")]);
    assert_eq!(claim_targets(&tmp, "docs/sub/c.md"), [json!("../..")]);

    let respond = "#4 done\n@: I said why.\n";
    let (code, stdout, stderr) = tmp.respond("docs/sub/c.md", respond);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(stdout.lines().last(), Some("next: birta review . --wait"));
    let (code, stdout, stderr) = tmp.run_in(
        "docs/sub",
        &["respond", "../../top.md", "--as", "claude"],
        respond,
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        stdout.lines().last(),
        Some("next: birta review ../.. --wait"),
        "{stdout}"
    );
}

#[test]
fn a_nested_file_of_a_folder_claim_points_at_the_claimed_folder() {
    let tmp = Tmp::new();
    for rel in ["docs/a.md", "docs/sub/c.md"] {
        let (s, _) = batched(&[(Some("second item"), None, "say why")], 7);
        tmp.put_doc(rel, &s);
    }
    tmp.review(&["docs"]);
    let (code, stdout, stderr) = tmp.respond("docs/sub/c.md", "#4 done\n@: I said why.\n");
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        stdout.lines().last(),
        Some("next: birta review docs --wait"),
        "{stdout}"
    );
}

#[test]
fn an_old_folder_claim_without_its_folder_is_found_by_walking_up() {
    let tmp = Tmp::new();
    for rel in ["top.md", "docs/sub/c.md"] {
        let (s, _) = batched(&[(Some("second item"), None, "say why")], 7);
        tmp.put_doc(rel, &s);
    }
    tmp.review(&["."]);
    for rel in ["top.md", "docs/sub/c.md"] {
        let log = tmp.read(&format!("{rel}.birta.jsonl"));
        let old: String = log
            .lines()
            .map(|l| {
                let mut v: Value = serde_json::from_str(l).unwrap();
                if v["op"] == "claim" {
                    v.as_object_mut().unwrap().remove("target");
                }
                format!("{v}\n")
            })
            .collect();
        tmp.write(&format!("{rel}.birta.jsonl"), &old);
    }
    let (code, stdout, stderr) = tmp.respond("docs/sub/c.md", "#4 done\n@: I said why.\n");
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(stdout.lines().last(), Some("next: birta review . --wait"));
}

#[test]
fn a_file_claim_of_a_batched_round_points_at_the_file() {
    let tmp = Tmp::new();
    for rel in ["docs/a.md", "docs/b.md"] {
        let (s, _) = batched(&[(Some("second item"), None, "say why")], 7);
        tmp.put_doc(rel, &s);
    }
    tmp.review(&["docs/a.md"]);
    assert_eq!(tmp.claims("docs/b.md"), 0);
    assert_eq!(claim_targets(&tmp, "docs/a.md"), [json!("a.md")]);
    let (code, stdout, stderr) = tmp.respond("docs/a.md", "#4 done\n@: I said why.\n");
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        stdout.lines().last(),
        Some("next: birta review docs/a.md --wait"),
        "{stdout}"
    );
}

#[test]
fn every_placeholder_the_template_prints_is_refused_by_respond() {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.comment(s.quote("Intro paragraph here"), Some("stet"), None);
    s.note("too long overall");
    s.comment(json!("doc"), Some("ask"), Some("why now?"));
    s.comment(s.quote("second item"), None, Some("say why"));
    s.comment(s.section("## Details"), Some("no"), None);
    s.comment(s.quote("detail two"), Some("ask"), Some("is this right?"));
    s.comment(s.section("## More"), Some("later"), None);
    s.comment(s.quote("appendix item"), Some("yes"), None);
    s.submit();
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &s);
    let out = tmp.review(&["pr.md"]);
    let asked = heredoc(&out);
    assert_eq!(
        asked.iter().filter(|l| l.contains(" answer: ")).count(),
        2,
        "one answer line per ask:\n{out}"
    );
    let mut checked = 0;
    let after: Vec<&str> = out.lines().skip_while(|l| *l != "EOF").skip(1).collect();
    let pieces = after.iter().flat_map(|l| l.split("  ")).map(str::trim);
    let lines = asked.iter().copied().chain(pieces);
    for line in lines.filter(|l| l.contains('<')) {
        let line = line.replace("#N ", "#6 ");
        if !(line.starts_with('#')
            || line.starts_with("ask ")
            || line.starts_with("unasked ")
            || line.starts_with("@:"))
        {
            continue;
        }
        checked += 1;
        match respond::parse(&line) {
            Ok(entries) => panic!("{line:?} was accepted: {entries:?}"),
            Err(errors) => assert!(
                errors
                    .iter()
                    .any(|e| e.msg.starts_with("placeholder left in")),
                "{line:?}: {errors:?}"
            ),
        }
    }
    assert!(
        checked >= 9,
        "only {checked} placeholder lines found:\n{out}"
    );
}

/// Like [`batched`], with the submit written at `at`.
fn batched_at(body: &str, batch: u64, at: u64) -> Scenario {
    let mut s = Scenario::new(DOC);
    let note = s.comment(s.quote("second item"), None, Some(body));
    s.raw(
        json!({"ts": at, "by": human(), "op": "submit", "upto": note, "not_before": at,
                 "hold_ms": 0, "to": "*", "batch": batch}),
    );
    s
}

#[test]
fn the_batch_line_shows_on_a_claim_with_the_latest_hand_back_time() {
    let tmp = Tmp::new();
    tmp.put_doc("docs/a.md", &batched_at("FROM-A", 7, ts(5)));
    tmp.put_doc("docs/b.md", &batched_at("FROM-B", 7, ts(5) + 120_000));
    let (code, peek, err) = tmp.run(&["review", "docs/", "--peek"], "");
    assert_eq!(code, 0, "{peek}{err}");
    let want = format!(
        "docs/ · handed back {} · 2 files · 2 notes",
        hhmm(ts(5) + 120_000)
    );
    assert_eq!(peek.lines().next(), Some(want.as_str()), "{peek}");
    let out = tmp.review(&["docs"]);
    assert_eq!(out.lines().next(), Some(want.as_str()), "{out}");
}

/// An approve by andres of `DOC` after one settled round.
fn approved_doc() -> Scenario {
    let mut s = Scenario::drafted_by(DOC, "claude");
    s.note("first");
    s.submit();
    let turn = s.claim("claude");
    s.yield_turn("claude", turn);
    s.approve();
    s
}

fn no_gh(out: &str) {
    assert!(!out.split_whitespace().any(|w| w == "gh"), "{out}");
}

#[test]
fn no_approved_view_names_gh() {
    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &approved_doc());
    for _ in 0..2 {
        // Claimed through the folder, then shown again as settled
        let out = tmp.review(&["."]);
        assert!(
            out.contains("birta show ") && out.contains("pr.md@"),
            "{out}"
        );
        no_gh(&out);
    }
    no_gh(&tmp.review(&["pr.md", "--json"]));

    tmp.write(
        "pr.md",
        &format!("{DOC}\nA line added after the approve.\n"),
    );
    let (code, out, err) = tmp.run(&["review", "pr.md", "--as", "claude"], "");
    assert_eq!(code, 3, "{out}{err}");
    assert!(out.contains("birta show pr.md@"), "{out}");
    no_gh(&out);
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[test]
fn json_only_adds_keys() {
    let tmp = ux_a_round_two();
    let v = tmp.review_json(&["pr-draft.md"]);
    assert_eq!(
        keys(&v),
        [
            "answers",
            "claim",
            "doc",
            "drafted_by",
            "handed_back",
            "items",
            "read_only",
            "round",
            "status",
            "you"
        ],
        "{v}"
    );
    assert_eq!(keys(&v["handed_back"]), ["by", "ts"], "{v}");
    assert_eq!(
        keys(&v["answers"][0]),
        ["body", "kind", "n", "quote", "to"],
        "{v}"
    );

    let tmp = Tmp::new();
    tmp.put_doc("pr.md", &approved_doc());
    let v = tmp.review_json(&["pr.md"]);
    assert_eq!(
        keys(&v),
        ["approve", "doc", "rev", "status", "unanswered"],
        "{v}"
    );

    let tmp = Tmp::new();
    for rel in ["docs/a.md", "docs/b.md"] {
        let (s, _) = batched(&[(Some("second item"), None, "say why")], 7);
        tmp.put_doc(rel, &s);
    }
    let v = tmp.review_json(&["docs"]);
    assert_eq!(keys(&v), ["docs", "status"], "{v}");
}
