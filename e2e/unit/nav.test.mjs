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
const ORDER = [
  "quote.js",
  "00-core.js",
  "10-keys.js",
  "20-pointing.js",
  "30-flow.js",
  "40-lane.js",
  "60-chrome.js",
  "70-dir.js",
];

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

/** A doc's `dir_state` summary as `review::summarize` builds it, with its live-note count. */
function summary(name, edit) {
  const s = bent(name, edit ?? (() => {}));
  const notes = s.items.filter((i) => i.by.type === "human" && !["retracted", "withdrawn"].includes(i.status)).length;
  return { stage: s.phrase.stage, phrase: s.phrase, counts: s.counts, notes };
}

function listed(paths, sum = null, news = false) {
  return { paths: [].concat(paths), summary: sum, news };
}

function dirState(files, totals = {}, folder = null) {
  return {
    type: "dir_state",
    root: "",
    truncated: false,
    folder,
    files,
    totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0, ...totals },
  };
}

const B = (text, kbd = true, disabled = false) => ({ text, kbd, disabled });

/** The defaults `KeybindingsConfig` serves, plus the change-revert key. */
const BINDINGS = {
  toggle_reading: "r",
  exit_reading: "Escape",
  toggle_dark: "d",
  focus_theme: "t",
  toggle_raw: "u",
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

/** A sheet as the lines it reads: each group's label and keys, then its notes. */
function lines(sheet) {
  return plain(sheet).flatMap((g) =>
    g.rows.map((r, i) => [i === 0 ? g.label : "", r.keys.join(" "), r.note ?? ""].filter(Boolean).join("  ")),
  );
}

test("the folder code loads the navigation helpers without a DOM", () => {
  for (const files of [ORDER, [...ORDER].reverse()]) {
    const BR = load(files);
    for (const fn of ["keySheet", "hasNotes", "nextFile"]) assert.equal(typeof BR.pure[fn], "function", fn);
  }
});

// --- The ? sheet ---------------------------------------------------------------------------------

test("the key sheet groups the default keys as the spec lists them", () => {
  const BR = load();
  assert.deepEqual(lines(BR.pure.keySheet(BINDINGS, true)), [
    "decide  y x l .",
    "write  c q",
    "move  j k n p ⇧J ⇧K Tab",
    "take back  z",
    "Claude's changes  u  revert Claude's change (open row)",
    "hand back / approve  ⌘⏎",
    "view  r d t m",
    "u  source view",
  ]);
});

test("off a Mac the turn key reads Ctrl⏎", () => {
  const BR = load();
  assert.ok(lines(BR.pure.keySheet(BINDINGS, false)).includes("hand back / approve  Ctrl⏎"));
});

test("the sheet names the keys as bound, and leaves out an unbound one", () => {
  const BR = load();
  const got = lines(BR.pure.keySheet({ ...BINDINGS, yes: "g", hide_marks: "", next_file: "Shift+L" }, true));
  assert.equal(got[0], "decide  g x l .");
  assert.equal(got[2], "move  j k n p ⇧L ⇧K Tab");
  assert.equal(got[6], "view  r d t");
});

test("the sheet leaves its bindings alone", () => {
  const BR = load();
  const before = JSON.stringify(BINDINGS);
  BR.pure.keySheet(BINDINGS, true);
  assert.equal(JSON.stringify(BINDINGS), before);
});

// --- The menu's keys row -------------------------------------------------------------------------

test("every writable doc's menu ends with a keys row, after the other files", () => {
  const BR = load();
  const dir = dirState([listed("pr.md", summary("drafts")), listed("b.md", summary("replied"))], { drafts: 3 });
  for (const name of ["fresh", "drafts", "sent", "picked", "replied", "approved", "hold"]) {
    const s = state(name);
    const rows = plain(BR.pure.menuRows(s, { now: s.server_now, dir, path: "pr.md", folder: true }));
    const last = rows[rows.length - 1];
    assert.deepEqual({ id: last.id, text: last.text, note: last.note }, { id: "keys", text: "keys", note: "?" }, name);
    assert.equal(last.op, undefined, name);
    assert.equal(rows.filter((r) => r.id === "keys").length, 1, name);
  }
  // Review keys do nothing in a folder birta can't write, and its menu stays empty
  const off = bent("drafts", (s) => (s.log_writable = false));
  assert.deepEqual(plain(BR.pure.menuRows(off, { now: off.server_now, dir, path: "pr.md" })), []);
});

// --- Shift+J and Shift+K, and link dots ----------------------------------------------------------

test("a file has notes when it holds drafts, or live notes on a round that isn't approved", () => {
  const BR = load();
  const has = (sum, news = false) => BR.pure.hasNotes(listed("b.md", sum, news));
  assert.equal(has(summary("drafts")), true);
  assert.equal(has(summary("sent")), true);
  assert.equal(has(summary("picked")), true);
  assert.equal(has(summary("replied")), true);
  // Approved: every note there is settled
  assert.equal(has(summary("approved")), false);
  assert.equal(has(summary("fresh")), false);
  assert.equal(has(null, true), false);
  assert.equal(has(null), false);
  // A summary from before the live-note count still counts its drafts
  assert.equal(has({ ...summary("drafts"), notes: undefined }), true);
  assert.equal(has({ ...summary("sent"), notes: undefined }), false);
  // Drafts after an approve open a new round
  assert.equal(has(summary("approved", (s) => (s.counts.drafts = 1))), true);
});

test("Shift+J's next file is the next with news, drafts or open notes; n's stays news only", () => {
  const BR = load();
  const dir = dirState([
    listed("news.md", null, true),
    listed("drafts.md", summary("drafts")),
    listed("pr.md", summary("drafts")),
    listed("fresh.md", summary("fresh")),
    listed("sent.md", summary("sent")),
    listed("approved.md", summary("approved")),
    listed("quiet.md"),
  ]);
  const shift = (p, step) => BR.pure.nextFile(dir, p, step, true);
  assert.equal(shift("pr.md", 1), "sent.md");
  assert.equal(shift("pr.md", -1), "drafts.md");
  assert.equal(shift("sent.md", 1), "news.md");
  assert.equal(shift("news.md", -1), "sent.md");
  // n and p cross only to news, as before
  assert.equal(BR.pure.nextFile(dir, "pr.md", 1), "news.md");
  assert.equal(BR.pure.nextFile(dir, "pr.md", -1), "news.md");
});

test("with only settled or quiet files elsewhere, Shift+J has nowhere to go", () => {
  const BR = load();
  const dir = dirState([listed("pr.md", summary("drafts"), true), listed("approved.md", summary("approved")), listed("quiet.md")]);
  assert.equal(BR.pure.nextFile(dir, "pr.md", 1, true), null);
  assert.equal(BR.pure.nextFile(dir, "pr.md", -1, true), null);
});

// --- N11: a folder with no drafter ---------------------------------------------------------------

/** A doc of yours in a folder: no drafter, no agent round, and `drafts` notes. */
function yours(drafts = 0) {
  return bent("fresh", (s) => {
    s.owner.drafted_by = null;
    s.rounds = [];
    s.counts.drafts = drafts;
  });
}

const folderOpts = (s, extra = {}) => ({ now: s.server_now, dir: dirState([]), path: "pr.md", folder: true, ...extra });

test("in a folder with no drafter and no agent, there is no Approve button until a note makes it a hand-back", () => {
  const BR = load();
  const btn = (s, extra) => plain(BR.pure.turnButton(s, folderOpts(s, extra)));
  assert.equal(btn(yours()), null);
  assert.deepEqual(btn(yours(1)), B("Hand back · 1"));
  assert.deepEqual(btn(yours(), { dir: dirState([], { drafts: 2 }) }), B("Hand back · 2"));
  assert.deepEqual(btn(yours(), { pending: 1, busy: true }), B("Hand back · 1"));
});

test("a drafter, or an agent that took a round, still gets Approve in a folder", () => {
  const BR = load();
  const btn = (s) => plain(BR.pure.turnButton(s, folderOpts(s)));
  assert.deepEqual(btn(state("fresh")), B("Approve"));
  const claimed = bent("replied", (s) => (s.owner.drafted_by = null));
  assert.match(btn(claimed).text, /^Approve/);
});

test("serving one file, a doc of yours keeps its Approve button, listing or not", () => {
  const BR = load();
  const s = yours();
  assert.deepEqual(plain(BR.pure.turnButton(s, { now: s.server_now, dir: null, path: "pr.md", folder: false })), B("Approve"));
  // Serving one file still lists other files, so the rule can't key on a listing
  const dir = dirState([listed("b.md", summary("fresh"))]);
  assert.deepEqual(plain(BR.pure.turnButton(s, { now: s.server_now, dir, path: "pr.md", folder: false })), B("Approve"));
  assert.deepEqual(plain(BR.pure.turnButton(s, { now: s.server_now, dir, path: "pr.md" })), B("Approve"));
});

test("in a folder with no drafter, the phrase offers the hand-off line once a hand-back sits unclaimed for 10 s", () => {
  const BR = load();
  const s = bent("sent", (x) => (x.owner.drafted_by = null));
  const sentAt = s.phrase.sent.ts;
  const phrase = (now, extra = {}) => BR.pure.formatPhrase(s, { now, dir: dirState([]), path: "pr.md", folder: true, ...extra });
  assert.equal(phrase(sentAt + 9_999), "sent 14:13 · not picked up yet");
  assert.equal(phrase(sentAt + 10_000), "sent 14:13 · not picked up yet · copy hand-off line");
  const others = dirState([listed("b.md", null, true)]);
  assert.equal(phrase(sentAt + 10_000, { dir: others }), "sent 14:13 · not picked up yet · copy hand-off line · 1 other file ▾");
  // Serving one file, or with a drafter, it never does
  assert.equal(phrase(sentAt + 60_000, { folder: false }), "sent 14:13 · not picked up yet");
  const drafted = state("sent");
  assert.equal(
    BR.pure.formatPhrase(drafted, { now: sentAt + 60_000, dir: dirState([]), path: "pr.md", folder: true }),
    "sent 14:13 · not picked up yet",
  );
  // A file row's note is the phrase alone
  assert.equal(BR.pure.fileNote(listed("b.md", summary("sent")), { now: sentAt + 60_000 }), "sent 14:13 · not picked up yet");
});

test("in a folder the button counts the other files' drafts and this doc's own, whichever came first", () => {
  const BR = load();
  // This doc's state shows its new note; the throttled listing still has it at none
  const s = bent("drafts", (x) => (x.counts.drafts = 1));
  const dir = dirState([listed("plan.md", summary("drafts", (x) => (x.counts.drafts = 1))), listed("pr.md", summary("fresh"))], { drafts: 1 });
  assert.deepEqual(plain(BR.pure.turnButton(s, folderOpts(s, { dir }))), B("Hand back · 2"));
  // Once the listing catches up the count stays
  const caught = dirState(
    [listed("plan.md", summary("drafts", (x) => (x.counts.drafts = 1))), listed("pr.md", summary("drafts", (x) => (x.counts.drafts = 1)))],
    { drafts: 2 },
  );
  assert.deepEqual(plain(BR.pure.turnButton(s, folderOpts(s, { dir: caught }))), B("Hand back · 2"));
});
