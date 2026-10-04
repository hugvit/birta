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

/*
 * The contract under test:
 *
 *   BR.pure.stripText(state, focus, opts) -> {where, keys}
 *
 * focus, what holds it (the page builds the breadcrumb parts from headingOver/blockLabel):
 *   {k: "doc"}                                          whole doc
 *   {k: "block", sp, heading: "## Why", kind: "paragraph"}
 *   {k: "removal", change: "11.0", heading: "## What", kind: "list item"}
 *   {k: "blank", id: "#16"}                             Claude's question
 * opts:
 *   now        server time, for holds and the approved view
 *   mode       null | "raw" | "reading" | "control"
 *   composer   the open composer's label, or null
 *   button     the turn button, {text, kbd, disabled}, or null
 *   mac        whether ⌘ names the turn key
 *   selection  whether a selection outranks the stop
 *   stopped    whether birta went away
 *
 * `N of M` counts walk STOPS in `state.news` (Part 2), checked ones included until they settle:
 * items on one stop number once (whole doc's reply and turn note are one stop), and each removal
 * and each question is its own stop. M is not `news.length`.
 */

const NOW = 1790000600000;

/** Nine news items in walk order, one per stop, as Part 2 names them. */
const NEWS = () => [
  { id: "#20", kind: "turn_note", sp: null, check: null },
  { id: "9.0", kind: "change", sp: "5:1-6:68", check: null },
  { id: "10.0", kind: "change", sp: "10:1-10:16", check: null },
  { id: "11.0", kind: "removal", sp: null, after_sp: "10:1-10:16", check: null },
  { id: "#15", kind: "reply", sp: "20:1-20:30", check: null },
  { id: "#16", kind: "question", sp: "20:1-20:30", check: null },
  { id: "nh:21", kind: "not_handled", sp: "21:1-21:20", check: null },
  { id: "#23", kind: "answer", sp: "22:1-22:30", check: null },
  { id: "12.0", kind: "change", sp: "24:1-24:10", check: null },
];

/** Claude's change records behind NEWS, so a check can be set on both. */
const CHANGES = () =>
  ["9.0", "10.0", "11.0", "12.0"].map((id) => ({
    id,
    op: Number(id.split(".")[0]),
    idx: 0,
    attr: "agent",
    by: { type: "agent", name: "claude" },
    range: id === "11.0" ? null : { status: "ok", sp: NEWS().find((n) => n.id === id).sp },
    seen: false,
    settled: false,
    check: null,
  }));

/** Claude replied, with NEWS to check. */
const replied = (edit) =>
  bent("fresh", (s) => {
    s.phrase = state("unseen").phrase;
    s.news = NEWS();
    s.changes = CHANGES();
    s.counts.to_check = s.news.length;
    edit?.(s);
  });

/** A doc with nothing of Claude's to check: Claude's draft, before you write. */
const quiet = (edit) =>
  bent("fresh", (s) => {
    s.news = [];
    edit?.(s);
  });

/** One human note on `sp`, in `status`. */
const note = (n, sp, status) => ({
  n,
  op: "comment",
  by: { type: "human", via: "browser" },
  cid: `c-${n}`,
  ts: NOW - 1000,
  at: sp ? { kind: "anchor", status: "ok", scope: "block", loc: { sp } } : { kind: "doc", loc: null },
  body: "say why",
  status,
  amended: [],
  replies: [],
});

const DOC = { k: "doc" };
const WHY = { k: "block", sp: "5:1-6:68", heading: "## Why", kind: "paragraph" };
const TESTING = { k: "block", sp: "20:1-20:30", heading: "## Testing", kind: "list item" };
const OPTS = { now: NOW, mode: null, composer: null, button: { text: "Approve", kbd: true, disabled: false }, mac: true, selection: false, stopped: false };

const strip = (BR, s, focus, opts = {}) => plain(BR.pure.stripText(s, focus, { ...OPTS, ...opts }));
const line = (r) => `${r.where} | ${r.keys}`;

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the strip's text loads without a DOM, ${name}`, () => {
    assert.equal(typeof load(files).pure.stripText, "function");
  });
}

// --- The strip table, row by row ----------------------------------------------------------------

