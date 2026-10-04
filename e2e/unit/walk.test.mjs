// Times in the phrase are local; pin the zone before anything reads the clock
process.env.TZ = "UTC";

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ASSETS = path.join(HERE, "..", "..", "assets", "review");
const FIXTURES = path.join(HERE, "..", "fixtures", "review");
const ORDER = ["quote.js", "00-core.js", "10-keys.js", "20-pointing.js", "30-flow.js", "40-lane.js", "60-chrome.js"];

/** Runs the review files in a context with no DOM, as they must touch none at load. */
function load(files = ORDER) {
  const ctx = vm.createContext({});
  for (const f of files) {
    const file = path.join(ASSETS, f);
    vm.runInContext(fs.readFileSync(file, "utf-8"), ctx, { filename: file });
  }
  return ctx.BR;
}

function state(name) {
  return JSON.parse(fs.readFileSync(path.join(FIXTURES, `${name}.json`), "utf-8"));
}

function bent(name, edit) {
  const s = state(name);
  edit(s);
  return s;
}

// Results come from the vm context, whose objects have their own prototypes
const plain = (v) => (v == null ? v : JSON.parse(JSON.stringify(v)));

/** The n/p walk as `kind id @sp` strings, `@doc` for the doc slot and `@after sp` (or `top`) for a removal's gap. */
function walk(BR, s) {
  return plain(BR.pure.walk(s)).map((e) =>
    e.kind === "removal" ? `removal ${e.id} @after ${e.after_sp ?? "top"}` : `${e.kind} ${e.id} @${e.sp ?? "doc"}`,
  );
}

/** The stop of each walk entry: `doc`, `b:<sp>`, `r:<removal id>` or `q:<question id>`. */
function stopsOf(BR, s) {
  return plain(BR.pure.walk(s)).map((e) => e.stop);
}

/** The distinct stops of `entries`, in order. */
const distinct = (entries) => [...new Set(entries.map((e) => e.stop))];

/** Checks every news item, as y on each stop would; the fold then counts nothing to check. */
function allChecked(s) {
  for (const n of s.news) if (n.check == null) n.check = n.kind === "change" || n.kind === "removal" ? "kept" : n.kind === "question" ? "answered" : "ok";
  for (const c of s.changes) if (c.attr !== "human" && c.check == null) c.check = "kept";
  s.counts.to_check = 0;
}

/** A hand-back after everything in `s` settles what was checked: it leaves the news list. */
function handedBack(s, at = 40) {
  s.rounds.push({ submit: at, ts: s.server_now, by: "", to: "*", items: [], released: false });
  s.news = s.news.filter((n) => n.check == null);
  for (const c of s.changes) if (c.check != null) c.settled = true;
}

const NAMES = ["unseen", "flow", "replied", "approved", "fresh", "drafts", "sent", "picked", "rewrite", "hold", "lost", "journey/round1"];

