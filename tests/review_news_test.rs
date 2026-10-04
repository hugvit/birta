//! Claude's news: the one list of what the reviewer has to check, `counts.to_check`, and
//! what each change carries for its row: the old and new text, its check, and why it
//! was flagged as unasked.

use birta::review::doc::Doc;
use birta::review::fold;
use birta::review::state::ReviewState;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, Scenario, T0, agent};

const CLAUDE: &str = "claude";

/// The journey's draft, and the text Claude's first write leaves.
const PR: &str = include_str!("../e2e/fixtures/review/journey/pr-draft.md");
const ROUND1: &str = include_str!("../e2e/fixtures/review/journey/round1.md");

fn id(op: u32, idx: u32) -> String {
    format!("{op}.{idx}")
}

fn hash(n: u32) -> String {
    format!("#{n}")
}

/// The state as the page receives it, so new fields are checked by their wire names.
fn wire(state: &ReviewState) -> Value {
    serde_json::to_value(state).unwrap()
}

fn news(v: &Value) -> &Vec<Value> {
    v["news"]
        .as_array()
        .unwrap_or_else(|| panic!("no news list: {}", v["news"]))
}

/// `(id, kind, sp)` of each news item, in list order.
fn listed(v: &Value) -> Vec<(String, String, Value)> {
    news(v)
        .iter()
        .map(|n| {
            (
                n["id"].as_str().unwrap_or_default().to_string(),
                n["kind"].as_str().unwrap_or_default().to_string(),
                n["sp"].clone(),
            )
        })
        .collect()
}

fn item(id: &str, kind: &str, sp: Value) -> (String, String, Value) {
    (id.to_string(), kind.to_string(), sp)
}

fn entry<'v>(v: &'v Value, id: &str) -> &'v Value {
    news(v)
        .iter()
        .find(|n| n["id"] == id)
        .unwrap_or_else(|| panic!("no news item {id} in {:?}", listed(v)))
}

/// A news item's check; `null` while unchecked.
fn check(v: &Value, id: &str) -> Value {
    let n = entry(v, id);
    assert!(n.get("check").is_some(), "{id} has no check field: {n}");
    n["check"].clone()
}

fn to_check(v: &Value) -> Value {
    v["counts"]["to_check"].clone()
}

fn change<'v>(v: &'v Value, id: &str) -> &'v Value {
    v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no change {id}"))
}

/// The sourcepos of the stop whose first line in `text` holds `needle`.
fn sp_of(text: &str, needle: &str) -> Value {
    let doc = Doc::new(text);
    let stop = doc
        .blocks
        .stops
        .iter()
        .find(|s| {
            text.lines()
                .nth(s.lines.0 as usize - 1)
                .is_some_and(|l| l.contains(needle))
        })
        .unwrap_or_else(|| panic!("no stop on a line with {needle:?}"));
    json!(stop.sp)
}

fn unasked_with(s: &mut Scenario, quote: &str, body: Option<&str>, turn: u32, late: bool) -> u32 {
    let mut op = json!({"op": "unasked", "quote": quote, "turn": turn});
    if let Some(body) = body {
        op["body"] = json!(body);
    }
    if late {
        op["late"] = json!(true);
    }
    s.push(agent(CLAUDE), op)
}

// --- the journey's first round, from try2 ---

/// The answer to "is 5 retries enough?" and the turn note, as the journey writes them.
const ANSWER: &str = "The default is 4 attempts (3 retries), not 5; with a 5 s cap that rides out typical blips and short deploys, and it's configurable. The integration suite hasn't been run yet, so that's still unverified.";
const TURN_NOTE: &str = "I trimmed the whole doc to roughly half, explained that we fail because put makes a single attempt with no retry logic, and dropped the Stacked on section.";

/// The reviewer's four notes, handed back, and Claude's first turn answering them.
struct Journey {
    s: Scenario,
    verbose: u32,
    say_why: u32,
    stacked: u32,
    write: u32,
    answer: u32,
    note: u32,
}

fn journey() -> Journey {
    journey_of(Scenario::drafted_by(PR, CLAUDE))
}