test("whole doc, nothing there", () => {
  const BR = load();
  assert.equal(line(strip(BR, quiet(), DOC)), "whole doc | c note on the whole doc · j down · ? keys");
});

test("whole doc, Claude's news there", () => {
  const BR = load();
  assert.equal(line(strip(BR, replied(), DOC)), "1 of 9 · whole doc · Claude replied | y ok · c reply · n next");
});

test("a plain block", () => {
  const BR = load();
  assert.equal(line(strip(BR, quiet(), WHY)), "## Why › paragraph | c note · q ask · x cut · y keep as is · ? more");
  // On a doc Claude replied to, a block with none of its news is still plain
  assert.equal(
    line(strip(BR, replied(), { k: "block", sp: "7:1-7:10", heading: "## Why", kind: "paragraph" })),
    "## Why › paragraph | c note · q ask · x cut · y keep as is · ? more",
  );
});

test("a block with your note", () => {
  const BR = load();
  const s = quiet((s) => {
    s.phrase = { stage: "drafts" };
    s.items = [note(4, "5:1-6:68", "draft")];
    s.counts.drafts = 1;
  });
  assert.equal(line(strip(BR, s, WHY)), "## Why › paragraph · your note | c edit note · z undo · ? more");
});

test("a dotted block", () => {
  const BR = load();
  for (const status of ["handled", "answered", "later"]) {
    const s = quiet((s) => {
      s.items = [note(4, "5:1-6:68", status)];
    });
    assert.equal(line(strip(BR, s, WHY)), "## Why › paragraph · settled note | ⏎ open note · ? more", status);
  }
});

test("Claude's change, not checked", () => {
  const BR = load();
  assert.equal(
    line(strip(BR, replied(), WHY)),
    "2 of 9 · ## Why › paragraph · Claude's change | y keep · u revert · c reply · n next",
  );
});

// Interim until 6.21: a kept or reverted change shows the not-checked keys. The table's own
// copy, `kept · u revert · c reply · n next` and `reverted · u restore · c reply · n next`, lands then.
for (const check of ["kept", "reverted"]) {
  test(`${check === "kept" ? "Kept" : "Reverted"}: the where reads the same, and the keys are the not-checked ones until 6.21`, () => {
    const BR = load();
    const s = replied((s) => {
      s.changes.find((c) => c.id === "9.0").check = check;
      s.news.find((n) => n.id === "9.0").check = check;
      s.counts.to_check -= 1;
    });
    assert.equal(
      line(strip(BR, s, WHY)),
      "2 of 9 · ## Why › paragraph · Claude's change | y keep · u revert · c reply · n next",
    );
  });
}

test("a removal", () => {
  const BR = load();
  const focus = { k: "removal", change: "11.0", heading: "## What", kind: "list item" };
  assert.equal(
    line(strip(BR, replied(), focus)),
    "4 of 9 · ## What › removed list item | y keep the cut · u put it back · c reply · n next",
  );
});

test("Claude's reply or a not-handled row", () => {
  const BR = load();
  assert.equal(line(strip(BR, replied(), TESTING)), "5 of 9 · ## Testing › list item · Claude replied | y ok · c reply · n next");
  const nh = { k: "block", sp: "21:1-21:20", heading: "## Testing", kind: "list item" };
  assert.equal(line(strip(BR, replied(), nh)), "7 of 9 · ## Testing › list item · Claude replied | y ok · c reply · n next");
  // An answer to your question is a reply too
  const answer = { k: "block", sp: "22:1-22:30", heading: "## Testing", kind: "list item" };
  assert.equal(line(strip(BR, replied(), answer)), "8 of 9 · ## Testing › list item · Claude replied | y ok · c reply · n next");
});

test("Claude's question", () => {
  const BR = load();
  assert.equal(line(strip(BR, replied(), { k: "blank", id: "#16" })), "6 of 9 · Claude's question | ⏎ answer · y yes · x no · n next");
});

test("the last of the walk is 9 of 9", () => {
  const BR = load();
  const last = { k: "block", sp: "24:1-24:10", heading: "## Testing", kind: "list item" };
  assert.equal(strip(BR, replied(), last).where, "9 of 9 · ## Testing › list item · Claude's change");
});

