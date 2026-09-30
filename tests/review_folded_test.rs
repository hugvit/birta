use std::fs;
use std::path::{Path, PathBuf};

use birta::rev::Rev;
use birta::review::fold::{self, Eol, HeadInfo, RoundKind};
use birta::review::state::{AtKind, ItemStatus, ReviewState, Stage};
use birta::review::{self, FOLDER_REVIEW};
use serde_json::json;

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, HOLD_MS, Scenario, human, ts};

const CLAUDE: &str = "claude";

fn head_info(text: &str) -> HeadInfo {
    let (parsed, chain, doc) = Scenario::new(text).build();
    fold::fold(&parsed, &chain, &doc).head_info()
}

// --- the Folded API ---

#[test]
fn claimable_round_follows_the_hold_like_scan() {
    let mut s = Scenario::new(DOC);
    s.note("first");
    let sub = s.submit_held();
    let (parsed, chain, doc) = s.build();
    let folded = fold::fold(&parsed, &chain, &doc);
    let index = fold::scan(&parsed, 0);
    let due = ts(sub) + HOLD_MS;
    for now in [ts(sub), due - 1, due, due + 60_000] {
        assert_eq!(
            folded.claimable_round(now),
            index.claimable_round(now),
            "{now}"
        );
    }
    assert_eq!(folded.claimable_round(due - 1), None);
    assert_eq!(folded.claimable_round(due), Some(sub));

    s.claim_round(CLAUDE, sub);
    let (parsed, chain, doc) = s.build();
    assert_eq!(fold::fold(&parsed, &chain, &doc).claimable_round(due), None);
}

#[test]
fn round_names_a_hand_back_or_an_approve() {
    let mut s = Scenario::new(DOC);
    let note = s.note("first");
    let sub = s.submit();
    let claim = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, claim);
    let approve = s.approve();
    let (parsed, chain, doc) = s.build();
    let folded = fold::fold(&parsed, &chain, &doc);

    let r = folded.round(sub).expect("the submit");
    assert_eq!(
        (r.n, r.kind, r.claim),
        (sub, RoundKind::Submit, Some(claim))
    );
    assert_eq!(folded.round(approve).unwrap().kind, RoundKind::Approve);
    assert!(folded.round(note).is_none());
    assert!(folded.round(999).is_none());
}

#[test]
fn notes_covered_by_lists_what_a_round_sent() {
    let mut s = Scenario::new(DOC);
    let a = s.note("a");
    let b = s.verdict("- first item", "no");
    let c = s.note("taken back");
    s.retract(c);
    let first = s.submit();
    let d = s.note("d");
    let second = s.submit_held();
    let e = s.note("e");
    let approve = s.approve();
    let (parsed, chain, doc) = s.build();
    let folded = fold::fold(&parsed, &chain, &doc);
    let covered = |round| {
        folded
            .notes_covered_by(round)
            .iter()
            .map(|i| i.n)
            .collect::<Vec<_>>()
    };
    assert_eq!(covered(first), [a, b]);
    assert_eq!(covered(second), [d]);
    assert_eq!(covered(approve), [e]);
    assert!(covered(a).is_empty());
}

#[test]
fn text_at_gives_the_text_of_every_recorded_rev() {
    let mut s = Scenario::new(DOC);
    let edited = s.replaced("- first item", "- first item, reworded");
    s.external(&edited);
    let (parsed, chain, doc) = s.build();
    let folded = fold::fold(&parsed, &chain, &doc);
    let before = Rev::of(DOC.as_bytes());
    let after = Rev::of(edited.as_bytes());
    assert_eq!(folded.text_at(&before).as_deref(), Some(DOC));
    assert_eq!(folded.text_at(&after).as_deref(), Some(edited.as_str()));
    assert_eq!(
        folded.text_bytes_at(&before).as_deref(),
        Some(DOC.as_bytes())
    );
    assert!(folded.text_at(&Rev::of(b"never logged")).is_none());
    assert!(folded.text_bytes_at(&Rev::of(b"never logged")).is_none());
}