// unseen.json's news, in walk order: whole doc's reply and turn note, then 9.0 and the done on
// its paragraph, the not-handled bullet, 10.0 on the next bullet, and the removal in its gap
const UNSEEN_WALK = [
  "reply #13 @doc",
  "turn_note #14 @doc",
  "change 9.0 @5:1-6:68",
  "reply #4 @5:1-6:68",
  "not_handled nh:5 @10:1-10:16",
  "change 10.0 @11:1-11:44",
  "removal 11.0 @after 14:1-14:13",
];

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the n/p walk loads without a DOM, ${name}`, () => {
    assert.equal(typeof load(files).pure.walk, "function");
  });
}

// --- The walk is Claude's news ------------------------------------------------------------------

test("the walk is Claude's news in its own order, each entry on its stop", () => {
  const BR = load();
  assert.deepEqual(walk(BR, state("unseen")), UNSEEN_WALK);
  assert.deepEqual(stopsOf(BR, state("unseen")), ["doc", "doc", "b:5:1-6:68", "b:5:1-6:68", "b:10:1-10:16", "b:11:1-11:44", "r:11.0"]);
});

test("every kind in one state: the doc slot first, the lost-notes row just before the turn note, then the news list's order", () => {
  const BR = load();
  assert.deepEqual(walk(BR, state("flow")), [
    "not_handled nh:4 @doc",
    "not_handled nh:5 @doc",
    "reply #18 @doc",
    // The lost-notes row closes the doc slot, and Claude's turn note comes last in it
    "lost lost @doc",
    "turn_note #23 @doc",
    // Your answer to it is not handed back yet, so the question is still news, checked
    "question #22 @1:1-1:16",
    "question #21 @3:1-3:6",
    "change 13.0 @5:1-6:68",
    "change 26.0 @8:1-8:15",
    "not_handled nh:7 @10:1-10:16",
    "change 15.0 @11:1-11:44",
    "reply #6 @11:1-11:44",
    "reply #9 @12:1-13:0",
    "change 19.0 @14:1-14:21",
    "change 27.0 @16:1-16:19",
  ]);
  // A question is a stop of its own, under its block
  const stops = stopsOf(BR, state("flow"));
  assert.deepEqual(stops.slice(5, 7), ["q:#22", "q:#21"]);
  assert.equal(stops[11], "b:11:1-11:44");
});

test("for every fixture the walk's news entries are the state's news, in order, nothing re-sorted or left out", () => {
  const BR = load();
  for (const name of NAMES) {
    const s = state(name);
    const got = plain(BR.pure.walk(s))
      .filter((e) => e.kind !== "draft" && e.kind !== "lost")
      .map((e) => ({ id: e.id, kind: e.kind, sp: e.sp ?? null, check: e.check ?? null }));
    const want = s.news.map((n) => ({ id: n.id, kind: n.kind, sp: n.sp ?? null, check: n.check ?? null }));
    assert.deepEqual(got, want, name);
  }
});

test("the walk follows news, not the changes' own flags", () => {
  const BR = load();
  // Flags that once took a change off the walk; news still lists it, so it stays
  const flagged = bent("unseen", (s) => {
    const c = s.changes.find((c) => c.id === "9.0");
    c.seen = true;
    c.settled = true;
  });
  assert.ok(walk(BR, flagged).includes("change 9.0 @5:1-6:68"));
  assert.ok(plain(BR.pure.attention(flagged)).some((e) => e.id === "9.0"));
  // Not in news, so not walked, however unseen its change
  const gone = bent("unseen", (s) => (s.news = s.news.filter((n) => n.id !== "10.0")));
  assert.ok(!walk(BR, gone).some((e) => e.includes("10.0")));
  assert.ok(!plain(BR.pure.attention(gone)).some((e) => e.id === "10.0"));
});

test("a reply to a note on a block Claude cut is on that removal's stop, after the removal", () => {
  const BR = load();
  const entries = plain(BR.pure.walk(state("journey/round1")));
  const done = entries.find((e) => e.id === "#6");
  assert.equal(done.stop, "r:10.5");
  const at = entries.findIndex((e) => e.id === "#6");
  assert.deepEqual([entries[at - 1].kind, entries[at - 1].id], ["removal", "10.5"]);
  // J-08: one y on whole doc checks its reply and turn note, which leaves 8 stops, none of them whole doc
  const s = bent("journey/round1", (s) => {
    for (const n of s.news) if (n.id === "#4" || n.id === "#21") n.check = "ok";
    s.counts.to_check = 8;
  });
  const left = distinct(plain(BR.pure.attention(s)));
  assert.equal(left.length, 8);
  assert.ok(!left.includes("doc"), left.join(", "));
  assert.ok(left.includes("r:10.5"));
});

test("the journey's twelve items are nine stops, in walk order", () => {
  const BR = load();
  assert.deepEqual(distinct(plain(BR.pure.walk(state("journey/round1")))), [
    "doc",
    "b:5:1-7:54",
    "b:11:1-11:80",
    "r:10.2",
    "b:12:1-13:0",
    "r:10.4",
    "r:10.5",
    "b:16:1-16:62",
    "b:17:1-17:31",
  ]);
});

test("the lost-notes row is one stop at the end of the doc slot, before the turn note, while any note is lost", () => {
  const BR = load();
  assert.deepEqual(walk(BR, state("lost")).slice(0, 4), ["reply #4 @doc", "reply #11 @doc", "lost lost @doc", "turn_note #12 @doc"]);
  // It stays reachable after a hand-back settles all the news
  const settled = bent("lost", (s) => {
    allChecked(s);
    handedBack(s);
  });
  assert.deepEqual(walk(BR, settled), ["lost lost @doc"]);
  assert.ok(!walk(BR, bent("lost", (s) => (s.lost = []))).some((e) => e.startsWith("lost")));
});

test("two changes on one block are one stop, in news order", () => {
  const BR = load();
  const both = plain(BR.pure.walk(state("lost"))).filter((e) => e.kind === "change");
  assert.deepEqual(both.map((e) => e.id), ["9.0", "14.0"]);
  assert.deepEqual(distinct(both), ["b:5:1-6:68"]);
});

test("two removals in one gap are two stops, in news order", () => {
  const s = bent("unseen", (s) => {
    s.changes.push({ ...JSON.parse(JSON.stringify(s.changes.find((c) => c.id === "11.0"))), id: "11.1", idx: 1 });
    s.deletions.push({ ...JSON.parse(JSON.stringify(s.deletions.find((d) => d.change === "11.0"))), change: "11.1" });
    s.news.push({ id: "11.1", kind: "removal", sp: null, after_sp: "14:1-14:13", check: null });
  });
  const BR = load();
  assert.deepEqual(walk(BR, s).slice(-2), ["removal 11.0 @after 14:1-14:13", "removal 11.1 @after 14:1-14:13"]);
  assert.deepEqual(stopsOf(BR, s).slice(-2), ["r:11.0", "r:11.1"]);
});

// --- Membership: checked news stays until a hand-back settles it ----------------------------------

test("once everything is checked the walk keeps every stop, so n works on a second pass", () => {
  const BR = load();
  const s = bent("unseen", allChecked);
  assert.deepEqual(walk(BR, s), UNSEEN_WALK);
  // Each entry carries its check, and none is left to check
  assert.ok(plain(BR.pure.walk(s)).every((e) => e.check != null));
  assert.deepEqual(plain(BR.pure.attention(s)), []);
});

test("a hand-back after it settles what was checked: nothing is left to walk", () => {
  const s = bent("unseen", (s) => {
    allChecked(s);
    handedBack(s);
  });
  assert.deepEqual(walk(load(), s), []);
});

test("what a hand-back left unchecked is still walked", () => {
  const s = bent("unseen", (s) => {
    for (const n of s.news) if (n.id !== "10.0" && n.id !== "nh:5") n.check = "ok";
    handedBack(s);
  });
  assert.deepEqual(walk(load(), s), ["not_handled nh:5 @10:1-10:16", "change 10.0 @11:1-11:44"]);
});

test("an open question is walked, and stays walked once answered until a hand-back", () => {
  const BR = load();
  assert.ok(walk(BR, state("flow")).includes("question #21 @3:1-3:6"));
  const answered = bent("flow", (s) => (s.news.find((n) => n.id === "#21").check = "answered"));
  assert.ok(walk(BR, answered).includes("question #21 @3:1-3:6"));
  assert.ok(!plain(BR.pure.attention(answered)).some((e) => e.id === "#21"));
});

test("a state with nothing in its news has an empty walk, but for your unsent notes", () => {
  const BR = load();
  assert.deepEqual(walk(BR, state("fresh")), []);
  assert.deepEqual(walk(BR, state("drafts")), ["draft #6 @doc", "draft #4 @5:1-6:54", "draft #5 @10:1-10:16"]);
  assert.deepEqual(stopsOf(BR, state("drafts")), ["doc", "b:5:1-6:54", "b:10:1-10:16"]);
});

test("your whole-doc draft comes first in the doc slot, and a draft on a block of news shares its stop", () => {
  const BR = load();
  const human = { type: "human", via: "browser" };
  const s = bent("unseen", (s) => {
    s.items.push({ n: 30, id: "#30", by: human, body: "more", status: "draft", at: { kind: "doc" }, replies: [], ts: s.server_now });
    s.items.push({
      n: 31,
      id: "#31",
      by: human,
      body: "and here",
      status: "draft",
      at: { kind: "anchor", status: "ok", scope: "block", loc: { sp: "11:1-11:44" } },
      replies: [],
      ts: s.server_now,
    });
    s.counts.drafts = 2;
  });
  const entries = plain(BR.pure.walk(s));
  assert.deepEqual([entries[0].kind, entries[0].id, entries[0].stop], ["draft", "#30", "doc"]);
  const mine = entries.findIndex((e) => e.id === "#31");
  assert.equal(entries[mine].stop, "b:11:1-11:44");
  // Next to 10.0 on its block, so one n lands on both
  assert.ok(entries[mine - 1].id === "10.0" || entries[mine + 1].id === "10.0", JSON.stringify(entries));
  // A draft is never something to check
  assert.ok(!plain(BR.pure.attention(s)).some((e) => e.kind === "draft"));
});

test("the walk leaves the state as it found it", () => {
  const s = state("flow");
  const before = JSON.stringify(s);
  load().pure.walk(s);
  assert.equal(JSON.stringify(s), before);
});

test("the unseen set behind the tab's news is unchanged: its length is still the fold's count", () => {
  for (const name of ["unseen", "flow", "replied", "approved", "fresh", "drafts", "sent", "picked", "rewrite", "hold", "lost"]) {
    const s = state(name);
    // The tab's count is the fold's stops to check, which the walk reads from 6.21
    assert.equal(s.counts.attention, s.counts.to_check, name);
  }
});

test("the walk's stops holding something to check are the fold's to_check, per fixture", () => {
  const BR = load();
  for (const name of NAMES) {
    const s = state(name);
    const unchecked = plain(BR.pure.walk(s)).filter((e) => e.kind !== "draft" && e.kind !== "lost" && e.check == null);
    assert.equal(distinct(unchecked).length, s.counts.to_check, name);
    // The guard and the tab read the same stops as the walk
    assert.deepEqual(distinct(plain(BR.pure.attention(s))), distinct(unchecked), name);
  }
});

// --- The approve guard's message -----------------------------------------------------------------

const KEY = { key: "⌘⏎" };

test("with news to check the guard counts its stops and offers the walk and approve as is", () => {
  assert.equal(load().pure.guardPill(state("unseen"), KEY), "5 to check · n next · ⌘⏎ again approves as is");
  assert.equal(load().pure.guardPill(state("flow"), KEY), "9 to check · n next · ⌘⏎ again approves as is");
});

test("the guard counts stops, not items: the journey's twelve items are 9 to check", () => {
  assert.equal(load().pure.guardPill(state("journey/round1"), KEY), "9 to check · n next · ⌘⏎ again approves as is");
});

test("a reply alone still stops approve", () => {
  const s = bent("unseen", (s) => {
    for (const n of s.news) if (n.id !== "#13") n.check = n.kind === "change" || n.kind === "removal" ? "kept" : "ok";
    s.counts.to_check = 1;
  });
  assert.equal(load().pure.guardPill(s, KEY), "1 to check · n next · ⌘⏎ again approves as is");
});

test("an open question alone is one to check", () => {
  const s = bent("flow", (s) => {
    allChecked(s);
    s.news.find((n) => n.id === "#21").check = null;
    s.counts.to_check = 1;
    s.counts.drafts = 0;
  });
  assert.equal(load().pure.guardPill(s, KEY), "1 to check · n next · ⌘⏎ again approves as is");
});

test("the message names the key this platform shows", () => {
  assert.equal(load().pure.guardPill(state("unseen"), { key: "Ctrl⏎" }), "5 to check · n next · Ctrl⏎ again approves as is");
});

test("with nothing to check there is no stop", () => {
  assert.equal(load().pure.guardPill(state("fresh"), KEY), null);
  assert.equal(load().pure.guardPill(bent("unseen", allChecked), KEY), null);
  assert.equal(load().pure.guardPill(bent("flow", allChecked), KEY), null);
});

// --- The clean view's phrase --------------------------------------------------------------------

const phrase = (s) => load().pure.formatPhrase(s, { now: s.server_now });

test("an approved doc says how to start again", () => {
  assert.equal(phrase(bent("approved", (s) => (s.approve.later = 0))), "approved 14:13 · c to reopen");
});

test("changed since replaces c to reopen in the phrase; what waits for later goes to the tooltip", () => {
  assert.equal(phrase(state("approved")), "approved 14:13 · c to reopen");
  const s = state("approved");
  assert.equal(load().pure.phraseTitle(s, { now: s.server_now }), "approved 14:13 · 1 later · c to reopen");
  const changed = bent("approved", (s) => {
    s.approve.later = 0;
    s.approve.changed_since = true;
    s.approve.current_matches = false;
  });
  assert.equal(phrase(changed), "approved 14:13 · changed since");
});

test("inside the approve's hold the phrase is still the one from before it", () => {
  const s = state("approved");
  assert.ok(!load().pure.formatPhrase(s, { now: s.hold.not_before - 1_000 }).includes("c to reopen"));
});

// --- The button counts what the guard counts ----------------------------------------------------

/** unseen.json with its doc-level reply seen, so only its three changes are unseen. */
const changesOnly = () =>
  bent("unseen", (s) => {
    s.items.find((i) => i.n === 6).replies[0].seen = true;
    s.counts.attention -= 1;
  });

const button = (s) => plain(load().pure.turnButton(s, { now: s.server_now }));

test("the Approve button never counts: with things to check it is Approve with its key, and the guard counts", () => {
  assert.deepEqual(button(state("unseen")), { text: "Approve", kbd: true, disabled: false });
  assert.deepEqual(button(state("replied")), { text: "Approve", kbd: true, disabled: false });
  assert.deepEqual(button(changesOnly()), { text: "Approve", kbd: true, disabled: false });
});

test("an unchecked reply alone leaves the button a plain Approve, and the guard still stops the first press", () => {
  const s = bent("unseen", (s) => {
    for (const c of s.changes) c.seen = true;
    s.counts.unseen = 0;
  });
  assert.deepEqual(button(s), { text: "Approve", kbd: true, disabled: false });
  assert.match(load().pure.guardPill(s, KEY), /^\d+ to check · n next · /);
});

// --- Killers ------------------------------------------------------------------------------------

test("the guard reads the fold's count: it says what counts.to_check says", () => {
  const s = bent("unseen", (s) => (s.counts.to_check = 2));
  assert.equal(load().pure.guardPill(s, KEY), "2 to check · n next · ⌘⏎ again approves as is");
});

test("a seen change still in news is still walked, whatever the rounds say", () => {
  const s = bent("unseen", (s) => {
    s.changes.find((c) => c.id === "9.0").seen = true;
    s.rounds.push({ submit: 40, ts: s.server_now, by: "", to: "*", items: [], released: false });
  });
  assert.ok(walk(load(), s).includes("change 9.0 @5:1-6:68"));
});

test("on one stop the walk keeps news order: change, reply, then not-handled", () => {
  const s = bent("unseen", (s) => {
    // The not-handled row moves onto the changed paragraph, after its done in the list
    s.news.find((n) => n.id === "nh:5").sp = "5:1-6:68";
    const nh = s.news.splice(s.news.findIndex((n) => n.id === "nh:5"), 1)[0];
    s.news.splice(s.news.findIndex((n) => n.id === "#4") + 1, 0, nh);
  });
  assert.deepEqual(walk(load(), s).slice(2, 5), ["change 9.0 @5:1-6:68", "reply #4 @5:1-6:68", "not_handled nh:5 @5:1-6:68"]);
});

// --- Rulings ------------------------------------------------------------------------------------

test("an approval the doc moved on from can be approved again: the button is a plain Approve", () => {
  const s = bent("approved", (s) => {
    s.approve.changed_since = true;
    s.approve.current_matches = false;
  });
  assert.deepEqual(button(s), { text: "Approve", kbd: true, disabled: false });
  assert.equal(load().pure.clean(s, s.server_now), true);
});

test("a question answered by a verdict alone is checked: the guard doesn't stop for it, the walk keeps it until handed back", () => {
  const answered = (s) => {
    allChecked(s);
    s.counts.drafts = 0;
    const ask = s.items.find((i) => i.n === 21);
    ask.status = "answered";
    ask.replies = [{ n: 26, id: "#26", by: { type: "human", via: "browser" }, verdict: "yes", ts: s.server_now, seen: false }];
    s.counts.open_asks = 0;
  };
  const s = bent("flow", answered);
  assert.equal(load().pure.guardPill(s, KEY), null);
  assert.ok(walk(load(), s).includes("question #21 @3:1-3:6"));
  const sent = bent("flow", (s) => {
    answered(s);
    handedBack(s);
  });
  assert.ok(!walk(load(), sent).includes("question #21 @3:1-3:6"));
});

// --- Your revert -------------------------------------------------------------------------------

/** flow.json after your revert of 13.0 by write 40, which put the paragraph's words back. */
function reverted(s) {
  const c = s.changes.find((c) => c.id === "13.0");
  Object.assign(c, { undone_by: 40, settled: true, seen: true, check: "reverted", range: { ...c.range, status: "superseded", sp: null } });
  // An undone change can't be reverted again; the fold names a later overlap
  c.reversible = { ok: false, reason: "later_overlap" };
  s.changes.push({
    ...JSON.parse(JSON.stringify(c)),
    id: "40.0",
    op: 40,
    turn: null,
    attr: "human",
    undone_by: undefined,
    check: null,
    settled: false,
    seen: false,
    range: { status: "ok", lines: [5, 6], bytes: [99, 167], sp: "5:1-6:68" },
  });
  // News keeps it, checked, on the block your revert put back
  s.news.find((n) => n.id === "13.0").check = "reverted";
  s.counts.to_check -= 1;
}

test("a change you reverted stays a stop, at the block your revert put back, until a hand-back settles it", () => {
  const BR = load();
  const s = bent("flow", reverted);
  assert.ok(walk(BR, s).includes("change 13.0 @5:1-6:68"));
  assert.equal(plain(BR.pure.walk(s)).find((e) => e.id === "13.0").check, "reverted");
  // Your own write is never walked
  assert.ok(!walk(BR, s).some((e) => e.includes("40.0")));
  // Checked: the guard and the tab's dot pass it by
  assert.ok(!plain(BR.pure.attention(s)).some((e) => e.id === "13.0" || e.id === "40.0"));
  const after = bent("flow", (x) => {
    reverted(x);
    handedBack(x, 41);
  });
  assert.ok(!walk(BR, after).some((e) => e.includes("13.0")));
});

test("your revert's block keeps no wash: the revert checks the change, seen or not", () => {
  const BR = load();
  const washes = (s) => plain(BR.pure.laneMarks(s, { now: s.server_now })).filter((m) => m.mark === "wash").map((m) => m.sp);
  assert.ok(!washes(bent("flow", reverted)).includes("5:1-6:68"));
  // Reverted before it was seen: landing no longer marks seen, so the check decides
  const unseen = bent("flow", (s) => {
    reverted(s);
    s.changes.find((c) => c.id === "13.0").seen = false;
  });
  assert.ok(!washes(unseen).includes("5:1-6:68"));
});

// --- The lane: new or checked, and your revert's head ---------------------------------------------

const lane = (s) => plain(load().pure.laneMarks(s, { now: s.server_now }));

test("a bar is new while its change is unchecked, and checked once kept", () => {
  const bars = (s) => Object.fromEntries(lane(s).filter((m) => m.mark === "bar").map((m) => [m.sp, m.check]));
  assert.deepEqual(bars(state("unseen")), { "5:1-6:68": "new", "11:1-11:44": "new" });
  const kept = bent("unseen", (s) => {
    s.changes.find((c) => c.id === "9.0").check = "kept";
    s.news.find((n) => n.id === "9.0").check = "kept";
  });
  assert.deepEqual(bars(kept), { "5:1-6:68": "checked", "11:1-11:44": "new" });
  // A kept bar is muted, never the new colour
  assert.equal(lane(kept).find((m) => m.mark === "bar" && m.sp === "5:1-6:68").fresh, false);
});

test("a bar reads its change's check, not whether it was seen", () => {
  const check = (s) => lane(s).find((m) => m.mark === "bar" && m.sp === "5:1-6:68").check;
  // You replied to it without a y: checked, though never seen
  assert.equal(check(bent("unseen", (s) => (s.changes.find((c) => c.id === "9.0").check = "replied"))), "checked");
  // Seen yet unchecked does not happen in a fold, but the bar must not read seen
  assert.equal(check(bent("unseen", (s) => (s.changes.find((c) => c.id === "9.0").seen = true))), "new");
});

test("a change you reverted draws a ↺ head in place of your blue bar", () => {
  const marks = lane(bent("flow", reverted)).filter((m) => m.sp === "5:1-6:68");
  assert.ok(marks.some((m) => m.mark === "revert"), JSON.stringify(marks));
  assert.ok(!marks.some((m) => m.mark === "bar" && m.role === "you"), JSON.stringify(marks));
  // A settled revert keeps its head
  const settled = bent("flow", (s) => {
    reverted(s);
    handedBack(s, 41);
  });
  assert.ok(lane(settled).some((m) => m.mark === "revert" && m.sp === "5:1-6:68"));
});