test("the position counts the news list as it stands, checked items included until they settle", () => {
  const BR = load();
  // Two checked, and one settled out of the list by a later hand-back
  const s = replied((s) => {
    s.news[1].check = "kept";
    s.news[4].check = "ok";
    s.news.splice(2, 1);
    s.counts.to_check = 6;
  });
  assert.equal(strip(BR, s, WHY).where, "2 of 8 · ## Why › paragraph · Claude's change");
  assert.equal(strip(BR, s, TESTING).where, "4 of 8 · ## Testing › list item · Claude replied");
});

test("the composer: its label, and the ⌘⏎ part follows the button", () => {
  const BR = load();
  const label = 'note on "we just fail"';
  const hand = { text: "Hand back · 2", kbd: true, disabled: false };
  assert.equal(
    line(strip(BR, quiet(), WHY, { composer: label, button: hand })),
    `${label} | ⏎ save · ⇧⏎ new line · ⌘⏎ save & hand back · esc save & close`,
  );
  // Dropped when the button is disabled
  const off = { text: "With Claude", kbd: false, disabled: true };
  assert.equal(line(strip(BR, quiet(), WHY, { composer: label, button: off })), `${label} | ⏎ save · ⇧⏎ new line · esc save & close`);
  // The composer outranks Claude's news on its stop
  assert.equal(
    line(strip(BR, replied(), WHY, { composer: "reply to Claude's change on ## Why › paragraph", button: hand })),
    "reply to Claude's change on ## Why › paragraph | ⏎ save · ⇧⏎ new line · ⌘⏎ save & hand back · esc save & close",
  );
});

test("with Claude: from the hand-back's end to the reply", () => {
  const BR = load();
  const focus = { k: "block", sp: "10:1-10:16", heading: "## Testing", kind: "list item" };
  // sent.json's hold ended long before NOW
  const sent = state("sent");
  assert.equal(line(strip(BR, sent, focus)), "## Testing › list item · with Claude | c note for next hand-back · ? keys");
  const picked = bent("sent", (s) => {
    s.hold = null;
    s.phrase = { stage: "picked_up", sent: s.phrase.sent, picked_up: { n: 8, ts: NOW - 60000, by: "claude" } };
  });
  assert.equal(line(strip(BR, picked, focus)), "## Testing › list item · with Claude | c note for next hand-back · ? keys");
  assert.equal(line(strip(BR, picked, WHY)), "## Why › paragraph · with Claude | c note for next hand-back · ? keys");
});

test("inside the hand-back's hold it is not with Claude yet", () => {
  const BR = load();
  const s = state("sent");
  const r = strip(BR, s, WHY, { now: s.hold.not_before - 2000 });
  assert.doesNotMatch(r.where, /with Claude/);
});

test("birta stopped", () => {
  const BR = load();
  assert.equal(line(strip(BR, quiet(), WHY, { stopped: true })), "birta stopped | notes wait here · ? keys");
  assert.equal(line(strip(BR, replied(), DOC, { stopped: true })), "birta stopped | notes wait here · ? keys");
});

test("source view, reading mode and page controls", () => {
  const BR = load();
  for (const s of [quiet(), replied(), state("sent")]) {
    assert.equal(line(strip(BR, s, WHY, { mode: "raw" })), "source view | esc back");
    assert.equal(line(strip(BR, s, WHY, { mode: "reading" })), "reading mode | esc back");
    assert.equal(line(strip(BR, s, WHY, { mode: "control" })), "on page controls | esc back to the doc");
  }
});

test("approved", () => {
  const BR = load();
  const s = state("approved");
  assert.equal(line(strip(BR, s, WHY)), "approved | c reopen · ? keys");
  assert.equal(line(strip(BR, s, DOC)), "approved | c reopen · ? keys");
});

test("a selection's hint", () => {
  const BR = load();
  const r = strip(BR, quiet(), WHY, { selection: true });
  assert.equal(r.keys, "c note · q ask · x cut these words");
  assert.equal(r.where, "## Why › paragraph");
});

// --- Interim until 6.21: with no `news` in the state, the walk gives the position -----------------

