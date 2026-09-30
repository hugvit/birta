//! The claim CAS, the claim-or-peek rule and the wait timeout, called as a library with
//! an injected clock. `birta review` itself is driven in `agent_review_test.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use birta::agent::identity::Identity;
use birta::agent::{review_cmd, wait};
use birta::review::fold;
use birta::review::log::{AppendCtx, Log};
use birta::review::ops::{self, By, ByKind, Op, Via};
use birta::review::review_path_for;

mod support {
    pub mod scenario;
}
use support::scenario::{DOC, HOLD_MS, Scenario, ts};

fn agent(name: &str) -> By {
    By {
        kind: ByKind::Agent,
        name: Some(name.to_string()),
        via: Some(Via::Cli),
    }
}

/// A scenario's doc and review file on disk.
struct Placed {
    _tmp: tempfile::TempDir,
    doc: PathBuf,
}

impl Placed {
    fn new(s: &Scenario) -> Self {
        let p = Self::doc_only(s.text());
        fs::write(review_path_for(&p.doc), s.log()).unwrap();
        p
    }

    fn doc_only(text: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let doc = tmp.path().canonicalize().unwrap().join("pr.md");
        fs::write(&doc, text).unwrap();
        Self { _tmp: tmp, doc }
    }

    fn log(&self) -> PathBuf {
        review_path_for(&self.doc)
    }

    fn bytes(&self) -> Vec<u8> {
        fs::read(self.log()).unwrap()
    }

    /// Every claim line: `(n, round, agent name)`.
    fn claims(&self) -> Vec<(u32, u32, Option<String>)> {
        claims_in(&self.log())
    }
}

