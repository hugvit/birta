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

/** Claude's news still to check, as `kind id @stop` strings. */
function attention(BR, s) {
  return plain(BR.pure.attention(s)).map((e) => `${e.kind} ${e.id} @${e.stop}`);
}

const NAMES = ["unseen", "flow", "replied", "approved", "fresh", "drafts", "sent", "picked", "rewrite", "hold", "lost", "journey/round1"];

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the attention walk loads without a DOM, ${name}`, () => {
    assert.equal(typeof load(files).pure.attention, "function");
  });
}

// --- What is left to check -------------------------------------------------------------------

test("what is left to check is every unchecked news item, in walk order, each on its stop", () => {
  assert.deepEqual(attention(load(), state("unseen")), [
    "reply #13 @doc",
    "turn_note #14 @doc",
    "change 9.0 @b:5:1-6:68",
    "reply #4 @b:5:1-6:68",
    "not_handled nh:5 @b:10:1-10:16",
    "change 10.0 @b:11:1-11:44",
    "removal 11.0 @r:11.0",
  ]);
});

test("every kind in one state: an answered question is checked, so it leaves; an open one stays", () => {
  assert.deepEqual(attention(load(), state("flow")), [
    "not_handled nh:4 @doc",
    "not_handled nh:5 @doc",
    "reply #18 @doc",
    "turn_note #23 @doc",
    "question #21 @q:#21",
    "change 13.0 @b:5:1-6:68",
    "change 26.0 @b:8:1-8:15",
    "not_handled nh:7 @b:10:1-10:16",
    "change 15.0 @b:11:1-11:44",
    "reply #6 @b:11:1-11:44",
    "reply #9 @b:12:1-13:0",
    "change 19.0 @b:14:1-14:21",
    "change 27.0 @b:16:1-16:19",
  ]);
});

test("the walk has as many stops as the fold counts for attention", () => {
  for (const name of ["unseen", "flow", "replied", "approved", "fresh", "drafts", "sent", "picked", "rewrite", "hold", "lost"]) {
    const s = state(name);
    // The tab's count is the fold's stops to check, which the walk reads from 6.21
    assert.equal(s.counts.attention, s.counts.to_check, name);
  }
});

test("the stops left to check are the fold's to_check, per fixture", () => {
  const BR = load();
  for (const name of NAMES) {
    const s = state(name);
    assert.equal(new Set(plain(BR.pure.attention(s)).map((e) => e.stop)).size, s.counts.to_check, name);
  }
});

test("a checked item leaves what is left to check, but stays walked", () => {
  const BR = load();
  const s = bent("unseen", (s) => {
    s.news.find((n) => n.id === "9.0").check = "kept";
    s.news.find((n) => n.id === "#13").check = "ok";
  });
  const left = attention(BR, s);
  assert.ok(!left.some((e) => e.includes(" 9.0 ") || e.includes(" #13 ")), left.join(", "));
  // #4 is still unchecked on 9.0's paragraph, so the stop is still to check
  assert.ok(left.includes("reply #4 @b:5:1-6:68"));
  const walked = plain(BR.pure.walk(s)).map((e) => e.id);
  assert.ok(walked.includes("9.0") && walked.includes("#13"));
});

test("every check counts as checked: kept, ok, reverted, restored, replied and answered", () => {
  const BR = load();
  for (const check of ["kept", "ok", "reverted", "restored", "replied", "answered"]) {
    const s = bent("unseen", (s) => (s.news.find((n) => n.id === "10.0").check = check));
    assert.ok(!attention(BR, s).some((e) => e.includes(" 10.0 ")), check);
  }
});

test("what is left to check follows news, not the seen flags on changes, replies or rows", () => {
  const BR = load();
  const s = bent("unseen", (s) => {
    for (const c of s.changes) c.seen = true;
    for (const i of s.items) {
      i.seen = true;
      for (const r of i.replies ?? []) r.seen = true;
    }
    s.counts.unseen = 0;
  });
  assert.equal(attention(BR, s).length, 7);
});

test("an open question stays to check until it is answered, whatever was seen", () => {
  const BR = load();
  const seen = bent("flow", (s) => (s.items.find((i) => i.n === 21).seen = true));
  assert.ok(attention(BR, seen).includes("question #21 @q:#21"));
  const answered = bent("flow", (s) => (s.news.find((n) => n.id === "#21").check = "answered"));
  assert.ok(!attention(BR, answered).some((e) => e.startsWith("question")));
});

test("a state with nothing new has nothing to check", () => {
  assert.deepEqual(attention(load(), state("fresh")), []);
  assert.deepEqual(attention(load(), state("drafts")), []);
});

test("what is left to check leaves the state as it found it", () => {
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

test("a not-handled row checked by y leaves what is left to check, and the stops still match the count", () => {
  const BR = load();
  const s = bent("unseen", (s) => {
    s.news.find((n) => n.id === "nh:5").check = "ok";
    s.counts.to_check -= 1;
  });
  assert.deepEqual(attention(BR, s).filter((e) => e.startsWith("not_handled")), []);
  assert.equal(new Set(plain(BR.pure.attention(s)).map((e) => e.stop)).size, s.counts.to_check);
});

test("a removal is a stop of its own, even at the very top", () => {
  const s = bent("unseen", (s) => {
    s.deletions.find((d) => d.change === "11.0").after_sp = null;
    s.news.find((n) => n.id === "11.0").after_sp = null;
  });
  assert.ok(attention(load(), s).includes("removal 11.0 @r:11.0"));
});

test("two changes on one block are two items on one stop", () => {
  assert.deepEqual(
    attention(load(), state("lost")).filter((e) => e.startsWith("change")),
    ["change 9.0 @b:5:1-6:68", "change 14.0 @b:5:1-6:68"],
  );
});

test("a reply on a note whose block Claude cut is to check on that removal's stop", () => {
  assert.ok(attention(load(), state("journey/round1")).includes("reply #6 @r:10.5"));
});