test("with no news list, a stop on the walk takes its place from the walk, and drafts don't count", () => {
  const BR = load();
  const s = state("unseen");
  // The page hands the strip the state without `news` until the walk reads it
  delete s.news;
  // unseen.json walks #13 on whole doc, 9.0, nh:5, 10.0, then the removal 11.0 in its gap
  assert.equal(strip(BR, s, DOC).where, "1 of 5 · whole doc · Claude replied");
  assert.equal(strip(BR, s, WHY).where, "2 of 5 · ## Why › paragraph · Claude's change");
  const nh = { k: "block", sp: "10:1-10:16", heading: "## What changes", kind: "list item" };
  assert.equal(strip(BR, s, nh).where, "3 of 5 · ## What changes › list item · Claude replied");
  const bullet = { k: "block", sp: "11:1-11:44", heading: "## What changes", kind: "list item" };
  assert.equal(line(strip(BR, s, bullet)), "4 of 5 · ## What changes › list item · Claude's change | y keep · u revert · c reply · n next");
  // A draft of yours joins the walk but not the count
  const drafted = bent("unseen", (s) => {
    s.items.push(note(30, "12:1-12:20", "draft"));
    s.counts.drafts = 1;
  });
  assert.equal(strip(BR, drafted, bullet).where, "4 of 5 · ## What changes › list item · Claude's change");
  const mine = { k: "block", sp: "12:1-12:20", heading: "## What changes", kind: "list item" };
  assert.equal(strip(BR, drafted, mine).where, "## What changes › list item · your note");
});

test("with no news list, a removal is a walk stop of its own in its gap, apart from the block above", () => {
  const BR = load();
  const s = state("unseen");
  delete s.news;
  const removal = { k: "removal", change: "11.0", heading: "## Stacked on", kind: "paragraph" };
  assert.equal(line(strip(BR, s, removal)), "5 of 5 · ## Stacked on › removed paragraph | y keep the cut · u put it back · c reply · n next");
  // The heading above the gap holds nothing of Claude's
  const heading = { k: "block", sp: "14:1-14:13", heading: "## Stacked on", kind: null };
  assert.equal(strip(BR, s, heading).where, "## Stacked on");
});

// --- The walk: your drafts join it ---------------------------------------------------------------

/** The n/p walk as `kind id @sp` strings, `@doc` for the doc slot and `@after sp` for a removal's gap. */
const walk = (BR, s) =>
  plain(BR.pure.walk(s)).map((e) => (e.kind === "removal" ? `removal ${e.id} @after ${e.after_sp ?? "top"}` : `${e.kind} ${e.id} @${e.sp ?? "doc"}`));

test("your own draft notes join the walk, in doc order", () => {
  const BR = load();
  const s = state("drafts");
  const w = walk(BR, s);
  for (const want of ["@doc", "@5:1-6:54", "@10:1-10:16"]) assert.ok(w.some((e) => e.endsWith(want)), `${want} in ${w.join(", ")}`);
  assert.equal(w.length, 3);
  assert.deepEqual(
    w.map((e) => e.split(" @")[1]),
    ["doc", "5:1-6:54", "10:1-10:16"],
  );
});

test("a sent note or a settled one is no draft, so it is not walked", () => {
  const BR = load();
  assert.deepEqual(walk(BR, state("sent")), []);
  assert.deepEqual(walk(BR, state("fresh")), []);
  // unseen.json's notes are handled or not handled; only Claude's news walks
  assert.deepEqual(walk(BR, state("unseen")), [
    "reply #13 @doc",
    "change 9.0 @5:1-6:68",
    "not_handled nh:5 @10:1-10:16",
    "change 10.0 @11:1-11:44",
    "removal 11.0 @after 14:1-14:13",
  ]);
});

test("a draft sits in doc order among Claude's news", () => {
  const BR = load();
  const s = bent("unseen", (s) => {
    s.items.push(note(30, "11:1-11:44", "draft"));
    s.items.push(note(31, null, "draft"));
    s.counts.drafts = 2;
  });
  const at = walk(BR, s).map((e) => e.split(" @")[1]);
  assert.deepEqual(at, ["doc", "doc", "5:1-6:68", "10:1-10:16", "11:1-11:44", "11:1-11:44", "after 14:1-14:13"]);
});

// --- Adversary: N of M counts stops, not items ----------------------------------------------------

