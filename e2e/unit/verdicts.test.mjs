// Times in the pill are local; pin the zone before anything reads the clock
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

function load() {
  const ctx = vm.createContext({});
  for (const f of ORDER) {
    const file = path.join(ASSETS, f);
    vm.runInContext(fs.readFileSync(file, "utf-8"), ctx, { filename: file });
  }
  return ctx.BR;
}

function state(name) {
  return JSON.parse(fs.readFileSync(path.join(FIXTURES, `${name}.json`), "utf-8"));
}

/** The server's default bindings for a review page: u is the change revert, and the raw view is ⌥u. */
const KB = {
  toggle_reading: "r",
  exit_reading: "Escape",
  toggle_dark: "d",
  focus_theme: "t",
  toggle_raw: "Alt+u",
  revert_change: "u",
  focus_next: "j",
  focus_prev: "k",
  note: "c",
  ask: "q",
  yes: "y",
  no: "x",
  later: "l",
  your_call: ".",
  next_item: "n",
  prev_item: "p",
  next_file: "Shift+J",
  prev_file: "Shift+K",
  undo: "z",
  hide_marks: "m",
  open: "Enter",
  turn: "Mod+Enter",
};

const EVENT = { code: "", shiftKey: false, altKey: false, ctrlKey: false, metaKey: false, repeat: false, isComposing: false, keyCode: 0 };

// --- C1: u is the change revert, and only that ------------------------------------------------

test("u asks for revert_change at its default binding, once per press", () => {
  const BR = load();
  const act = (init, kb = KB) => BR.pure.keyAction({ ...EVENT, ...init }, kb) ?? null;
  assert.equal(act({ key: "u" }), "revert_change");
  // Caps Lock types it upper case without Shift
  assert.equal(act({ key: "U" }), "revert_change");
  // A revert writes the doc, so a held key acts once
  assert.equal(act({ key: "u", repeat: true }), null);
  assert.equal(act({ key: "u", metaKey: true }), null);
  assert.equal(act({ key: "U", shiftKey: true }), null);
});

test("an older server's bindings, or a revert moved off u, leave u to the viewer", () => {
  const BR = load();
  const act = (init, kb) => BR.pure.keyAction({ ...EVENT, ...init }, kb) ?? null;
  const { revert_change, ...older } = KB;
  assert.equal(revert_change, "u");
  assert.equal(act({ key: "u" }, older), null);
  assert.equal(act({ key: "u" }, { ...KB, revert_change: "Shift+U" }), null);
  assert.equal(act({ key: "U", shiftKey: true }, { ...KB, revert_change: "Shift+U" }), "revert_change");
  assert.equal(act({ key: "u" }, { ...KB, revert_change: "" }), null);
});

// --- C4: the whole-doc stop -------------------------------------------------------------------

test("whole doc is a stop of its own: no H1 stands for it", () => {
  const BR = load();
  // A title is an ordinary block, so the H1 that spanned the doc is no stop for it any more
  assert.equal(BR.pure.wholeDocStop, undefined);
});

test("at the whole-doc stop each verdict key says what to do instead, and y names the button's act", () => {
  const BR = load();
  const pill = (action, text, mac = true) => BR.pure.wholeDocPill(action, text == null ? null : { text }, mac);
  assert.equal(pill("later", "Approve"), "later needs a block · j");
  assert.equal(pill("your_call", "Approve"), "your call needs a block · j");
  assert.equal(pill("no", "Approve"), "whole doc: c to say what's wrong");
  assert.equal(pill("yes", "Hand back · 2"), "⌘⏎ hands back");
  assert.equal(pill("yes", "Hand back · 2 · 1 file unseen"), "⌘⏎ hands back");
  assert.equal(pill("yes", "Approve"), "⌘⏎ approves");
  assert.equal(pill("yes", "Approve · 3 unseen"), "⌘⏎ approves");
  assert.equal(pill("yes", "Approve · 1 unanswered", false), "Ctrl⏎ approves");
  // A button that acts on nothing is read out; after approve the clean view's words win
  assert.equal(BR.pure.wholeDocPill("yes", { text: "With Claude", disabled: true }, true), "with Claude");
  assert.equal(BR.pure.wholeDocPill("yes", { text: "Approved ✓", disabled: true }, true), "approved · c to reopen");
  assert.equal(BR.pure.wholeDocPill("yes", null, true), null);
  // u there has nothing of Claude's to revert, whatever the button says
  assert.equal(pill("revert_change", "Approve"), "nothing here to revert");
  assert.equal(pill("revert_change", "Hand back · 2"), "nothing here to revert");
  assert.equal(BR.pure.wholeDocPill("revert_change", null, true), "nothing here to revert");
  // The keys it names are the bound ones
  const keys = { focus_next: "h", note: "w" };
  assert.equal(BR.pure.wholeDocPill("later", null, true, keys), "later needs a block · h");
  assert.equal(BR.pure.wholeDocPill("no", null, true, keys), "whole doc: w to say what's wrong");
});

test("pills name a key as its binding types it", () => {
  const BR = load();
  assert.equal(BR.pure.keyName("revert_change", KB), "u");
  assert.equal(BR.pure.keyName("revert_change", { ...KB, revert_change: "S" }), "s");
  assert.equal(BR.pure.keyName("open", KB), "⏎");
  assert.equal(BR.pure.keyName("undo", null), "z");
});

test("a label keeps six words and marks only a cut", () => {
  const BR = load();
  assert.equal(BR.pure.fewWords("one two three four five six"), "one two three four five six");
  assert.equal(BR.pure.fewWords("one two three four five six seven"), "one two three four five six…");
  assert.equal(BR.pure.fewWords("  spaced\n  out  "), "spaced out");
});

// --- C6: the hand-back counts answers ----------------------------------------------------------

/** flow.json with a hand-back on line 30 in its hold, holding `notes` notes. */
function handingBack(notes, edit = () => {}) {
  const s = state("flow");
  s.hold = { n: 30, kind: "hand_back", count: notes, not_before: s.server_now + 5_000 };
  s.rounds.push({ submit: 30, ts: s.server_now, by: "", to: "*", items: [], released: false });
  // The hand-back covers the earlier draft, so nothing is left written since it
  s.counts.drafts = 0;
  edit(s);
  return s;
}

test("the hand-back pill counts the answers it carries beside its notes", () => {
  const BR = load();
  const pill = (s) => BR.pure.holdPill(s, { now: s.server_now });
  // Reply 25 answers the agent's ask 22, after the earlier round and before this one
  assert.equal(pill(handingBack(1)), "handing back · 1 note · 1 answer · z to undo");
  assert.equal(pill(handingBack(0)), "handing back · 1 answer · z to undo");
  assert.equal(
    pill(handingBack(2, (s) => s.items.find((i) => i.n === 22).replies.push({ ...s.items.find((i) => i.n === 22).replies[0], n: 28 }))),
    "handing back · 2 notes · 2 answers · z to undo",
  );
  // An answer an earlier round already carried is not this hand-back's
  assert.equal(
    pill(handingBack(1, (s) => (s.items.find((i) => i.n === 22).replies[0].n = 9))),
    "handing back · 1 note · z to undo",
  );
  // A reply on your own note is not an answer
  assert.equal(
    pill(handingBack(1, (s) => {
      const ask = s.items.find((i) => i.n === 22);
      s.items.find((i) => i.n === 9).replies.push({ ...ask.replies[0], n: 27 });
      ask.replies = [];
    })),
    "handing back · 1 note · z to undo",
  );
});
