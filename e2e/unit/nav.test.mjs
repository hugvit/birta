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

/** The defaults `KeybindingsConfig` serves a review page: the source view on ⌥u, off the revert's u. */
const BINDINGS = {
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
  focus_first: "Home",
  focus_last: "End",
};

test("the folder code loads the navigation helpers without a DOM", () => {
  for (const files of [ORDER, [...ORDER].reverse()]) {
    const BR = load(files);
    for (const fn of ["keySheet", "hasNotes", "nextFile"]) assert.equal(typeof BR.pure[fn], "function", fn);
  }
});

// --- The ? sheet ---------------------------------------------------------------------------------

/** Rows the sheet must hold with the default keys, keys then words (the same list as harness.ts `SHEET_WORDS`). */
const SHEET_WORDS = [
  ["y", "keep as is / keep / ok"],
  ["x", "no: Claude cuts it"],
  ["l", "later: Claude leaves it"],
  [".", "your call: Claude decides"],
  ["⏎", "open / follow"],
  ["Esc", "close"],
  ["Home End", "top / bottom"],
];
const SHEET_VERDICT_NOTE = "a verdict is a one-word note";

/** Every row of the sheet as `{keys, note}`, its group left out. */
const rowsOf = (sheet) => plain(sheet).flatMap((g) => g.rows.map((r) => ({ keys: r.keys, note: r.note ?? "" })));

test("the key sheet gives the default keys their words", () => {
  const BR = load();
  const rows = rowsOf(BR.pure.keySheet(BINDINGS, true));
  for (const [keys, note] of SHEET_WORDS) {
    assert.ok(
      rows.some((r) => r.keys.join(" ") === keys && r.note === note),
      `the sheet has the row "${keys} ${note}" in ${JSON.stringify(rows)}`,
    );
  }
});

test("every row of the key sheet has words, and the sheet says a verdict is a one-word note", () => {
  const BR = load();
  const sheet = BR.pure.keySheet(BINDINGS, true);
  for (const r of rowsOf(sheet)) assert.ok(r.note.trim().length > 0, `a row with keys only: ${JSON.stringify(r)}`);
  assert.ok(JSON.stringify(plain(sheet)).includes(SHEET_VERDICT_NOTE), JSON.stringify(plain(sheet)));
});

test("off a Mac the turn key reads Ctrl⏎", () => {
  const BR = load();
  const rows = rowsOf(BR.pure.keySheet(BINDINGS, false));
  assert.ok(rows.some((r) => r.keys.length === 1 && r.keys[0] === "Ctrl⏎"), JSON.stringify(rows));
  assert.ok(!rows.some((r) => r.keys.includes("⌘⏎")), JSON.stringify(rows));
});

test("the sheet names the keys as bound, and leaves out an unbound one", () => {
  const BR = load();
  const rows = rowsOf(BR.pure.keySheet({ ...BINDINGS, yes: "g", hide_marks: "", next_file: "Shift+L" }, true));
  assert.ok(rows.some((r) => r.keys.join(" ") === "g" && r.note === "keep as is / keep / ok"), JSON.stringify(rows));
  assert.ok(!rows.some((r) => r.keys.includes("y")), JSON.stringify(rows));
  assert.ok(rows.some((r) => r.keys.includes("⇧L")), JSON.stringify(rows));
  assert.ok(!rows.some((r) => r.keys.includes("m")), JSON.stringify(rows));
});

test("the sheet leaves its bindings alone", () => {
  const BR = load();
  const before = JSON.stringify(BINDINGS);
  BR.pure.keySheet(BINDINGS, true);
  assert.equal(JSON.stringify(BINDINGS), before);
});

// --- j, k, Home and End at the ends --------------------------------------------------------------

/** A keydown as the browser reports it, for `keyAction`. */
const key = (k, mods = {}) => ({ key: k, shiftKey: false, ctrlKey: false, metaKey: false, altKey: false, repeat: false, ...mods });

test("Home and End are the first- and last-stop keys, and a modifier leaves them to the browser", () => {
  const BR = load();
  assert.equal(BR.pure.keyAction(key("Home"), BINDINGS), "focus_first");
  assert.equal(BR.pure.keyAction(key("End"), BINDINGS), "focus_last");
  // Held, they go on moving, as j and k do
  assert.equal(BR.pure.keyAction(key("End", { repeat: true }), BINDINGS), "focus_last");
  for (const mods of [{ shiftKey: true }, { ctrlKey: true }, { metaKey: true }, { altKey: true }]) {
    assert.equal(BR.pure.keyAction(key("Home", mods), BINDINGS), null, JSON.stringify(mods));
    assert.equal(BR.pure.keyAction(key("End", mods), BINDINGS), null, JSON.stringify(mods));
  }
  // Rebound, the new key moves and the old one is the browser's again
  const moved = { ...BINDINGS, focus_first: "g", focus_last: "Shift+G" };
  assert.equal(BR.pure.keyAction(key("g"), moved), "focus_first");
  assert.equal(BR.pure.keyAction(key("G", { shiftKey: true }), moved), "focus_last");
  assert.equal(BR.pure.keyAction(key("Home"), moved), null);
});