/// The journey's first round on `s`, a fresh scenario over the draft.
fn journey_of(mut s: Scenario) -> Journey {
    let verbose = s.note("way too verbose");
    let say_why = s.comment(s.quote("we just fail"), None, Some("say why"));
    let stacked = s.comment(s.section("## Stacked on"), Some("no"), None);
    let question = s.comment(
        s.block("Ran the integration suite"),
        Some("ask"),
        Some("is 5 retries enough?"),
    );
    s.submit();
    let turn = s.claim(CLAUDE);
    let write = s.external(ROUND1);
    s.done(CLAUDE, say_why, turn);
    s.done(CLAUDE, verbose, turn);
    for quote in [
        "makes a single attempt with no retry logic",
        "Retry on network errors, 429 and 502–504 with exponential backoff and jitter",
        "Honor `Retry-After`; defaults: 4 attempts, 200 ms base, 5 s cap",
        "Unit tests for backoff timing and retryable status codes",
    ] {
        s.covers(CLAUDE, verbose, quote, turn);
    }
    s.done(CLAUDE, stacked, turn);
    unasked_with(
        &mut s,
        "Respect `Retry-After` when the server sends it",
        Some("folded into the defaults bullet"),
        turn,
        false,
    );
    unasked_with(
        &mut s,
        "Log each retry at `debug` with attempt number and delay",
        Some("too much detail for a PR description"),
        turn,
        false,
    );
    let answer = s.agent_reply(CLAUDE, question, "answer", ANSWER);
    let note = s.turn_note(CLAUDE, TURN_NOTE, turn);
    s.yield_turn(CLAUDE, turn);
    Journey {
        s,
        verbose,
        say_why,
        stacked,
        write,
        answer,
        note,
    }
}

/// J-07: twelve items of news on nine stops. Removals are split at headings, so the
/// cut `Log each retry` bullet and the cut `## Stacked on` section are two items, and
/// two stops, in the same gap.
#[test]
fn the_journeys_reply_is_twelve_items_of_news_on_nine_stops() {
    let j = journey();
    let v = wire(&j.s.fold());
    let w = |idx| id(j.write, idx);
    let why = sp_of(ROUND1, "`UploadClient.put` makes");
    let merged = sp_of(ROUND1, "Retry on network errors");
    let honor = sp_of(ROUND1, "Honor `Retry-After`");
    let unit = sp_of(ROUND1, "Unit tests for backoff timing");
    let suite = sp_of(ROUND1, "Ran the integration suite");
    assert_eq!(
        listed(&v),
        [
            // `whole doc`: the reply under each whole-doc note, then the turn note last
            item(&hash(j.verbose), "reply", Value::Null),
            item(&hash(j.note), "turn_note", Value::Null),
            // then doc order, and on one stop the change before the reply
            item(&w(0), "change", why.clone()),
            item(&hash(j.say_why), "reply", why),
            item(&w(1), "change", merged.clone()),
            item(&w(2), "removal", Value::Null),
            item(&w(3), "change", honor.clone()),
            item(&w(4), "removal", Value::Null),
            item(&w(5), "removal", Value::Null),
            item(&hash(j.stacked), "reply", Value::Null),
            item(&w(6), "change", unit),
            item(&hash(j.answer), "answer", suite),
        ]
    );
    // Each removal sits in its gap: after the merged bullet, and twice after `Honor`
    assert_eq!(entry(&v, &w(2))["after_sp"], merged);
    assert_eq!(entry(&v, &w(4))["after_sp"], honor);
    assert_eq!(entry(&v, &w(5))["after_sp"], honor);
    for n in news(&v) {
        assert!(n.get("check").is_some_and(Value::is_null), "{n}");
    }
    // Nine stops: whole doc, Why, the merged bullet, the Respect cut, Honor, the Log
    // cut, the Stacked on cut (with its reply), the unit-test bullet, the suite item
    assert_eq!(to_check(&v), 9);
}

#[test]
fn the_journeys_changes_carry_their_checks_and_why_they_were_unasked() {
    let j = journey();
    let v = wire(&j.s.fold());
    let w = |idx| id(j.write, idx);
    for c in v["changes"].as_array().unwrap() {
        assert!(c.get("check").is_some_and(Value::is_null), "{c}");
        assert_eq!(c["is_rewrite"], false, "{c}");
    }
    assert_eq!(
        change(&v, &w(2))["unasked_why"],
        "folded into the defaults bullet"
    );
    assert_eq!(
        change(&v, &w(4))["unasked_why"],
        "too much detail for a PR description"
    );
    for idx in [0, 1, 3, 5, 6] {
        assert!(change(&v, &w(idx)).get("unasked_why").is_none(), "{idx}");
    }
    // The `no` on the section is answered by the section's own removal
    assert_eq!(change(&v, &w(5))["asked"]["notes"], json!([j.stacked]));
    assert_eq!(change(&v, &w(4))["asked"]["kind"], "unasked");
    // Every change has its own row, however much of the doc the turn touched
    assert!(v["rewrite"].is_null(), "{}", v["rewrite"]);
}

