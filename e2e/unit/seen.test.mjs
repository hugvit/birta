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

/** The walk as `kind id @sp` strings, `@doc` for the doc slot. */
function walk(BR, s) {
  return plain(BR.pure.attention(s)).map((e) => `${e.kind} ${e.id} @${e.sp ?? "doc"}`);
}

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the attention walk loads without a DOM, ${name}`, () => {
    assert.equal(typeof load(files).pure.attention, "function");
  });
}

// --- The walk n and p take -------------------------------------------------------------------

test("unseen changes come first in doc order, then unseen replies, then not-handled rows", () => {
  // The doc-level reply sits above every block and the not-handled row above the second
  // change, so a walk in plain doc order would interleave them
  assert.deepEqual(walk(load(), state("unseen")), [
    "change 9.0 @5:1-6:68",
    "change 10.0 @11:1-11:44",
    // A removed block has no sp of its own: it is walked at the block above its tick
    "change 11.0 @14:1-14:13",
    "reply #13 @doc",
        "not_handled nh:5 @10:1-10:16",
  ]);
});

test("every kind in one state: changes, replies, open blanks, then not-handled rows", () => {
  assert.deepEqual(walk(load(), state("flow")), [
    // Outside writes count as unseen too; changes go by their line, not their op
    "change 13.0 @5:1-6:68",
    "change 26.0 @8:1-8:15",
    "change 15.0 @11:1-11:44",
    "change 19.0 @14:1-14:21",
    "change 27.0 @16:1-16:19",
    "reply #18 @doc",
    // Item 22 is answered, and your own reply to it is not news
    "blank #21 @3:1-3:6",
    // Notes that lost their place are walked in the doc slot, in line order
    "not_handled nh:4 @doc",
    "not_handled nh:5 @doc",
    "not_handled nh:7 @10:1-10:16",
  ]);
});

test("the walk has as many stops as the fold counts for attention", () => {
  const BR = load();
  for (const name of ["unseen", "flow", "replied", "approved", "fresh", "drafts", "sent", "picked", "rewrite", "hold", "lost"]) {
    const s = state(name);
    assert.equal(plain(BR.pure.attention(s)).length, s.counts.attention, name);
  }
});

test("a change you have seen, your own edit, and a settled change leave the walk", () => {
  const s = bent("unseen", (s) => {
    s.changes[0].seen = true;
    s.changes[1].attr = "human";
    s.changes[1].by = { type: "human", via: "browser" };
    s.changes[2].settled = true;
  });
  assert.deepEqual(walk(load(), s), ["reply #13 @doc", "not_handled nh:5 @10:1-10:16"]);
});

test("a reply you have seen leaves the walk", () => {
  const s = bent("unseen", (s) => (s.items.find((i) => i.n === 6).replies[0].seen = true));
  assert.deepEqual(walk(load(), s).filter((e) => e.startsWith("reply")), []);
});

test("a not-handled row you have acknowledged leaves the walk", () => {
  const s = bent("flow", (s) => (s.items.find((i) => i.n === 7).seen = true));
  assert.deepEqual(walk(load(), s).filter((e) => e.startsWith("not_handled")), ["not_handled nh:4 @doc", "not_handled nh:5 @doc"]);
});

test("an open blank stays in the walk until it is answered, whatever was seen", () => {
  const BR = load();
  const seen = bent("flow", (s) => (s.items.find((i) => i.n === 21).seen = true));
  assert.ok(walk(BR, seen).includes("blank #21 @3:1-3:6"));
  const answered = bent("flow", (s) => (s.items.find((i) => i.n === 21).status = "answered"));
  assert.ok(!walk(BR, answered).some((e) => e.startsWith("blank")));
});

test("a state with nothing new has an empty walk", () => {
  assert.deepEqual(walk(load(), state("fresh")), []);
  assert.deepEqual(walk(load(), state("drafts")), []);
});

test("the walk leaves the state as it found it", () => {
  const s = state("flow");
  const before = JSON.stringify(s);
  load().pure.attention(s);
  assert.equal(JSON.stringify(s), before);
});

// --- Refusals in words -----------------------------------------------------------------------

test("a read-only refusal on a writable doc blames the review file", () => {
  const BR = load();
  assert.equal(BR.pure.nackText("read_only", state("flow")), "review file is read-only");
  assert.equal(BR.pure.nackText("read_only", null), "review file is read-only");
});

test("an undo the server can't make reads as the receipt's own refusal", () => {
  assert.equal(load().pure.nackText("not_reversible", state("flow")), "can't undo this one · c instead");
});

// --- Killers ---------------------------------------------------------------------------------

test("a not-handled row the fold marks seen leaves the walk, and the length still matches the count", () => {
  const BR = load();
  const s = bent("unseen", (s) => {
    s.items.find((i) => i.n === 5).seen = true;
    s.counts.attention -= 1;
  });
  assert.deepEqual(walk(BR, s).filter((e) => e.startsWith("not_handled")), []);
  assert.equal(plain(BR.pure.attention(s)).length, s.counts.attention);
});

test("a removal at the very top walks in the doc slot, ahead of the other changes", () => {
  const s = bent("unseen", (s) => (s.deletions.find((d) => d.change === "11.0").after_sp = null));
  assert.deepEqual(walk(load(), s).slice(0, 3), ["change 11.0 @doc", "change 9.0 @5:1-6:68", "change 10.0 @11:1-11:44"]);
});

test("two changes on one block are two stops, in op order", () => {
  assert.deepEqual(
    walk(load(), state("lost")).filter((e) => e.startsWith("change")),
    ["change 9.0 @5:1-6:68", "change 14.0 @5:1-6:68"],
  );
});

test("only an agent's open ask is a blank", () => {
  const s = bent("flow", (s) => (s.items.find((i) => i.n === 21).by = { type: "human", via: "browser" }));
  assert.ok(!walk(load(), s).some((e) => e.startsWith("blank")));
});
