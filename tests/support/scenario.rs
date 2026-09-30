//! Builds review logs op by op for fold tests. Lines go through the real parser,
//! anchors through the real constructor, and doc writes through the real chain.
// Each test binary that includes the kit uses a different part of it
#![allow(dead_code)]

use birta::rev::Rev;
use birta::review::anchor_from_req;
use birta::review::chain::{CHECKPOINT_EVERY, Chain};
use birta::review::doc::Doc;
use birta::review::fold::{self, TurnIndex};
use birta::review::hunk;
use birta::review::ops::{self, AnchorReq, BlockScope, Parsed};
use birta::review::state::ReviewState;
use serde_json::{Value, json};

/// `ts` of line 1; line `n` is written at `T0 + n` seconds.
pub const T0: u64 = 1_790_000_000_000;
/// The hold a browser hand-back asks for.
pub const HOLD_MS: u64 = 5_000;
pub const HUMAN: &str = "andres";
pub const DOC_NAME: &str = "pr.md";

/// Lines 1-27. Stops: headings 1, 11, 21, 25; paragraph 3-4; items 6, 7-8 (holding 8),
/// 9, 13, 14, 23, 27; table rows 18 and 19. `## Details` covers 11-20, `# Plan` 1-24.
pub const DOC: &str = "\
# Plan

Intro paragraph here,
with a second line.

- first item
- second item
  - nested item
- third item

## Details

- detail one
- detail two

| key | value |
|-----|-------|
| a | 1 |
| b | 2 |

## More

- more item

# Appendix

- appendix item
";

/// The `ts` the kit writes on line `n`.
pub fn ts(n: u32) -> u64 {
    T0 + u64::from(n) * 1_000
}

pub fn human() -> Value {
    json!({"type": "human", "name": HUMAN, "via": "browser"})
}

pub fn agent(name: &str) -> Value {
    json!({"type": "agent", "name": name, "via": "cli"})
}

pub fn birta_by() -> Value {
    json!({"type": "birta"})
}

pub struct Scenario {
    lines: Vec<String>,
    /// The doc's current text, as the last recorded write left it.
    text: String,
    /// The last human line a hand-back would cover.
    last_human: u32,
    /// The last submit or approve line.
    last_round: u32,
    /// Set by hand-written lines, which may be meant not to parse.
    hand_written: bool,
    /// Writes made by [`Scenario::external_diffed`].
    diffed: usize,
}

impl Scenario {
    /// A review file created for `text` by a human: header, snap, `owner{null}`.
    pub fn new(text: &str) -> Self {
        Self::created(text, Some(None))
    }

    /// A review file created with `--drafted-by name`.
    pub fn drafted_by(text: &str, name: &str) -> Self {
        Self::created(text, Some(Some(name)))
    }

    /// Header and snap only, as a hand-made file might be.
    pub fn without_owner(text: &str) -> Self {
        Self::created(text, None)
    }

    /// A folder's review file, `<root>/.birta.jsonl`, whose notes are on the folder.
    pub fn folder(name: &str) -> Self {
        Self::named(name, "", Some(None))
    }

    fn created(text: &str, owner: Option<Option<&str>>) -> Self {
        Self::named(DOC_NAME, text, owner)
    }

    fn named(doc: &str, text: &str, owner: Option<Option<&str>>) -> Self {
        let mut s = Self {
            lines: vec![
                json!({"birta": "review/1", "doc": doc, "about": "ops after the last hand-back are drafts; don't act on them"})
                    .to_string(),
            ],
            text: text.to_string(),
            last_human: 0,
            last_round: 0,
            hand_written: false,
            diffed: 0,
        };
        s.push(
            birta_by(),
            json!({"op": "snap", "rev": Rev::of(text.as_bytes()), "text": text}),
        );
        if let Some(drafted_by) = owner {
            s.push(birta_by(), json!({"op": "owner", "drafted_by": drafted_by}));
        }
        s
    }