/// The journey's first reply as the page receives it, which the e2e row specs serve.
/// Rewrite with `BIRTA_UPDATE_FIXTURES=1`.
#[test]
fn the_journeys_first_reply_matches_the_committed_json() {
    let j = journey_of(Scenario::drafted_by(PR, CLAUDE).nameless());
    let mut state = j.s.fold();
    assert_eq!(state.rev, birta::rev::Rev::of(ROUND1.as_bytes()).as_str());
    state.server_now = T0 + 600_000;
    state.hand_off = "Take a look at my notes: `birta review round1.md`".to_string();
    let want = serde_json::to_string_pretty(&state).unwrap() + "\n";
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("e2e/fixtures/review/journey/round1.json");
    if std::env::var_os("BIRTA_UPDATE_FIXTURES").is_some_and(|v| v == "1") {
        fs::write(&path, &want).unwrap();
    }
    assert!(
        fs::read_to_string(&path).ok().as_deref() == Some(want.as_str()),
        "out of date: {}; rerun with BIRTA_UPDATE_FIXTURES=1",
        path.display()
    );
}

/// The texts each change and removal carries for its row, as the hunks recorded them.
#[test]
fn the_journeys_payload() {
    let j = journey();
    let v = wire(&j.s.fold());
    let changes: Vec<Value> = v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["op"] == j.write)
        .map(|c| json!({"id": c["id"], "old_text": c["old_text"], "new_text": c["new_text"]}))
        .collect();
    // The head's final line end is the page's to drop
    let deletions: Vec<Value> = v["deletions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["kind"] == "block")
        .map(|d| {
            json!({
                "change": d["change"],
                "head": d["head"].as_str().map(str::trim_end),
                "more": d["more"],
            })
        })
        .collect();
    let payload = json!({"changes": changes, "deletions": deletions});
    insta::assert_snapshot!(
        "journey_payload",
        serde_json::to_string_pretty(&payload).unwrap()
    );
}

/// The text the reviewer reverts the merged bullet to.
fn bullets_back(text: &str) -> String {
    text.replace(
        "- Retry on network errors, 429 and 502–504 with exponential backoff and jitter\n",
        "- Wrap `UploadClient.put` in a retry loop with exponential backoff and full jitter\n- Retry only on idempotent failure modes: network errors, 502, 503, 504, and 429\n",
    )
}