#[test]
fn text_bytes_at_keeps_a_bom() {
    let text = "\u{feff}# T\n\nbody\n";
    let (parsed, chain, doc) = Scenario::new(text).build();
    let folded = fold::fold(&parsed, &chain, &doc);
    let bytes = folded.text_bytes_at(&Rev::of(text.as_bytes())).unwrap();
    assert_eq!(&bytes[..3], b"\xEF\xBB\xBF");
    assert_eq!(bytes, text.as_bytes());
}

#[test]
fn head_info_reads_the_bom_and_line_endings() {
    let crlf = "\u{feff}# T\r\n\r\nbody\r\n";
    let info = head_info(crlf);
    assert_eq!(info.rev, Rev::of(crlf.as_bytes()));
    assert!(info.bom);
    assert_eq!(info.eol, Eol::Crlf);

    let lf = head_info("# T\n\nbody\n");
    assert!(!lf.bom);
    assert_eq!(lf.eol, Eol::Lf);
    assert_eq!(head_info("# T\r\n\nbody\n").eol, Eol::Mixed);
    assert_eq!(head_info("# T").eol, Eol::None);
}

#[test]
fn head_info_is_the_current_texts() {
    let mut s = Scenario::new(DOC);
    let crlf = DOC.replace('\n', "\r\n");
    s.external(&crlf);
    let (parsed, chain, doc) = s.build();
    let info = fold::fold(&parsed, &chain, &doc).head_info();
    assert_eq!(info.rev, Rev::of(crlf.as_bytes()));
    assert_eq!(info.eol, Eol::Crlf);
}

// --- summarize and the review file's names ---

/// A replied turn with a change, written to disk as `pr.md` and its review file.
fn on_disk(dir: &Path) -> (Scenario, PathBuf, PathBuf) {
    let mut s = Scenario::new(DOC);
    s.verdict("- first item", "no");
    let q = s.note("tighten it");
    s.submit();
    let turn = s.claim(CLAUDE);
    s.replace_line("- third item", "- third item, reworded");
    s.agent_reply(CLAUDE, q, "answer", "done");
    s.yield_turn(CLAUDE, turn);
    let doc = dir.join("pr.md");
    let review = review::review_path_for(&doc);
    fs::write(&doc, s.text()).unwrap();
    fs::write(&review, s.log()).unwrap();
    (s, doc, review)
}

#[test]
fn summarize_is_the_full_fold_in_brief() {
    let dir = tempfile::tempdir().unwrap();
    let (s, doc, review) = on_disk(dir.path());
    let summary = review::summarize(&review, &doc).unwrap();
    let state: ReviewState = s.fold();
    assert_eq!(summary.stage, Stage::Replied);
    assert_eq!(summary.stage, state.phrase.stage);
    assert_eq!(
        serde_json::to_value(&summary.counts).unwrap(),
        serde_json::to_value(&state.counts).unwrap()
    );
    // The counts only a full fold has
    assert_eq!(summary.counts.unseen, 1);
    assert_eq!(summary.counts.not_handled, 1);
    let v = serde_json::to_value(&summary).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["counts", "phrase", "stage"]);
    assert_eq!(v["phrase"]["stage"], "replied");
}

#[test]
fn summarize_needs_the_review_file_and_the_doc() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("pr.md");
    fs::write(&doc, DOC).unwrap();
    assert!(review::summarize(&review::review_path_for(&doc), &doc).is_err());

    let (_, doc, review) = on_disk(dir.path());
    fs::remove_file(&doc).unwrap();
    assert!(review::summarize(&review, &doc).is_err());
}

#[test]
fn a_review_file_sits_beside_its_doc() {
    assert_eq!(
        review::review_path_for(Path::new("/work/pr.md")),
        Path::new("/work/pr.md.birta.jsonl")
    );
    assert_eq!(
        review::review_path_for(Path::new("notes.markdown")),
        Path::new("notes.markdown.birta.jsonl")
    );
    assert_eq!(FOLDER_REVIEW, ".birta.jsonl");
}