    /// Line number the next op lands on.
    pub fn next(&self) -> u32 {
        self.lines.len() as u32 + 1
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn log(&self) -> String {
        let mut log = self.lines.join("\n");
        log.push('\n');
        log
    }

    /// Appends `op` with the kit's `ts` and `by`; returns its line number.
    pub fn push(&mut self, by: Value, mut op: Value) -> u32 {
        let n = self.next();
        op["ts"] = json!(ts(n));
        if by["type"] == "human" {
            self.last_human = n;
        }
        op["by"] = by;
        self.lines.push(op.to_string());
        n
    }

    /// Appends `line` exactly as given, like a hand edit.
    pub fn raw(&mut self, line: Value) -> u32 {
        self.raw_text(&line.to_string())
    }

    pub fn raw_text(&mut self, line: &str) -> u32 {
        self.hand_written = true;
        self.lines.push(line.to_string());
        self.next() - 1
    }

    // --- anchors, built on the current text by the one constructor ---

    fn doc(&self) -> Doc {
        Doc::new(&self.text)
    }

    fn at(&self, req: AnchorReq) -> Value {
        let doc = self.doc();
        let at = anchor_from_req(&req, &doc, &doc.rev).expect("test anchor must build");
        serde_json::to_value(at).unwrap()
    }

    /// A block anchor on the stop whose first line contains `needle`.
    pub fn block(&self, needle: &str) -> Value {
        let sp = self.stop_sp(needle);
        self.at(AnchorReq::Block {
            sp,
            scope: BlockScope::Block,
        })
    }

    /// A section anchor on the heading whose line contains `needle`.
    pub fn section(&self, needle: &str) -> Value {
        let sp = self.stop_sp(needle);
        self.at(AnchorReq::Block {
            sp,
            scope: BlockScope::Section,
        })
    }

    /// A quote anchor on the one place `q` occurs.
    pub fn quote(&self, q: &str) -> Value {
        self.at(AnchorReq::Quote {
            sp: None,
            q: q.to_string(),
            pre: String::new(),
            suf: String::new(),
            off: None,
            tl: None,
        })
    }

    fn stop_sp(&self, needle: &str) -> String {
        let doc = self.doc();
        let stop = doc
            .blocks
            .stops
            .iter()
            .find(|s| {
                self.text
                    .lines()
                    .nth(s.lines.0 as usize - 1)
                    .is_some_and(|l| l.contains(needle))
            })
            .unwrap_or_else(|| panic!("no stop starts on a line with {needle:?}"));
        stop.sp.clone()
    }

    // --- human ops ---

    pub fn comment(&mut self, at: Value, verdict: Option<&str>, body: Option<&str>) -> u32 {
        let n = self.next();
        let mut op = json!({"op": "comment", "at": at, "cid": format!("c-{n}")});
        if let Some(v) = verdict {
            op["verdict"] = json!(v);
        }
        if let Some(b) = body {
            op["body"] = json!(b);
        }
        self.push(human(), op)
    }

    /// A plain note on the whole doc.
    pub fn note(&mut self, body: &str) -> u32 {
        self.comment(json!("doc"), None, Some(body))
    }

    /// A walk verdict: a block anchor on the stop starting with `needle`.
    pub fn verdict(&mut self, needle: &str, verdict: &str) -> u32 {
        let at = self.block(needle);
        self.comment(at, Some(verdict), None)
    }

    pub fn amend(&mut self, to: u32, body: Option<&str>, verdict: Option<&str>) -> u32 {
        let mut op = json!({"op": "amend", "to": to});
        if let Some(b) = body {
            op["body"] = json!(b);
        }
        if let Some(v) = verdict {
            op["verdict"] = json!(v);
        }
        self.push(human(), op)
    }

    pub fn retract(&mut self, to: u32) -> u32 {
        self.push(human(), json!({"op": "retract", "to": to}))
    }

    pub fn human_reply(&mut self, to: u32, verdict: Option<&str>, body: Option<&str>) -> u32 {
        let mut op = json!({"op": "reply", "to": to});
        if let Some(v) = verdict {
            op["verdict"] = json!(v);
        }
        if let Some(b) = body {
            op["body"] = json!(b);
        }
        self.push(human(), op)
    }

    /// A hand-back of every human line so far, due at once.
    pub fn submit(&mut self) -> u32 {
        self.submit_until(ts(self.next()))
    }

    /// A hand-back with the browser's hold.
    pub fn submit_held(&mut self) -> u32 {
        self.submit_until(ts(self.next()) + HOLD_MS)
    }

    pub fn submit_until(&mut self, not_before: u64) -> u32 {
        let upto = self.last_human;
        let hold_ms = not_before.saturating_sub(ts(self.next()));
        let n = self.push(
            human(),
            json!({"op": "submit", "upto": upto, "not_before": not_before, "hold_ms": hold_ms, "to": "*"}),
        );
        self.last_round = n;
        n
    }

    /// Approves the current text, due at once.
    pub fn approve(&mut self) -> u32 {
        self.approve_until(ts(self.next()))
    }

    pub fn approve_held(&mut self) -> u32 {
        self.approve_until(ts(self.next()) + HOLD_MS)
    }

    fn approve_until(&mut self, not_before: u64) -> u32 {
        let hold_ms = not_before.saturating_sub(ts(self.next()));
        let rev = Rev::of(self.text.as_bytes());
        let n = self.push(
            human(),
            json!({"op": "approve", "rev": rev, "not_before": not_before, "hold_ms": hold_ms}),
        );
        self.last_round = n;
        n
    }

    pub fn withdraw(&mut self, to: u32) -> u32 {
        self.push(human(), json!({"op": "withdraw", "to": to}))
    }

    /// The human takes a stalled round back from the claim on line `turn`.
    pub fn release(&mut self, turn: u32) -> u32 {
        self.push(
            human(),
            json!({"op": "yield", "turn": turn, "release": true}),
        )
    }

    pub fn seen(&mut self, ids: &[&str]) -> u32 {
        self.push(human(), json!({"op": "seen", "ids": ids}))
    }

    pub fn owner(&mut self, by: Value, drafted_by: Option<&str>) -> u32 {
        self.push(by, json!({"op": "owner", "drafted_by": drafted_by}))
    }

    // --- agent ops ---

    /// Claims the last submit or approve.
    pub fn claim(&mut self, name: &str) -> u32 {
        let round = self.last_round;
        self.claim_round(name, round)
    }

    pub fn claim_round(&mut self, name: &str, round: u32) -> u32 {
        self.push(agent(name), json!({"op": "claim", "round": round}))
    }

    pub fn done(&mut self, name: &str, to: u32, turn: u32) -> u32 {
        self.push(agent(name), json!({"op": "done", "to": to, "turn": turn}))
    }

    /// An agent reply of `kind` (`answer`, `didnt`, `note`).
    pub fn agent_reply(&mut self, name: &str, to: u32, kind: &str, body: &str) -> u32 {
        self.push(
            agent(name),
            json!({"op": "reply", "to": to, "kind": kind, "body": body}),
        )
    }

    /// A reply from a claim the human released.
    pub fn late_reply(&mut self, name: &str, to: u32, kind: &str, body: &str) -> u32 {
        self.push(
            agent(name),
            json!({"op": "reply", "to": to, "kind": kind, "body": body, "late": true}),
        )
    }

    /// A blank for the human: an agent `ask` in turn `turn`.
    pub fn blank(&mut self, name: &str, at: Value, body: &str, turn: u32) -> u32 {
        self.push(
            agent(name),
            json!({"op": "comment", "at": at, "verdict": "ask", "body": body, "turn": turn}),
        )
    }

    /// The agent cites note `to` for the text `quote` it wrote.
    pub fn covers(&mut self, name: &str, to: u32, quote: &str, turn: u32) -> u32 {
        self.push(
            agent(name),
            json!({"op": "covers", "to": to, "quote": quote, "turn": turn}),
        )
    }

    /// The agent flags its own change of `quote` as one nobody asked for.
    pub fn unasked(&mut self, name: &str, quote: &str, turn: u32) -> u32 {
        self.push(
            agent(name),
            json!({"op": "unasked", "quote": quote, "body": "while I was there", "turn": turn}),
        )
    }

    pub fn turn_note(&mut self, name: &str, body: &str, turn: u32) -> u32 {
        self.push(
            agent(name),
            json!({"op": "turn_note", "body": body, "turn": turn}),
        )
    }

    pub fn yield_turn(&mut self, name: &str, turn: u32) -> u32 {
        self.push(agent(name), json!({"op": "yield", "turn": turn}))
    }

    // --- doc writes ---

    /// A write birta observed (an agent's, inside a turn), recorded as the chain would.
    pub fn external(&mut self, new: &str) -> u32 {
        self.write("external", birta_by(), new)
    }

    /// birta's own write for the human.
    pub fn edit(&mut self, new: &str) -> u32 {
        self.write("edit", human(), new)
    }

    /// An `external` that replaces the one line equal to `old` with `new`.
    pub fn replace_line(&mut self, old: &str, new: &str) -> u32 {
        let text = self.replaced(old, new);
        self.external(&text)
    }

    pub fn replaced(&self, old: &str, new: &str) -> String {
        let hits: Vec<_> = self.text.match_indices(&format!("{old}\n")).collect();
        assert_eq!(hits.len(), 1, "{old:?} must be one whole line");
        self.text
            .replacen(&format!("{old}\n"), &format!("{new}\n"), 1)
    }

    /// An `external` that git made, as a checkout or pull would.
    pub fn external_git(&mut self, new: &str) -> u32 {
        let chain = Chain::build(&self.parsed());
        let rec = chain.record(new).expect("the write must change the doc");
        let line = json!({"op": "external", "rev": rec.rev, "to_rev": rec.to_rev, "patch": rec.patch, "cause": "git"});
        self.land(birta_by(), line, rec.checkpoint, new)
    }

    /// An `external` recorded by rev only, with a checkpoint snap giving its text, as a
    /// write whose patch is too large is recorded.
    pub fn external_rev_only(&mut self, new: &str) -> u32 {
        let rev = Rev::of(self.text.as_bytes());
        let to_rev = Rev::of(new.as_bytes());
        let line = json!({"op": "external", "rev": rev, "to_rev": to_rev, "patch": null});
        self.land(birta_by(), line, Some(new.to_string()), new)
    }

    /// birta's `x` undo of change `of`, leaving the doc as `new`.
    pub fn undo(&mut self, of: &str, new: &str) -> u32 {
        let chain = Chain::build(&self.parsed());
        let rec = chain.record(new).expect("the undo must change the doc");
        let line = json!({"op": "edit", "rev": rec.rev, "to_rev": rec.to_rev, "patch": rec.patch, "act": "undo", "of": of});
        self.land(human(), line, rec.checkpoint, new)
    }

    /// A bare snap of a new text, standing in for a write nobody recorded.
    pub fn snap_to(&mut self, new: &str) -> u32 {
        let n = self.push(
            birta_by(),
            json!({"op": "snap", "rev": Rev::of(new.as_bytes()), "text": new}),
        );
        self.text = new.to_string();
        n
    }

    /// An agent's `external` diffed against the kit's own text, without rebuilding the
    /// chain; a checkpoint snap follows every 20th. Only for logs written this way.
    pub fn external_diffed(&mut self, new: &str) -> u32 {
        let line = json!({
            "op": "external",
            "rev": Rev::of(self.text.as_bytes()),
            "to_rev": Rev::of(new.as_bytes()),
            "patch": hunk::diff(&self.text, new),
        });
        self.diffed += 1;
        let checkpoint = self
            .diffed
            .is_multiple_of(CHECKPOINT_EVERY)
            .then(|| new.to_string());
        self.land(birta_by(), line, checkpoint, new)
    }

    fn write(&mut self, op: &str, by: Value, new: &str) -> u32 {
        let chain = Chain::build(&self.parsed());
        let rec = chain.record(new).expect("the write must change the doc");
        let mut line = json!({"op": op, "rev": rec.rev, "to_rev": rec.to_rev, "patch": rec.patch});
        if op == "edit" {
            line["act"] = json!("edit");
        }
        self.land(by, line, rec.checkpoint, new)
    }

    /// Appends a write's line and its checkpoint snap, and makes `new` the doc's text.
    fn land(&mut self, by: Value, line: Value, checkpoint: Option<String>, new: &str) -> u32 {
        let to_rev = line["to_rev"].clone();
        let n = self.push(by, line);
        if let Some(text) = checkpoint {
            self.push(
                birta_by(),
                json!({"op": "snap", "rev": to_rev, "text": text, "checkpoint": true}),
            );
        }
        self.text = new.to_string();
        n
    }

    // --- reading it back ---

    pub fn parsed(&self) -> Parsed {
        let parsed = ops::parse(self.log());
        assert!(!parsed.read_only, "test log must be writable");
        parsed
    }

    pub fn build(&self) -> (Parsed, Chain, Doc) {
        let parsed = self.parsed();
        let chain = Chain::build(&parsed);
        (parsed, chain, self.doc())
    }

    /// Folds the log. Every line the kit wrote itself must parse, so a mistake in the
    /// kit can't pass as a line the fold ignores.
    pub fn fold(&self) -> ReviewState {
        let (parsed, chain, doc) = self.build();
        let state = fold::fold(&parsed, &chain, &doc).state;
        if !self.hand_written {
            assert_eq!(
                (state.log.bad, state.log.unknown_ops),
                (0, 0),
                "kit lines must parse: {:?} {:?}",
                parsed.bad,
                parsed.unknown
            );
        }
        state
    }

    pub fn scan(&self, now: u64) -> TurnIndex {
        fold::scan(&self.parsed(), now)
    }
}