/// J-08 to J-12: each stop's check takes one off `to_check`, and checked items stay in
/// the list until the next hand-back.
#[test]
fn the_journeys_nine_stops_are_checked_one_at_a_time() {
    let mut j = journey();
    let w = |idx| id(j.write, idx);
    let steps: [(&[String], u32); 9] = [
        (&[hash(j.verbose), hash(j.note)], 8),
        (&[w(0), hash(j.say_why)], 7),
        (&[], 6),
        (&[w(2)], 5),
        (&[w(3)], 4),
        (&[w(4)], 3),
        (&[w(5), hash(j.stacked)], 2),
        (&[w(6)], 1),
        (&[hash(j.answer)], 0),
    ];
    for (k, (ids, left)) in steps.iter().enumerate() {
        if ids.is_empty() {
            // J-10: `u` on the merged bullets
            let text = bullets_back(j.s.text());
            j.s.undo(&w(1), &text);
        } else {
            j.s.seen(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        }
        let v = wire(&j.s.fold());
        assert_eq!(to_check(&v), *left, "after step {k}: {:?}", listed(&v));
    }
    let v = wire(&j.s.fold());
    assert_eq!(news(&v).len(), 12, "{:?}", listed(&v));
    assert_eq!(check(&v, &hash(j.verbose)), "ok");
    assert_eq!(check(&v, &hash(j.note)), "ok");
    assert_eq!(check(&v, &w(0)), "kept");
    assert_eq!(check(&v, &hash(j.say_why)), "ok");
    assert_eq!(check(&v, &w(1)), "reverted");
    assert_eq!(check(&v, &w(4)), "kept");
    assert_eq!(check(&v, &hash(j.answer)), "ok");
    assert_eq!(change(&v, &w(0))["check"], "kept");
    assert_eq!(change(&v, &w(1))["check"], "reverted");
    assert_eq!(change(&v, &w(5))["check"], "kept");
}

/// J-13 to J-15: the hand-back settles round one; round two's reply and turn note are
/// the only news, and the approve settles them.
#[test]
fn the_hand_back_settles_round_one_and_round_two_is_two_stops() {
    let mut j = journey();
    let w = |idx| id(j.write, idx);
    j.s.seen(&[&hash(j.verbose), &hash(j.note)]);
    j.s.seen(&[&w(0), &hash(j.say_why)]);
    let text = bullets_back(j.s.text());
    j.s.undo(&w(1), &text);
    for ids in [
        vec![w(2)],
        vec![w(3)],
        vec![w(4)],
        vec![w(5), hash(j.stacked)],
        vec![w(6)],
        vec![hash(j.answer)],
    ] {
        j.s.seen(&ids.iter().map(String::as_str).collect::<Vec<_>>());
    }
    let at = j.s.block("Wrap `UploadClient.put`");
    let keep = j.s.comment(at, None, Some("keep these two separate"));
    j.s.submit();
    let v = wire(&j.s.fold());
    assert!(
        listed(&v).is_empty(),
        "settled by the hand-back: {:?}",
        listed(&v)
    );
    assert_eq!(to_check(&v), 0);

    let turn = j.s.claim(CLAUDE);
    let reply =
        j.s.agent_reply(CLAUDE, keep, "answer", "I'll leave them separate.");
    let note = j.s.turn_note(
        CLAUDE,
        "I left both bullets as they were and kept the rest.",
        turn,
    );
    j.s.yield_turn(CLAUDE, turn);
    let v = wire(&j.s.fold());
    let wrap = sp_of(j.s.text(), "Wrap `UploadClient.put`");
    assert_eq!(
        listed(&v),
        [
            item(&hash(note), "turn_note", Value::Null),
            // An `answer:` to a plain note is a reply
            item(&hash(reply), "reply", wrap),
        ]
    );
    assert_eq!(to_check(&v), 2);
    j.s.seen(&[&hash(note)]);
    assert_eq!(to_check(&wire(&j.s.fold())), 1);
    j.s.seen(&[&hash(reply)]);
    let v = wire(&j.s.fold());
    assert_eq!(to_check(&v), 0);
    assert_eq!(news(&v).len(), 2);
    j.s.approve();
    let v = wire(&j.s.fold());
    assert!(
        listed(&v).is_empty(),
        "settled by the approve: {:?}",
        listed(&v)
    );
    // The settled revert is still a revert
    assert_eq!(change(&v, &w(1))["check"], "reverted");
}

// --- settling ---

/// An agent's turn rewording `- third item` and replying to a whole-doc note, on DOC.
fn reworded() -> (Scenario, u32, u32) {
    let mut s = Scenario::new(DOC);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- third item", "- third item, reworded");
    let reply = s.agent_reply(CLAUDE, x, "answer", "tightened the list");
    s.yield_turn(CLAUDE, turn);
    (s, ext, reply)
}

#[test]
fn a_check_holds_its_item_until_a_later_hand_back_settles_it() {
    let (mut s, ext, reply) = reworded();
    let v = wire(&s.fold());
    let third = sp_of(s.text(), "- third item, reworded");
    assert_eq!(
        listed(&v),
        [
            item(&hash(reply), "reply", Value::Null),
            item(&id(ext, 0), "change", third),
        ]
    );
    assert_eq!(to_check(&v), 2);

    // A hand-back settles only what was checked before it
    s.note("one more thing");
    s.submit();
    let v = wire(&s.fold());
    assert_eq!(news(&v).len(), 2, "unchecked news outlives a hand-back");
    assert_eq!(to_check(&v), 2);

    s.seen(&[&id(ext, 0), &hash(reply)]);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &id(ext, 0)), "kept");
    assert_eq!(check(&v, &hash(reply)), "ok");
    assert_eq!(change(&v, &id(ext, 0))["check"], "kept");
    assert_eq!(to_check(&v), 0);
    assert_eq!(
        news(&v).len(),
        2,
        "checked news stays until the next hand-back"
    );

    s.note("and another");
    s.submit();
    assert!(news(&wire(&s.fold())).is_empty());
}

#[test]
fn a_withdrawn_hand_back_settles_nothing() {
    let (mut s, ext, reply) = reworded();
    s.seen(&[&id(ext, 0), &hash(reply)]);
    s.note("one more thing");
    let sub = s.submit_held();
    s.withdraw(sub);
    let v = wire(&s.fold());
    assert_eq!(news(&v).len(), 2, "{:?}", listed(&v));
    assert_eq!(check(&v, &id(ext, 0)), "kept");
    s.submit();
    assert!(news(&wire(&s.fold())).is_empty());
}

#[test]
fn an_approve_settles_what_was_checked() {
    let (mut s, ext, reply) = reworded();
    s.seen(&[&id(ext, 0)]);
    s.approve();
    let v = wire(&s.fold());
    assert_eq!(
        listed(&v),
        [item(&hash(reply), "reply", Value::Null)],
        "the unchecked reply stays"
    );
    assert_eq!(to_check(&v), 1);
}

// --- what checks an item ---