#[test]
fn review_files_are_known_by_their_suffix_in_any_case() {
    for name in [
        "pr.md.birta.jsonl",
        FOLDER_REVIEW,
        "PR.MD.BIRTA.JSONL",
        "x.Birta.Jsonl",
    ] {
        assert!(review::is_review_file(name), "{name}");
    }
    for name in [
        "pr.md",
        "birta.jsonl",
        "pr.birta.json",
        "pr.birta.jsonl.md",
        "",
    ] {
        assert!(!review::is_review_file(name), "{name}");
    }
}

// --- the folder file (scenario 17) ---

#[test]
fn scenario_17_a_folder_file_folds_its_notes_on_the_folder() {
    let mut s = Scenario::folder("dirmode");
    let a = s.comment(json!("dir"), None, Some("the sub/ docs repeat the index"));
    let b = s.comment(
        json!("dir"),
        Some("ask"),
        Some("which of these is current?"),
    );
    let state = fold::fold_dir(&s.parsed());
    assert_eq!(state.doc, "dirmode");
    assert_eq!(state.phrase.stage, Stage::Drafts);
    assert_eq!(state.counts.drafts, 2);
    assert_eq!((state.counts.items, state.counts.decided), (0, 0));
    for item in &state.items {
        assert_eq!(item.at.kind, AtKind::Dir);
        assert!(item.at.loc.is_none() && item.at.status.is_none());
        assert_eq!(item.status, ItemStatus::Draft);
    }

    let sub = s.submit();
    let turn = s.claim(CLAUDE);
    s.done(CLAUDE, a, turn);
    s.agent_reply(CLAUDE, b, "answer", "index.md");
    s.yield_turn(CLAUDE, turn);
    let state = fold::fold_dir(&s.parsed());
    assert_eq!(state.phrase.stage, Stage::Replied);
    for n in [a, b] {
        let item = state.items.iter().find(|i| i.n == n).unwrap();
        assert_eq!(item.status, ItemStatus::Handled, "{n}");
        assert_eq!((item.sent_in, item.turn), (Some(sub), Some(turn)), "{n}");
    }
    let replied = state.phrase.replied.as_ref().unwrap();
    assert_eq!((replied.done, replied.answered), (1, 1));
    assert!(state.changes.is_empty() && state.deletions.is_empty());
    assert!(state.rewrite.is_none() && state.lost.is_empty());
    assert_eq!(state.rounds.len(), 1);
}

#[test]
fn scenario_17_a_folder_hand_back_holds_like_a_docs() {
    let mut s = Scenario::folder("dirmode");
    s.comment(json!("dir"), None, Some("one"));
    let held = s.submit_held();
    let state = fold::fold_dir(&s.parsed());
    let hold = state.hold.as_ref().expect("a hold");
    assert_eq!((hold.n, hold.count), (held, 1));
    assert_eq!(
        state.phrase_before_hold.as_ref().unwrap().stage,
        Stage::Drafts
    );
}

#[test]
fn scenario_17_an_unanswered_folder_note_is_not_handled() {
    let mut s = Scenario::folder("dirmode");
    let a = s.comment(json!("dir"), Some("no"), Some("too many files"));
    s.submit();
    let turn = s.claim(CLAUDE);
    s.yield_turn(CLAUDE, turn);
    let state = fold::fold_dir(&s.parsed());
    let item = state.items.iter().find(|i| i.n == a).unwrap();
    assert_eq!(item.status, ItemStatus::NotHandled);
    assert_eq!(item.id.as_deref(), Some(format!("nh:{a}").as_str()));
    assert_eq!(state.counts.not_handled, 1);
}

#[test]
fn scenario_17_a_folder_file_has_no_changes() {
    let mut s = Scenario::folder("dirmode");
    s.comment(json!("dir"), None, Some("one"));
    s.snap_to("stray text\n");
    let state = fold::fold_dir(&s.parsed());
    assert!(state.changes.is_empty() && state.deletions.is_empty());
    assert_eq!(state.items.len(), 1);
}

// --- adversary killers ---