fn claims_in(log: &Path) -> Vec<(u32, u32, Option<String>)> {
    let parsed = ops::parse(fs::read(log).unwrap());
    assert!(
        parsed.bad.is_empty(),
        "claim lines must parse: {:?}",
        parsed.bad
    );
    parsed
        .lines
        .iter()
        .filter_map(|l| match l.op {
            Op::Claim { round } => Some((l.n, round, l.by.name.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_claim_waits_for_not_before_then_takes_the_round() {
    let mut s = Scenario::new(DOC);
    s.note("first");
    let sub = s.submit_held();
    let p = Placed::new(&s);
    let due = ts(sub) + HOLD_MS;

    let before = p.bytes();
    assert_eq!(
        review_cmd::claim(&p.doc, &agent("claude"), due - 1).unwrap(),
        None
    );
    assert_eq!(p.bytes(), before, "a claim in the hold wrote to the log");

    let line = review_cmd::claim(&p.doc, &agent("claude"), due)
        .unwrap()
        .expect("the round is due");
    assert_eq!(p.claims(), vec![(line, sub, Some("claude".to_string()))]);
    let after = p.bytes();
    assert!(after.starts_with(&before));
    assert_eq!(
        after[before.len()..]
            .iter()
            .filter(|&&b| b == b'\n')
            .count(),
        1,
        "a claim appends one line and nothing else"
    );
}

#[test]
fn a_claimed_round_takes_no_second_claim_from_anyone() {
    let mut s = Scenario::new(DOC);
    s.note("first");
    let sub = s.submit();
    let p = Placed::new(&s);
    let now = ts(sub) + 60_000;

    let line = review_cmd::claim(&p.doc, &agent("claude"), now)
        .unwrap()
        .expect("the round is due");
    let held = p.bytes();
    for name in ["codex", "claude"] {
        assert_eq!(
            review_cmd::claim(&p.doc, &agent(name), now).unwrap(),
            None,
            "{name}"
        );
        assert_eq!(p.bytes(), held, "{name} wrote a second claim");
    }
    assert_eq!(p.claims(), vec![(line, sub, Some("claude".to_string()))]);
}

#[test]
fn a_withdrawn_round_is_not_claimed() {
    let mut s = Scenario::new(DOC);
    s.note("taken back");
    let sub = s.submit();
    s.withdraw(sub);
    let p = Placed::new(&s);
    let before = p.bytes();
    assert_eq!(
        review_cmd::claim(&p.doc, &agent("claude"), ts(s.next()) + 60_000).unwrap(),
        None
    );
    assert_eq!(p.bytes(), before);
}

#[test]
fn an_approve_is_claimed_like_a_hand_back() {
    let mut s = Scenario::new(DOC);
    s.note("first");
    s.submit();
    let turn = s.claim("claude");
    s.yield_turn("claude", turn);
    let approve = s.approve();
    let p = Placed::new(&s);
    let line = review_cmd::claim(&p.doc, &agent("claude"), ts(approve) + 60_000)
        .unwrap()
        .expect("the approve is due");
    assert_eq!(
        p.claims().last(),
        Some(&(line, approve, Some("claude".to_string())))
    );
}

#[test]
fn a_claim_on_a_doc_with_no_review_file_creates_nothing() {
    let p = Placed::doc_only(DOC);
    assert_eq!(
        review_cmd::claim(&p.doc, &agent("claude"), ts(100)).unwrap(),
        None
    );
    assert!(!p.log().exists());
}

#[test]
fn two_claimers_racing_through_a_barrier_write_exactly_one_claim() {
    for i in 0..100 {
        let mut s = Scenario::new(DOC);
        s.note("first");
        let sub = s.submit();
        let p = Placed::new(&s);
        let now = ts(sub) + 60_000;
        let barrier = Arc::new(Barrier::new(2));
        let racers: Vec<_> = ["claude", "codex"]
            .into_iter()
            .map(|name| {
                let doc = p.doc.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (name, review_cmd::claim(&doc, &agent(name), now).unwrap())
                })
            })
            .collect();
        let results: Vec<(&str, Option<u32>)> =
            racers.into_iter().map(|r| r.join().unwrap()).collect();
        let winners: Vec<_> = results.iter().filter(|(_, won)| won.is_some()).collect();
        assert_eq!(winners.len(), 1, "iteration {i}: {results:?}");
        let (name, line) = winners[0];
        assert_eq!(
            p.claims(),
            vec![(line.unwrap(), sub, Some(name.to_string()))],
            "iteration {i}"
        );
    }
}

#[test]
fn only_an_agent_shell_or_a_pipe_claims() {
    let claude = Identity {
        name: "claude".to_string(),
    };
    // (peek, identity, stdout is a tty) -> claims
    let cases = [
        (false, Some(&claude), true, true),
        (false, Some(&claude), false, true),
        (false, None, false, true),
        (false, None, true, false),
        (true, Some(&claude), false, false),
        (true, Some(&claude), true, false),
        (true, None, false, false),
    ];
    for (peek, identity, tty, want) in cases {
        assert_eq!(
            review_cmd::should_claim(peek, identity, tty),
            want,
            "peek={peek} identity={identity:?} tty={tty}"
        );
    }
}

#[test]
fn the_wait_timeout_defaults_to_900_s_and_0_waits_forever() {
    assert_eq!(wait::timeout(None), Some(Duration::from_secs(900)));
    assert_eq!(wait::timeout(Some(0)), None);
    assert_eq!(wait::timeout(Some(7)), Some(Duration::from_secs(7)));
}

#[test]
fn a_claim_racing_a_withdraw_never_lets_both_land() {
    let human = By {
        kind: ByKind::Human,
        name: Some("andres".to_string()),
        via: Some(Via::Browser),
    };
    for i in 0..100 {
        let mut s = Scenario::new(DOC);
        s.note("first");
        let sub = s.submit();
        let p = Placed::new(&s);
        let now = ts(sub) + 60_000;
        let barrier = Arc::new(Barrier::new(2));
        let withdraw = {
            let log = Log::open(&p.log());
            let (barrier, by) = (Arc::clone(&barrier), human.clone());
            std::thread::spawn(move || {
                barrier.wait();
                let ctx = AppendCtx {
                    now_ms: now,
                    by,
                    cid: None,
                };
                log.append(ctx, vec![Op::Withdraw { to: sub }], |parsed, _| {
                    fold::scan(parsed, now).check_withdraw(sub)
                })
                .is_ok()
            })
        };
        let claim = {
            let (doc, barrier) = (p.doc.clone(), Arc::clone(&barrier));
            std::thread::spawn(move || {
                barrier.wait();
                review_cmd::claim(&doc, &agent("claude"), now)
                    .unwrap()
                    .is_some()
            })
        };
        let (withdrew, claimed) = (withdraw.join().unwrap(), claim.join().unwrap());
        assert!(
            withdrew != claimed,
            "iteration {i}: withdrew={withdrew} claimed={claimed}"
        );
        assert_eq!(p.claims().len(), usize::from(claimed), "iteration {i}");
    }
}

#[test]
fn a_released_round_can_be_claimed_again() {
    let mut s = Scenario::new(DOC);
    s.note("first");
    let sub = s.submit();
    let turn = s.claim("pi");
    s.release(turn);
    let p = Placed::new(&s);
    let line = review_cmd::claim(&p.doc, &agent("claude"), ts(s.next()) + 60_000)
        .unwrap()
        .expect("a release hands the round back");
    assert_eq!(
        p.claims().last(),
        Some(&(line, sub, Some("claude".to_string())))
    );
}