#[test]
fn an_undo_is_reverted_and_outranks_a_keep() {
    // `y`, then `u`
    let (mut s, ext, _) = reworded();
    s.seen(&[&id(ext, 0)]);
    s.undo(&id(ext, 0), DOC);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &id(ext, 0)), "reverted");
    assert_eq!(change(&v, &id(ext, 0))["check"], "reverted");

    // `u` alone
    let (mut s, ext, reply) = reworded();
    s.seen(&[&hash(reply)]);
    s.undo(&id(ext, 0), DOC);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &id(ext, 0)), "reverted");
    assert_eq!(to_check(&v), 0);
}

#[test]
fn a_reply_never_clears_a_revert() {
    let (mut s, ext, _) = reworded();
    s.undo(&id(ext, 0), DOC);
    // `c` on the reverted change, then `y` on it
    let at = s.block("- third item");
    s.comment(at, None, Some("keep the old wording"));
    s.seen(&[&id(ext, 0)]);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &id(ext, 0)), "reverted");
    assert_eq!(change(&v, &id(ext, 0))["check"], "reverted");
}

/// `c` on a change writes a note on the block holding it, as the server places a note
/// on a change.
#[test]
fn a_note_on_a_changed_block_replies_to_the_change() {
    let (mut s, ext, reply) = reworded();
    s.seen(&[&hash(reply)]);
    // A note elsewhere checks nothing
    let at = s.block("- first item");
    s.comment(at, None, Some("unrelated"));
    let v = wire(&s.fold());
    assert!(
        check(&v, &id(ext, 0)).is_null(),
        "{}",
        entry(&v, &id(ext, 0))
    );
    assert_eq!(to_check(&v), 1);

    let at = s.block("- third item, reworded");
    s.comment(at, None, Some("why reword it?"));
    let v = wire(&s.fold());
    assert_eq!(check(&v, &id(ext, 0)), "replied");
    assert_eq!(change(&v, &id(ext, 0))["check"], "replied");
    assert_eq!(to_check(&v), 0);
    // Your own notes are not news
    assert_eq!(news(&v).len(), 2, "{:?}", listed(&v));

    // A reply outranks a keep
    s.seen(&[&id(ext, 0)]);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &id(ext, 0)), "replied");
    assert_eq!(change(&v, &id(ext, 0))["check"], "replied");
}

#[test]
fn a_reply_to_claudes_reply_or_turn_note_is_replied() {
    let mut s = Scenario::new(DOC);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let reply = s.agent_reply(CLAUDE, x, "answer", "it is tight already");
    let note = s.turn_note(CLAUDE, "Left it as it was.", turn);
    s.yield_turn(CLAUDE, turn);
    s.human_reply(x, None, Some("it is not"));
    s.human_reply(note, None, Some("thanks"));
    let v = wire(&s.fold());
    assert_eq!(check(&v, &hash(reply)), "replied");
    assert_eq!(check(&v, &hash(note)), "replied");
    assert_eq!(to_check(&v), 0);
}

#[test]
fn an_answered_question_leaves_to_check_but_stays_in_the_list() {
    let mut s = Scenario::new(DOC);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- third item", "- third item, reworded");
    let at = s.block("- third item, reworded");
    let blank = s.blank(CLAUDE, at, "keep both clauses?", turn);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let third = sp_of(s.text(), "- third item, reworded");
    assert_eq!(
        listed(&v),
        [
            item(&hash(x), "reply", Value::Null),
            item(&id(ext, 0), "change", third.clone()),
            item(&hash(blank), "question", third),
        ]
    );
    // A question is a stop of its own, under its block
    assert_eq!(to_check(&v), 3);

    s.seen(&[&hash(x), &id(ext, 0), &hash(blank)]);
    let v = wire(&s.fold());
    assert!(
        check(&v, &hash(blank)).is_null(),
        "only an answer checks a question"
    );
    assert_eq!(to_check(&v), 1);

    s.human_reply(blank, None, Some("yes"));
    let v = wire(&s.fold());
    assert_eq!(check(&v, &hash(blank)), "answered");
    assert_eq!(to_check(&v), 0);
    assert_eq!(news(&v).len(), 3);
    s.submit();
    assert!(news(&wire(&s.fold())).is_empty());
}

#[test]
fn a_note_the_turn_left_unhandled_is_news_until_seen_by_its_row_id() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let nh = format!("nh:{x}");
    assert_eq!(
        listed(&v),
        [item(&nh, "not_handled", sp_of(s.text(), "- first item"))]
    );
    assert_eq!(to_check(&v), 1);
    s.seen(&[&nh]);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &nh), "ok");
    assert_eq!(to_check(&v), 0);
}