#[test]
fn claimable_round_agrees_with_scan_on_approves_releases_and_withdrawals() {
    let mut approve = Scenario::new(DOC);
    approve.note("a");
    let a = approve.approve_held();
    let mut released = Scenario::new(DOC);
    released.note("a");
    let r = released.submit();
    let claim = released.claim(CLAUDE);
    released.release(claim);
    let mut withdrawn = Scenario::new(DOC);
    withdrawn.note("a");
    let w = withdrawn.submit_held();
    withdrawn.withdraw(w);
    let claimable = |s: &Scenario, round: u32| {
        let (parsed, chain, doc) = s.build();
        let now = ts(round) + HOLD_MS + 60_000;
        let folded = fold::fold(&parsed, &chain, &doc).claimable_round(now);
        assert_eq!(
            folded,
            fold::scan(&parsed, 0).claimable_round(now),
            "{round}"
        );
        folded
    };
    claimable(&approve, a);
    assert_eq!(claimable(&released, r), Some(r));
    assert_eq!(claimable(&withdrawn, w), None);
}

#[test]
fn notes_covered_by_follows_upto_and_skips_a_withdrawn_round() {
    let covered = |f: &fold::Folded, round: u32| {
        f.notes_covered_by(round)
            .iter()
            .map(|i| i.n)
            .collect::<Vec<_>>()
    };
    let mut s = Scenario::new(DOC);
    let a = s.note("a");
    let late = s.note("typed as the hand-back went");
    let at = ts(s.next());
    let sub = s.raw(
        json!({"op": "submit", "upto": a, "not_before": at, "hold_ms": 0,
        "to": "*", "ts": at, "by": human()}),
    );
    let (parsed, chain, doc) = s.build();
    let folded = fold::fold(&parsed, &chain, &doc);
    assert_eq!(
        folded.state.log.bad + folded.state.log.unknown_ops,
        0,
        "setup"
    );
    assert_eq!(covered(&folded, sub), [a]);
    let typed = folded.state.items.iter().find(|i| i.n == late).unwrap();
    assert_eq!(typed.status, ItemStatus::Draft);

    let mut s = Scenario::new(DOC);
    let a = s.note("a");
    let first = s.submit_held();
    s.withdraw(first);
    let second = s.submit();
    let (parsed, chain, doc) = s.build();
    let folded = fold::fold(&parsed, &chain, &doc);
    assert_eq!(covered(&folded, second), [a]);
    assert!(covered(&folded, first).is_empty());
}

#[test]
fn head_info_knows_a_lone_cr_and_an_unterminated_last_line() {
    assert_eq!(head_info("# T\rbody\r").eol, Eol::Cr);
    assert_eq!(head_info("# T\n\nbody").eol, Eol::Lf);
}

#[test]
fn summarize_folds_on_the_doc_as_it_is_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let (s, doc, review) = on_disk(dir.path());
    assert_eq!(s.fold().counts.items, 10, "setup");
    fs::write(
        &doc,
        s.replaced("- appendix item", "- appendix item\n- appendix two"),
    )
    .unwrap();
    assert_eq!(review::summarize(&review, &doc).unwrap().counts.items, 11);
}

#[test]
fn review_file_names_never_panic_and_keep_their_bytes() {
    // "é" + 11 bytes: byte len - 12 falls inside the "é"
    for name in ["a.birta.jsonl.bak", "éxxxxxxxxxxx", "é", "pr.birta.jsonl "] {
        assert!(!review::is_review_file(name), "{name}");
    }
    assert!(review::is_review_file(".BIRTA.JSONL"));
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let doc = Path::new(OsStr::from_bytes(b"/w/\xff.md"));
        assert_eq!(
            review::review_path_for(doc).as_os_str().as_bytes(),
            b"/w/\xff.md.birta.jsonl"
        );
    }
}

#[test]
fn summary_phrase_is_the_states_phrase() {
    let dir = tempfile::tempdir().unwrap();
    let (s, doc, review) = on_disk(dir.path());
    let summary = review::summarize(&review, &doc).unwrap();
    assert_eq!(
        serde_json::to_value(&summary.phrase).unwrap(),
        serde_json::to_value(&s.fold().phrase).unwrap()
    );
}