test("j and k step one stop, and at the ends stay put and say which end", () => {
  const BR = load();
  const step = (at, count, by) => plain(BR.pure.focusStep(at, count, by));
  assert.deepEqual(step(0, 5, 1), { to: 1, says: null });
  assert.deepEqual(step(3, 5, 1), { to: 4, says: null });
  assert.deepEqual(step(4, 5, -1), { to: 3, says: null });
  assert.deepEqual(step(1, 5, -1), { to: 0, says: null });
  // Nothing wraps
  assert.deepEqual(step(4, 5, 1), { to: 4, says: "end of the doc" });
  assert.deepEqual(step(0, 5, -1), { to: 0, says: "top of the doc" });
  // One stop is both ends
  assert.deepEqual(step(0, 1, 1), { to: 0, says: "end of the doc" });
  assert.deepEqual(step(0, 1, -1), { to: 0, says: "top of the doc" });
});

test("Home and End go to the first and the last stop, and say nothing, even there already", () => {
  const BR = load();
  const step = (at, count, by) => plain(BR.pure.focusStep(at, count, by));
  assert.deepEqual(step(3, 5, "first"), { to: 0, says: null });
  assert.deepEqual(step(1, 5, "last"), { to: 4, says: null });
  assert.deepEqual(step(0, 5, "first"), { to: 0, says: null });
  assert.deepEqual(step(4, 5, "last"), { to: 4, says: null });
});

// --- The menu's keys row -------------------------------------------------------------------------

test("every writable doc's menu ends with a keys row, after the other files, and only withdraw after it", () => {
  const BR = load();
  const dir = dirState([listed("pr.md", summary("drafts")), listed("b.md", summary("replied"))], { drafts: 3 });
  for (const name of ["fresh", "drafts", "sent", "picked", "replied", "approved", "hold"]) {
    const s = state(name);
    const all = plain(BR.pure.menuRows(s, { now: s.server_now, dir, path: "pr.md", folder: true }));
    const at = all.findIndex((r) => r.id === "withdraw");
    assert.ok(at < 0 || at === all.length - 1, `${name}: withdraw is last`);
    const rows = at < 0 ? all : all.slice(0, -1);
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
  // Nothing wraps: past the last file with notes, or before the first, there is nowhere to go
  assert.equal(shift("sent.md", 1), null);
  assert.equal(shift("news.md", -1), null);
  // n and p cross only to news, as before
  assert.equal(BR.pure.nextFile(dir, "pr.md", 1), null);
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
  // Approve never carries a count; the guard does the counting
  const claimed = bent("replied", (s) => (s.owner.drafted_by = null));
  assert.deepEqual(btn(claimed), B("Approve"));
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

test("in both modes, a hand-back unclaimed for 10 s turns the button into the hand-off line; the phrase stays", () => {
  const BR = load();
  const mine = bent("sent", (x) => (x.owner.drafted_by = null));
  const sentAt = mine.phrase.sent.ts;
  const others = dirState([listed("b.md", null, true)]);
  const COPY_LINE = B("Copy hand-off line");
  // A folder with no drafter, a folder with one, and one file served, with a drafter or not
  const modes = [
    ["a folder doc of yours", mine, { dir: dirState([]), path: "pr.md", folder: true }],
    ["a folder doc Claude drafted", state("sent"), { dir: dirState([]), path: "pr.md", folder: true }],
    ["one file Claude drafted", state("sent"), { dir: null, path: "pr.md", folder: false }],
    ["one file of yours", mine, { dir: null, path: "pr.md", folder: false }],
  ];
  for (const [name, s, o] of modes) {
    const btn = (now) => plain(BR.pure.turnButton(s, { ...o, now }));
    const phrase = (now, extra = {}) => BR.pure.formatPhrase(s, { ...o, now, ...extra });
    assert.match(btn(sentAt + 9_999).text, /^With /, name);
    assert.equal(btn(sentAt + 9_999).disabled, true, name);
    assert.deepEqual(btn(sentAt + 10_000), COPY_LINE, name);
    assert.deepEqual(btn(sentAt + 60_000), COPY_LINE, name);
    for (const now of [sentAt + 9_999, sentAt + 10_000, sentAt + 60_000]) assert.equal(phrase(now), "sent 14:13 · not picked up yet", name);
    if (o.folder) assert.equal(phrase(sentAt + 10_000, { dir: others }), "sent 14:13 · not picked up yet · 1 other file ▾", name);
  }
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

test("tookPlace follows a held page's line through every write of the turn", () => {
  const BR = load();
  // The probe fold's turn: rewrite lines 3-4, then a line above, then cut line 10
  const changes = [
    { id: "7.0", op: 7, old: [3, 4], new: [3, 3] },
    { id: "8.0", op: 8, old: [3, 2], new: [3, 4] },
    { id: "9.0", op: 9, old: [10, 10], new: [10, 9] },
  ];
  const took = (ops, line) => BR.pure.tookPlace(changes, ops, line)?.id ?? null;
  // The first write took the held text's line 4, though the last write is what a live page sees
  assert.equal(took([7, 8, 9], 4), "7.0");
  assert.equal(took([9], 4), null);
  // Held line 9: -1 after the rewrite, +2 after the line above, so the last write's line 10
  assert.equal(took([7, 8, 9], 9), "9.0");
  assert.equal(took([7, 8, 9], 8), null);
  // A line no write touched
  assert.equal(took([7, 8, 9], 1), null);
});