/// A bare `done` is Claude's reply too, under the note it answers, checked by the
/// note's own id.
#[test]
fn a_done_on_a_note_is_claudes_reply_at_the_note() {
    let mut s = Scenario::new(DOC);
    let x = s.verdict("- first item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(
        listed(&v),
        [item(&hash(x), "reply", sp_of(s.text(), "- first item"))]
    );
    assert_eq!(to_check(&v), 1);
    s.seen(&[&hash(x)]);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &hash(x)), "ok");
    assert_eq!(to_check(&v), 0);
}

// --- kinds ---

/// Two `answer:` lines and a `didnt:`: only the one to your question is an answer.
#[test]
fn a_reply_is_an_answer_only_to_your_question() {
    let mut s = Scenario::new(DOC);
    let q = s.comment(
        s.block("- first item"),
        Some("ask"),
        Some("is this needed?"),
    );
    let c = s.comment(s.block("- second item"), None, Some("reword this"));
    let d = s.verdict("- third item", "no");
    s.submit();
    let turn = s.claim(CLAUDE);
    let to_q = s.agent_reply(CLAUDE, q, "answer", "yes, for the CLI");
    let to_c = s.agent_reply(CLAUDE, c, "answer", "it reads fine as is");
    let to_d = s.agent_reply(CLAUDE, d, "didnt", "it is still used");
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(entry(&v, &hash(to_q))["kind"], "answer");
    assert_eq!(entry(&v, &hash(to_c))["kind"], "reply");
    assert_eq!(entry(&v, &hash(to_d))["kind"], "reply");
}

#[test]
fn only_claudes_and_outside_changes_are_news() {
    let mut s = Scenario::new(DOC);
    let outside = s.replace_line("- appendix item", "- appendix item, outside");
    let mine = s.edit(&s.replaced("- first item", "- first item, mine"));
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    // Renders the same: no change at all
    let blank = s.external(&s.text().replace("\n\n## More", "\n\n\n## More"));
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let ids: Vec<String> = listed(&v).into_iter().map(|(id, ..)| id).collect();
    assert!(ids.contains(&id(outside, 0)), "{ids:?}");
    assert!(!ids.contains(&id(mine, 0)), "{ids:?}");
    assert!(
        !ids.iter().any(|i| i.starts_with(&format!("{blank}."))),
        "{ids:?}"
    );
    assert_eq!(entry(&v, &id(outside, 0))["kind"], "change");
}

/// A folder's notes are on `whole doc`: Claude's replies to them, then its turn note.
#[test]
fn a_reply_to_a_folder_note_is_news_before_the_turn_note() {
    let mut s = Scenario::folder("docs");
    let a = s.comment(json!("dir"), None, Some("the sub/ docs repeat the index"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let reply = s.agent_reply(CLAUDE, a, "answer", "merged them into the index");
    let note = s.turn_note(CLAUDE, "Merged the sub docs.", turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&fold::fold_dir(&s.parsed()));
    assert_eq!(
        listed(&v),
        [
            item(&hash(reply), "reply", Value::Null),
            item(&hash(note), "turn_note", Value::Null),
        ]
    );
    assert_eq!(to_check(&v), 1);
}

#[test]
fn every_turns_note_is_news_by_its_own_line_until_settled() {
    let mut s = Scenario::new(DOC);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.done(CLAUDE, x, turn);
    let first = s.turn_note(CLAUDE, "Tightened it.", turn);
    s.yield_turn(CLAUDE, turn);
    s.seen(&[&hash(x), &hash(first)]);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &hash(first)), "ok");
    assert_eq!(to_check(&v), 0);

    let y = s.note("now the intro");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.done(CLAUDE, y, turn);
    let second = s.turn_note(CLAUDE, "Shortened the intro.", turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(
        listed(&v),
        [
            item(&hash(y), "reply", Value::Null),
            item(&hash(second), "turn_note", Value::Null),
        ]
    );
}

// --- the change payload ---

#[test]
fn unasked_why_is_the_body_of_the_unasked_line_on_time_or_late() {
    let mut s = Scenario::new(DOC);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = DOC
        .replace("- first item\n", "- first item, reworded\n")
        .replace("- third item\n", "")
        .replace("- appendix item\n", "- appendix item, reworded\n");
    let ext = s.external(&text);
    s.done(CLAUDE, x, turn);
    unasked_with(&mut s, "first item, reworded", None, turn, false);
    s.yield_turn(CLAUDE, turn);
    unasked_with(
        &mut s,
        "third item",
        Some("it repeated the second"),
        turn,
        true,
    );
    let v = wire(&s.fold());
    let mine: Vec<&Value> = v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["op"] == ext)
        .collect();
    assert_eq!(mine.len(), 3, "setup: {mine:#?}");
    let removal = mine.iter().find(|c| c["new_text"] == "").unwrap();
    assert_eq!(removal["unasked_why"], "it repeated the second");
    for c in mine.iter().filter(|c| c["new_text"] != "") {
        // One flagged without a reason, one not flagged at all
        assert!(c.get("unasked_why").is_none(), "{c}");
    }
}

