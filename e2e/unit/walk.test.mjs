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
    e.kind === "removal" ? `removal ${e.id} @after ${e.after_sp ?? "top"}${e.sp == null ? "" : ` (on ${e.sp})`}` : `${e.kind} ${e.id} @${e.sp ?? "doc"}`,
  );
}

/** Marks every change, agent reply and not-handled row seen, as walking them all would. */
function allSeen(s) {
  for (const c of s.changes) c.seen = true;
  for (const i of s.items) {
    for (const r of i.replies ?? []) if (r.by.type === "agent") r.seen = true;
    if (i.status === "not_handled") i.seen = true;
  }
  s.counts.attention -= s.counts.unseen;
  s.counts.unseen = 0;
}

/** A hand-back after everything in `s`, which settles whatever was seen before it. */
function handedBack(s, at = 40) {
  s.rounds.push({ submit: at, ts: s.server_now, by: "", to: "*", items: [], released: false });
  for (const c of s.changes) if (c.seen) c.settled = true;
}

// unseen.json, in doc order: the doc-level reply, then 9.0 on the paragraph, the not-handled
// bullet, 10.0 on the next bullet, and the removal in its gap under the last heading
const UNSEEN_WALK = [
  "reply #13 @doc",
  "change 9.0 @5:1-6:68",
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

// --- Order --------------------------------------------------------------------------------------

test("the walk is one top-to-bottom pass in doc order, every kind interleaved", () => {
  assert.deepEqual(walk(load(), state("unseen")), UNSEEN_WALK);
});

test("every kind in one state, in doc order: the doc slot first, then block by block", () => {
  assert.deepEqual(walk(load(), state("flow")), [
    // In the doc slot a reply goes before not-handled rows, those in line order
    "reply #18 @doc",
    "not_handled nh:4 @doc",
    "not_handled nh:5 @doc",
    // The lost-notes row closes the doc slot
    "lost lost @doc",
    // Your answer to it is not handed back yet, so the question is still this round's
    "blank #22 @1:1-1:16",
    "blank #21 @3:1-3:6",
    "change 13.0 @5:1-6:68",
    // Outside writes are walked too, at their own block
    "change 26.0 @8:1-8:15",
    "not_handled nh:7 @10:1-10:16",
    "change 15.0 @11:1-11:44",
    "change 19.0 @14:1-14:21",
    "change 27.0 @16:1-16:19",
  ]);
});

test("the lost-notes row is one stop at the end of the doc slot, while any note is lost", () => {
  const BR = load();
  const lost = walk(BR, state("lost"));
  assert.equal(lost.filter((e) => e.startsWith("lost")).length, 1);
  assert.equal(lost.filter((e) => e.endsWith("@doc")).at(-1), "lost lost @doc");
  // It stays reachable after a hand-back settles everything else
  const settled = bent("lost", (s) => {
    allSeen(s);
    handedBack(s);
  });
  assert.deepEqual(walk(BR, settled), ["lost lost @doc"]);
  assert.ok(!walk(BR, bent("lost", (s) => (s.lost = []))).some((e) => e.startsWith("lost")));
});

test("two changes on one block are two stops, in op order", () => {
  assert.deepEqual(
    walk(load(), state("lost")).filter((e) => e.startsWith("change")),
    ["change 9.0 @5:1-6:68", "change 14.0 @5:1-6:68"],
  );
});

test("on one block a change goes before a reply", () => {
  const s = bent("unseen", (s) => {
    // The doc-level note and its reply move onto the changed paragraph
    s.items.find((i) => i.n === 6).at = { kind: "anchor", status: "ok", scope: "block", loc: { sp: "5:1-6:68" } };
  });
  assert.deepEqual(walk(load(), s).slice(0, 2), ["change 9.0 @5:1-6:68", "reply #13 @5:1-6:68"]);
});

test("a removal at the very top walks in its gap above the first block, after whole doc's reply", () => {
  const s = bent("unseen", (s) => (s.deletions.find((d) => d.change === "11.0").after_sp = null));
  assert.deepEqual(walk(load(), s).slice(0, 3), ["reply #13 @doc", "removal 11.0 @after top", "change 9.0 @5:1-6:68"]);
});

test("a removal walks in its gap, after everything on the block above it", () => {
  const s = bent("unseen", (s) => {
    // A reply of Claude's on the heading the paragraph was cut from under
    s.items.find((i) => i.n === 6).at = { kind: "anchor", status: "ok", scope: "block", loc: { sp: "14:1-14:13" } };
  });
  assert.deepEqual(walk(load(), s).slice(-2), ["reply #13 @14:1-14:13", "removal 11.0 @after 14:1-14:13"]);
});

test("two removals in one gap are two stops, in op order", () => {
  const s = bent("unseen", (s) => {
    s.changes.push({ ...JSON.parse(JSON.stringify(s.changes.find((c) => c.id === "11.0"))), id: "11.1", idx: 1 });
    s.deletions.push({ ...JSON.parse(JSON.stringify(s.deletions.find((d) => d.change === "11.0"))), change: "11.1" });
  });
  assert.deepEqual(walk(load(), s).slice(-2), ["removal 11.0 @after 14:1-14:13", "removal 11.1 @after 14:1-14:13"]);
});

// --- Membership: the latest round stays, seen or not ---------------------------------------------

test("once everything is seen the walk keeps every stop of the round, so n works on a second pass", () => {
  assert.deepEqual(walk(load(), bent("unseen", allSeen)), UNSEEN_WALK);
});

test("a hand-back after it settles what was seen: nothing is left to walk", () => {
  const s = bent("unseen", (s) => {
    allSeen(s);
    handedBack(s);
  });
  assert.deepEqual(walk(load(), s), []);
});

test("what an earlier round left unseen is still walked after a hand-back", () => {
  const s = bent("unseen", (s) => {
    allSeen(s);
    handedBack(s);
    s.changes.find((c) => c.id === "10.0").seen = false;
    s.changes.find((c) => c.id === "10.0").settled = false;
    s.items.find((i) => i.n === 6).replies[0].seen = false;
    s.items.find((i) => i.n === 5).seen = false;
  });
  assert.deepEqual(walk(load(), s), ["reply #13 @doc", "not_handled nh:5 @10:1-10:16", "change 10.0 @11:1-11:44"]);
});

test("the fold's settled flag decides for changes, whatever the rounds say", () => {
  const s = bent("unseen", (s) => (s.changes.find((c) => c.id === "9.0").settled = true));
  assert.ok(!walk(load(), s).includes("change 9.0 @5:1-6:68"));
});

test("your own edits are never walked", () => {
  const s = bent("unseen", (s) => {
    const c = s.changes.find((c) => c.id === "10.0");
    c.attr = "human";
    c.by = { type: "human", via: "browser" };
  });
  assert.ok(!walk(load(), s).some((e) => e.includes("10.0")));
});

test("your own replies are never walked, only the agent's", () => {
  assert.ok(!walk(load(), state("flow")).includes("reply #25 @1:1-1:16"));
});

test("an open question is walked whatever was seen or handed back", () => {
  const s = bent("flow", (s) => {
    allSeen(s);
    handedBack(s);
    s.items.find((i) => i.n === 21).seen = true;
  });
  assert.ok(walk(load(), s).includes("blank #21 @3:1-3:6"));
});

test("a question whose answer went out with a hand-back leaves the walk", () => {
  const s = bent("flow", (s) => {
    allSeen(s);
    handedBack(s);
  });
  assert.ok(!walk(load(), s).some((e) => e.startsWith("blank #22")));
});

test("only an agent's ask is a blank", () => {
  const s = bent("flow", (s) => (s.items.find((i) => i.n === 21).by = { type: "human", via: "browser" }));
  assert.ok(!walk(load(), s).some((e) => e.startsWith("blank #21")));
});

test("a state with nothing in its round has an empty walk, but for your unsent notes", () => {
  assert.deepEqual(walk(load(), state("fresh")), []);
  assert.deepEqual(walk(load(), state("drafts")), ["draft #6 @doc", "draft #4 @5:1-6:54", "draft #5 @10:1-10:16"]);
});

test("the walk leaves the state as it found it", () => {
  const s = state("flow");
  const before = JSON.stringify(s);
  load().pure.walk(s);
  assert.equal(JSON.stringify(s), before);
});

test("the unseen set behind the tab's news is unchanged: its length is still the fold's count", () => {
  const BR = load();
  for (const name of ["unseen", "flow", "replied", "approved", "fresh", "drafts", "sent", "picked", "rewrite", "hold", "lost"]) {
    const s = state(name);
    assert.equal(plain(BR.pure.attention(s)).length, s.counts.attention, name);
  }
});

// --- The approve guard's pill -------------------------------------------------------------------

const KEY = { key: "⌘⏎" };

/** unseen.json with its doc-level reply seen, so only its three changes are unseen. */
const changesOnly = () =>
  bent("unseen", (s) => {
    s.items.find((i) => i.n === 6).replies[0].seen = true;
    s.counts.attention -= 1;
  });

test("with unseen changes the guard counts them and offers the walk and approve anyway", () => {
  assert.equal(load().pure.guardPill(changesOnly(), KEY), "3 unseen · n walk · ⌘⏎ again approves anyway");
});

test("an unseen reply alone still stops approve", () => {
  const s = bent("unseen", (s) => {
    for (const c of s.changes) c.seen = true;
    s.counts.unseen = 0;
  });
  assert.equal(load().pure.guardPill(s, KEY), "1 unseen · n walk · ⌘⏎ again approves anyway");
});

test("with nothing unseen, open questions get the same soft stop, named by who asked", () => {
  const one = bent("flow", (s) => {
    allSeen(s);
    s.counts.drafts = 0;
  });
  assert.equal(load().pure.guardPill(one, KEY), "Claude asked 1 question · n go · ⌘⏎ again approves anyway");
  const two = bent("flow", (s) => {
    allSeen(s);
    s.counts.drafts = 0;
    s.items.find((i) => i.n === 22).status = "open";
    s.items.find((i) => i.n === 22).replies = [];
    s.counts.open_asks = 2;
  });
  assert.equal(load().pure.guardPill(two, KEY), "Claude asked 2 questions · n go · ⌘⏎ again approves anyway");
});

test("unseen wins over open questions", () => {
  const s = bent("flow", (s) => (s.counts.drafts = 0));
  assert.match(load().pure.guardPill(s, KEY), / unseen · n walk · ⌘⏎ again approves anyway$/);
});

test("the pill names the key this platform shows", () => {
  assert.equal(load().pure.guardPill(changesOnly(), { key: "Ctrl⏎" }), "3 unseen · n walk · Ctrl⏎ again approves anyway");
});

test("with nothing unseen and no open question there is no stop", () => {
  for (const name of ["fresh", "approved"]) assert.equal(load().pure.guardPill(state(name), KEY), null, name);
  assert.equal(load().pure.guardPill(bent("unseen", allSeen), KEY), null);
});

// --- The clean view's phrase --------------------------------------------------------------------

const phrase = (s) => load().pure.formatPhrase(s, { now: s.server_now });

test("an approved doc says how to start again", () => {
  assert.equal(phrase(bent("approved", (s) => (s.approve.later = 0))), "approved 14:13 · c to reopen");
});

test("changed since stays in the phrase; what waits for later goes to the tooltip", () => {
  assert.equal(phrase(state("approved")), "approved 14:13 · c to reopen");
  const s = state("approved");
  assert.equal(load().pure.phraseTitle(s, { now: s.server_now }), "approved 14:13 · 1 later · c to reopen");
  const changed = bent("approved", (s) => {
    s.approve.later = 0;
    s.approve.changed_since = true;
    s.approve.current_matches = false;
  });
  assert.equal(phrase(changed), "approved 14:13 · changed since · c to reopen");
});

test("inside the approve's hold the phrase is still the one from before it", () => {
  const s = state("approved");
  assert.ok(!load().pure.formatPhrase(s, { now: s.hold.not_before - 1_000 }).includes("c to reopen"));
});

// --- The button counts what the guard counts ----------------------------------------------------

const button = (s) => plain(load().pure.turnButton(s, { now: s.server_now }));

test("the Approve button counts unseen replies with unseen changes", () => {
  assert.deepEqual(button(state("unseen")), { text: "Approve · 4 unseen", kbd: false, disabled: false });
  assert.deepEqual(button(state("replied")), { text: "Approve · 2 unseen", kbd: false, disabled: false });
  assert.deepEqual(button(changesOnly()), { text: "Approve · 3 unseen", kbd: false, disabled: false });
});

test("an unseen reply alone keeps the button from a plain Approve", () => {
  const s = bent("unseen", (s) => {
    for (const c of s.changes) c.seen = true;
    s.counts.unseen = 0;
  });
  assert.deepEqual(button(s), { text: "Approve · 1 unseen", kbd: false, disabled: false });
});

// --- Killers ------------------------------------------------------------------------------------

test("the guard pill counts unseen replies with unseen changes", () => {
  assert.equal(load().pure.guardPill(state("unseen"), KEY), "4 unseen · n walk · ⌘⏎ again approves anyway");
  assert.equal(load().pure.guardPill(state("flow"), KEY), "6 unseen · n walk · ⌘⏎ again approves anyway");
});

test("a seen change the fold leaves unsettled is still walked, whatever the rounds say", () => {
  const s = bent("unseen", (s) => {
    s.changes.find((c) => c.id === "9.0").seen = true;
    s.rounds.push({ submit: 40, ts: s.server_now, by: "", to: "*", items: [], released: false });
  });
  assert.ok(walk(load(), s).includes("change 9.0 @5:1-6:68"));
});

test("doc order goes by a stop's first line, and a tie on it by kind", () => {
  // A note on the whole list starts at its first item
  const whole = bent("unseen", (s) => (s.items.find((i) => i.n === 5).at.loc.sp = "10:1-14:13"));
  assert.deepEqual(walk(load(), whole).slice(2), ["not_handled nh:5 @10:1-14:13", "change 10.0 @11:1-11:44", "removal 11.0 @after 14:1-14:13"]);
  const tie = bent("unseen", (s) => {
    s.items.find((i) => i.n === 5).at.loc.sp = "10:1-14:13";
    s.changes.find((c) => c.id === "10.0").range.sp = "10:1-10:16";
  });
  assert.deepEqual(walk(load(), tie).slice(2, 4), ["change 10.0 @10:1-10:16", "not_handled nh:5 @10:1-14:13"]);
});

test("on one block a reply goes before a blank, and a blank before a not-handled row", () => {
  const s = bent("flow", (s) => {
    s.items.find((i) => i.n === 21).at.loc.sp = "10:1-10:16";
    s.items.find((i) => i.n === 8).at = { kind: "anchor", status: "ok", scope: "block", loc: { sp: "10:1-10:16" } };
  });
  const all = walk(load(), s);
  const at = all.indexOf("reply #18 @10:1-10:16");
  assert.deepEqual(all.slice(at, at + 3), ["reply #18 @10:1-10:16", "blank #21 @10:1-10:16", "not_handled nh:7 @10:1-10:16"]);
});

test("the soft stop names whichever agent asked", () => {
  const s = bent("flow", (s) => {
    allSeen(s);
    s.counts.drafts = 0;
    s.items.find((i) => i.n === 21).by.name = "codex";
  });
  assert.equal(load().pure.guardPill(s, KEY), "Codex asked 1 question · n go · ⌘⏎ again approves anyway");
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

test("a blank answered by a verdict alone is answered: the guard doesn't stop for it, the walk keeps it until handed back", () => {
  const answered = (s) => {
    allSeen(s);
    s.counts.drafts = 0;
    const ask = s.items.find((i) => i.n === 21);
    ask.status = "answered";
    ask.replies = [{ n: 26, id: "#26", by: { type: "human", via: "browser" }, verdict: "yes", ts: s.server_now, seen: false }];
    s.counts.open_asks = 0;
  };
  const s = bent("flow", answered);
  assert.equal(load().pure.guardPill(s, KEY), null);
  assert.ok(walk(load(), s).includes("blank #21 @3:1-3:6"));
  const sent = bent("flow", (s) => {
    answered(s);
    handedBack(s);
  });
  assert.ok(!walk(load(), sent).includes("blank #21 @3:1-3:6"));
});

// --- Your revert -------------------------------------------------------------------------------

/** flow.json after your revert of 13.0 by write 40, which put the paragraph's words back. */
function reverted(s) {
  const c = s.changes.find((c) => c.id === "13.0");
  Object.assign(c, { undone_by: 40, settled: true, seen: true, range: { ...c.range, status: "superseded", sp: null } });
  s.changes.push({
    ...JSON.parse(JSON.stringify(c)),
    id: "40.0",
    op: 40,
    turn: null,
    attr: "human",
    undone_by: undefined,
    settled: false,
    seen: false,
    range: { status: "ok", lines: [5, 6], bytes: [99, 167], sp: "5:1-6:68" },
  });
}

test("a change you reverted stays a stop, at the block your revert put back, until a hand-back covers the revert", () => {
  const BR = load();
  const s = bent("flow", reverted);
  assert.ok(walk(BR, s).includes("change 13.0 @5:1-6:68"));
  // Never news: the guard and the tab's dot pass it by
  assert.equal(plain(BR.pure.walk(s)).find((e) => e.id === "13.0").unseen, false);
  assert.ok(!plain(BR.pure.attention(s)).some((e) => e.id === "13.0" || e.id === "40.0"));
  const after = bent("flow", (x) => {
    reverted(x);
    handedBack(x, 41);
  });
  assert.ok(!walk(BR, after).some((e) => e.includes("13.0")));
});

test("your revert's block keeps no wash once the change it reverted was seen", () => {
  const BR = load();
  const washes = (s) => plain(BR.pure.laneMarks(s, { now: s.server_now })).filter((m) => m.mark === "wash").map((m) => m.sp);
  assert.ok(!washes(bent("flow", reverted)).includes("5:1-6:68"));
  // Reverted before it was seen: the write still washes
  const unseen = bent("flow", (s) => {
    reverted(s);
    s.changes.find((c) => c.id === "13.0").seen = false;
  });
  assert.ok(washes(unseen).includes("5:1-6:68"));
});