/** The journey's shape: whole doc holds a reply and the turn note, so 10 items sit on 9 stops. */
const docReplied = (edit) =>
  replied((s) => {
    s.news.unshift({ id: "#19", kind: "reply", sp: null, check: null });
    edit?.(s);
  });

test("adversary: whole doc's reply and turn note are one stop, so 10 items read as 9 stops", () => {
  const BR = load();
  const s = docReplied();
  assert.equal(s.news.length, 10);
  assert.equal(strip(BR, s, DOC).where, "1 of 9 · whole doc · Claude replied");
  assert.equal(strip(BR, s, WHY).where, "2 of 9 · ## Why › paragraph · Claude's change");
  const removal = { k: "removal", change: "11.0", heading: "## What", kind: "list item" };
  assert.equal(strip(BR, s, removal).where, "4 of 9 · ## What › removed list item");
  assert.equal(strip(BR, s, { k: "blank", id: "#16" }).where, "6 of 9 · Claude's question");
  const last = { k: "block", sp: "24:1-24:10", heading: "## Testing", kind: "list item" };
  assert.equal(strip(BR, s, last).where, "9 of 9 · ## Testing › list item · Claude's change");
});

test("adversary: a whole doc checked by one y keeps its place, and M stays the stop count", () => {
  const BR = load();
  // J-08: y on whole doc checks both its items; the strip still reads 1 of 9, and the next stop 2 of 9
  const s = docReplied((s) => {
    s.news[0].check = "ok";
    s.news[1].check = "ok";
    s.counts.to_check = 8;
  });
  assert.equal(strip(BR, s, DOC).where, "1 of 9 · whole doc · Claude replied");
  assert.equal(strip(BR, s, WHY).where, "2 of 9 · ## Why › paragraph · Claude's change");
});

test("adversary: a change and a reply on one block number once", () => {
  const BR = load();
  const s = replied((s) => {
    s.news.splice(2, 0, { id: "#31", kind: "reply", sp: "5:1-6:68", check: null });
  });
  assert.equal(s.news.length, 10);
  assert.equal(strip(BR, s, WHY).where, "2 of 9 · ## Why › paragraph · Claude's change");
  const next = { k: "block", sp: "10:1-10:16", heading: "## What", kind: "list item" };
  assert.match(strip(BR, s, next).where, /^3 of 9 · /);
});

test("adversary: two removals in one gap are two stops, not one", () => {
  const BR = load();
  const s = replied((s) => {
    s.news.splice(4, 0, { id: "13.0", kind: "removal", sp: null, after_sp: "10:1-10:16", check: null });
    s.changes.push({ ...s.changes.find((c) => c.id === "11.0"), id: "13.0", op: 13 });
  });
  const first = { k: "removal", change: "11.0", heading: "## What", kind: "list item" };
  const second = { k: "removal", change: "13.0", heading: "## What", kind: "list item" };
  assert.equal(strip(BR, s, first).where, "4 of 10 · ## What › removed list item");
  assert.equal(strip(BR, s, second).where, "5 of 10 · ## What › removed list item");
});

test("adversary: interim, two changes on one paragraph and the lost row on whole doc count as stops", () => {
  const BR = load();
  const s = state("lost");
  delete s.news;
  // #11 and the lost row on whole doc, 9.0 and 14.0 on the paragraph, nh:5 on the bullet: three stops of news
  assert.match(strip(BR, s, DOC).where, /^1 of 3 · whole doc/);
  const para = { k: "block", sp: "5:1-6:68", heading: "## Why", kind: "paragraph" };
  assert.match(strip(BR, s, para).where, /^2 of 3 · ## Why › paragraph/);
  const nh = { k: "block", sp: "10:1-10:16", heading: "## What changes", kind: "list item" };
  assert.match(strip(BR, s, nh).where, /^3 of 3 · ## What changes › list item/);
});

test("adversary: with Claude on whole doc offers a note for the next hand-back", () => {
  const BR = load();
  const r = strip(BR, state("sent"), DOC);
  assert.equal(r.keys, "c note for next hand-back · ? keys");
  // The where is derived from the table's block row, so only its tail is pinned
  assert.match(r.where, /with Claude$/);
});