#[test]
fn old_and_new_text_are_the_hunks_lines_with_their_ends() {
    let added = DOC.replace("- third item\n", "- third item\n- fourth item\n");
    let cut = DOC.replace("- third item\n", "");
    let cases = [
        (
            "an edit, CRLF",
            "# T\r\n\r\nold line\r\n\r\nEnd.\r\n",
            "# T\r\n\r\nnew line\r\n\r\nEnd.\r\n",
            "old line\r\n",
            "new line\r\n",
        ),
        ("an addition", DOC, added.as_str(), "", "- fourth item\n"),
        ("a removal", DOC, cut.as_str(), "- third item\n", ""),
    ];
    let mut bad = Vec::new();
    for (name, base, new, old_text, new_text) in cases {
        let mut s = Scenario::new(base);
        s.note("look");
        s.submit();
        let turn = s.claim(CLAUDE);
        let ext = s.external(new);
        s.yield_turn(CLAUDE, turn);
        let v = wire(&s.fold());
        let c = change(&v, &id(ext, 0));
        if (c["old_text"].clone(), c["new_text"].clone()) != (json!(old_text), json!(new_text)) {
            bad.push(format!("{name}: {} / {}", c["old_text"], c["new_text"]));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn a_reverted_change_keeps_claudes_text() {
    let (mut s, ext, _) = reworded();
    s.undo(&id(ext, 0), DOC);
    let v = wire(&s.fold());
    let c = change(&v, &id(ext, 0));
    assert_eq!(c["old_text"], "- third item\n");
    assert_eq!(c["new_text"], "- third item, reworded\n");
}

// --- adversary ---

/// The note that asked for a change is its cause, not a reply to it: only a note made
/// after the change checks it.
#[test]
fn the_note_that_asked_for_a_change_does_not_check_it() {
    let mut s = Scenario::new(DOC);
    let at = s.block("- third item");
    let asked = s.comment(at, None, Some("reword this"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.replace_line("- third item", "- third item, reworded");
    s.done(CLAUDE, asked, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert!(
        check(&v, &id(ext, 0)).is_null(),
        "{}",
        entry(&v, &id(ext, 0))
    );
    assert!(change(&v, &id(ext, 0))["check"].is_null());
    assert_eq!(to_check(&v), 1, "{:?}", listed(&v));
}

/// `y` on Claude's reply alone leaves its turn note to check.
#[test]
fn seeing_a_reply_leaves_the_turn_note_unchecked() {
    let mut s = Scenario::new(DOC);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let reply = s.agent_reply(CLAUDE, x, "answer", "it is tight already");
    let note = s.turn_note(CLAUDE, "Left it as it was.", turn);
    s.yield_turn(CLAUDE, turn);
    s.seen(&[&hash(reply)]);
    let v = wire(&s.fold());
    assert_eq!(check(&v, &hash(reply)), "ok");
    assert!(
        check(&v, &hash(note)).is_null(),
        "{}",
        entry(&v, &hash(note))
    );
    assert_eq!(to_check(&v), 1);
}

#[test]
fn a_withdrawn_approve_settles_nothing() {
    let (mut s, ext, reply) = reworded();
    s.seen(&[&id(ext, 0), &hash(reply)]);
    let approve = s.approve_held();
    s.withdraw(approve);
    let v = wire(&s.fold());
    assert_eq!(news(&v).len(), 2, "{:?}", listed(&v));
    assert_eq!(check(&v, &id(ext, 0)), "kept");
}

/// Whatever the respond keyword, a reply to your question is its answer.
#[test]
fn any_reply_to_your_question_is_an_answer() {
    let mut s = Scenario::new(DOC);
    let q1 = s.comment(
        s.block("- first item"),
        Some("ask"),
        Some("is this needed?"),
    );
    let q2 = s.comment(s.block("- second item"), Some("ask"), Some("and this?"));
    s.submit();
    let turn = s.claim(CLAUDE);
    let didnt = s.agent_reply(CLAUDE, q1, "didnt", "left it, the CLI uses it");
    let note = s.agent_reply(CLAUDE, q2, "note", "yes, the index links it");
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    assert_eq!(entry(&v, &hash(didnt))["kind"], "answer");
    assert_eq!(entry(&v, &hash(note))["kind"], "answer");
}

/// Each question is a stop of its own, even two under one block.
#[test]
fn two_questions_under_one_block_are_two_stops() {
    let mut s = Scenario::new(DOC);
    let x = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let at = s.block("- first item");
    let one = s.blank(CLAUDE, at.clone(), "keep it?", turn);
    let two = s.blank(CLAUDE, at, "or merge it?", turn);
    s.done(CLAUDE, x, turn);
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let first = sp_of(s.text(), "- first item");
    assert_eq!(
        listed(&v),
        [
            item(&hash(x), "reply", Value::Null),
            item(&hash(one), "question", first.clone()),
            item(&hash(two), "question", first),
        ]
    );
    assert_eq!(to_check(&v), 3);
    s.human_reply(one, None, Some("yes"));
    assert_eq!(to_check(&wire(&s.fold())), 2);
}

/// A removal of the doc's first block sits in the gap at the top.
#[test]
fn a_removal_at_the_top_has_no_stop_before_it() {
    let base = "Gone para.\n\n# T\n\nText.\n";
    let mut s = Scenario::new(base);
    s.note("look");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external("# T\n\nText.\n");
    s.yield_turn(CLAUDE, turn);
    let v = wire(&s.fold());
    let n = entry(&v, &id(ext, 0));
    assert_eq!(n["kind"], "removal", "{n}");
    assert!(n["sp"].is_null(), "{n}");
    assert_eq!(n.get("after_sp"), Some(&Value::Null), "{n}");
}

/// Stops are the current text's: a later write that moves the lines moves them too.
#[test]
fn news_sits_on_the_current_texts_stops() {
    let mut s = Scenario::new(DOC);
    s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    let text = DOC
        .replace("- detail two\n", "")
        .replace("- more item\n", "- more item, reworded\n");
    let ext = s.external(&text);
    s.yield_turn(CLAUDE, turn);
    let written = s.text().to_string();
    s.edit(&s.replaced("with a second line.", "with a second line,\nand a third."));
    assert_ne!(
        sp_of(&written, "- detail one"),
        sp_of(s.text(), "- detail one"),
        "setup: the edit moves the stops"
    );
    let v = wire(&s.fold());
    let mine: Vec<&Value> = news(&v)
        .iter()
        .filter(|n| {
            n["id"]
                .as_str()
                .is_some_and(|i| i.starts_with(&format!("{ext}.")))
        })
        .collect();
    let removal = mine
        .iter()
        .find(|n| n["kind"] == "removal")
        .expect("a removal");
    let reworded = mine
        .iter()
        .find(|n| n["kind"] == "change")
        .expect("a change");
    assert_eq!(removal["after_sp"], sp_of(s.text(), "- detail one"));
    assert_eq!(reworded["sp"], sp_of(s.text(), "- more item, reworded"));
}

// --- repair ---

/// A removal you put back is on its block again, and `c` there replies to it.
#[test]
fn a_reverted_removal_leaves_its_gap_for_the_block_it_put_back() {
    let base = "# T\n\nKeep.\n\nGone para.\n\nEnd.\n";
    let mut s = Scenario::new(base);
    s.note("look");
    s.submit();
    let turn = s.claim(CLAUDE);
    let ext = s.external("# T\n\nKeep.\n\nEnd.\n");
    s.yield_turn(CLAUDE, turn);
    s.undo(&id(ext, 0), base);
    let at = s.block("Gone para.");
    let note = s.comment(at, None, Some("keep it out"));
    let v = wire(&s.fold());
    let n = entry(&v, &id(ext, 0));
    assert_eq!(n["sp"], sp_of(s.text(), "Gone para."), "{n}");
    assert_eq!(n["check"], "reverted", "{n}");
    let it = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["n"] == note)
        .unwrap();
    assert_eq!(it["at"]["change"], id(ext, 0), "{it}");
}

/// Your revert's text, edited afterwards, still holds the reverted change.
#[test]
fn a_revert_edited_afterwards_keeps_its_change_on_the_block() {
    let (mut s, ext, _) = reworded();
    s.undo(&id(ext, 0), DOC);
    let text = s.replaced("- third item", "- third item, tweaked");
    s.edit(&text);
    let at = s.block("- third item, tweaked");
    let note = s.comment(at, None, Some("keep the old wording"));
    let v = wire(&s.fold());
    let n = entry(&v, &id(ext, 0));
    assert_eq!(n["sp"], sp_of(s.text(), "- third item, tweaked"), "{n}");
    let it = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["n"] == note)
        .unwrap();
    assert_eq!(it["at"]["change"], id(ext, 0), "{it}");
}
