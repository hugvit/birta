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
const SHARED_BR = "var BR = BR && BR.__birta === true ? BR : { __birta: true };";
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

/** A doc's `dir_state` summary, exactly as `review::summarize` builds it from the fold. */
function summary(name, edit) {
  const s = bent(name, edit ?? (() => {}));
  return { stage: s.phrase.stage, phrase: s.phrase, counts: s.counts };
}

function listed(paths, sum = null, news = false) {
  return { paths: [].concat(paths), summary: sum, news };
}

/** A `dir_state` frame; totals are given, as the server counts them. */
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
const COPY = "copy hand-off line";
const MINE = "drafted by Claude · mark as yours";
const FOLDER_ROW = "note on this folder";

test("70-dir.js starts by reusing the shared BR namespace", () => {
  assert.equal(fs.readFileSync(path.join(ASSETS, "70-dir.js"), "utf-8").split("\n")[0], SHARED_BR);
});

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the folder code loads without a DOM, ${name}`, () => {
    const BR = load(files);
    for (const fn of ["otherFiles", "fileNote", "nextFile", "formatPhrase", "turnButton", "menuRows"])
      assert.equal(typeof BR.pure[fn], "function", fn);
    assert.equal(typeof BR.boot, "function");
  });
}

// --- Other files ---------------------------------------------------------------------------------

test("other files are the listed files this page is not, in the listing's order", () => {
  const BR = load();
  const dir = dirState([listed("b.md"), listed(["notes/plan.md", "plan.md"]), listed("sub/c.md", null, true)]);
  const paths = (p) => plain(BR.pure.otherFiles(dir, p)).map((f) => f.paths[0]);
  assert.deepEqual(paths("pr.md"), ["b.md", "notes/plan.md", "sub/c.md"]);
  assert.deepEqual(paths("b.md"), ["notes/plan.md", "sub/c.md"]);
  // A page open under either spelling of a linked doc is that doc
  assert.deepEqual(paths("plan.md"), ["b.md", "sub/c.md"]);
  assert.deepEqual(paths("notes/plan.md"), ["b.md", "sub/c.md"]);
  assert.deepEqual(plain(BR.pure.otherFiles(null, "pr.md")), []);
});

// --- The phrase ----------------------------------------------------------------------------------

const TWO_OTHERS = dirState(
  [listed("pr.md", summary("drafts")), listed("b.md", summary("replied")), listed("sub/c.md", null, true)],
  { drafts: 3, unseen_files: 0 },
);

test("in a folder the phrase names how many other files have activity", () => {
  const BR = load();
  const s = state("drafts");
  const phrase = (dir, p = "pr.md") => BR.pure.formatPhrase(s, { now: s.server_now, dir, path: p });
  assert.equal(phrase(TWO_OTHERS), "1 of 3 decided · 2 other files ▾");
  assert.equal(phrase(dirState([listed("pr.md"), listed("b.md")])), "1 of 3 decided · 1 other file ▾");
  // Only this doc, or nothing, listed: no suffix
  assert.equal(phrase(dirState([listed("pr.md", summary("drafts"))])), "1 of 3 decided");
  assert.equal(phrase(dirState([])), "1 of 3 decided");
  assert.equal(phrase(null), "1 of 3 decided");
});

test("the folder's own review file is not one of the other files", () => {
  const BR = load();
  const s = state("fresh");
  const dir = dirState([listed("pr.md")], {}, { summary: summary("drafts") });
  assert.equal(BR.pure.formatPhrase(s, { now: s.server_now, dir, path: "pr.md" }), "drafted by Claude · 3 items · your turn");
});

test("the suffix follows every phrase but the one for a folder birta can't write", () => {
  const BR = load();
  const at = (s) => BR.pure.formatPhrase(s, { now: s.server_now, dir: TWO_OTHERS, path: "pr.md" });
  assert.equal(at(state("sent")), "sent 14:13 · not picked up yet · 2 other files ▾");
  assert.equal(at(state("replied")), "Claude replied 14:13 · 1 done · 1 answered · 1 not handled · 2 other files ▾");
  assert.equal(at(bent("drafts", (s) => (s.writable = false))), "read-only · notes only · 2 other files ▾");
  assert.equal(at(bent("drafts", (s) => (s.log_writable = false))), "review unavailable · folder not writable");
});

test("inside a hold the phrase from before it keeps the suffix", () => {
  const BR = load();
  const s = state("hold");
  const inside = BR.pure.formatPhrase(s, { now: s.hold.not_before - 1, dir: TWO_OTHERS, path: "pr.md" });
  assert.equal(inside, "1 of 3 decided · 2 other files ▾");
});

// --- The turn button -----------------------------------------------------------------------------

test("in a folder the button hands back every file's drafts", () => {
  const BR = load();
  const btn = (s, dir) => plain(BR.pure.turnButton(s, { now: s.server_now, dir, path: "pr.md" }));
  const mine = bent("drafts", (s) => (s.counts.drafts = 1));
  assert.deepEqual(btn(mine, dirState([listed("b.md", summary("drafts"))], { drafts: 4 })), B("Hand back · 4"));
  // The carry-forward label: unseen files elsewhere drop the key hint
  assert.deepEqual(
    btn(mine, dirState([listed("b.md", summary("drafts"))], { drafts: 4, unseen_files: 2 })),
    B("Hand back · 4 · 2 files unseen", false),
  );
  assert.deepEqual(
    btn(mine, dirState([listed("b.md", summary("drafts"))], { drafts: 4, unseen_files: 1 })),
    B("Hand back · 4 · 1 file unseen", false),
  );
});

test("drafts in other files make the button a hand-back even with none here", () => {
  const BR = load();
  const btn = (s, dir) => plain(BR.pure.turnButton(s, { now: s.server_now, dir, path: "pr.md" }));
  const others = dirState([listed("b.md", summary("drafts"))], { drafts: 3 });
  // Approving this doc while other notes wait would be the wrong letter
  assert.deepEqual(btn(state("fresh"), others), B("Hand back · 3"));
  assert.deepEqual(btn(state("replied"), others), B("Hand back · 3"));
  // With no drafts anywhere, this doc's own turn speaks
  assert.deepEqual(btn(state("replied"), dirState([listed("b.md", summary("sent"))])), B("Approve · 2 unseen", false));
  assert.deepEqual(btn(state("fresh"), dirState([listed("b.md", summary("sent"))])), B("Approve"));
});

test("a listing that lags this doc's own drafts never counts fewer than this doc has", () => {
  const BR = load();
  const s = state("drafts");
  // The folder listing is throttled; the doc's own state arrives first
  const stale = dirState([listed("pr.md", summary("drafts", (x) => (x.counts.drafts = 1)))], { drafts: 1 });
  assert.deepEqual(plain(BR.pure.turnButton(s, { now: s.server_now, dir: stale, path: "pr.md" })), B("Hand back · 3"));
});

// --- Menu rows -----------------------------------------------------------------------------------

test("in a folder the menu adds the folder note and one row per other file, after its own rows", () => {
  const BR = load();
  const s = state("drafts");
  const rows = plain(BR.pure.menuRows(s, { now: s.server_now, dir: TWO_OTHERS, path: "pr.md", folder: true }));
  assert.deepEqual(
    rows.map((r) => r.text),
    [COPY, MINE, FOLDER_ROW, "b.md", "sub/c.md"],
  );
  const [folder, b, c] = rows.slice(2);
  assert.equal(folder.id, "folder");
  assert.equal(b.id, "file");
  assert.equal(b.href, "/view/b.md");
  assert.equal(b.note, "Claude replied 14:13 · 1 done · 1 answered · 1 not handled");
  assert.equal(c.href, "/view/sub/c.md");
  assert.equal(c.note, "changed");
  // Rows a page can't act on carry no op
  assert.equal(b.op, undefined);
  assert.equal(folder.op, undefined);
});

test("the folder row shows the folder's own summary", () => {
  const BR = load();
  const s = state("fresh");
  const dir = dirState([listed("b.md")], {}, { summary: summary("drafts", (x) => (x.counts.decided = 0)) });
  const folder = plain(BR.pure.menuRows(s, { now: s.server_now, dir, path: "pr.md", folder: true })).find((r) => r.id === "folder");
  assert.equal(folder.text, FOLDER_ROW);
  assert.equal(folder.note, "your turn · 3 notes");
  const none = plain(BR.pure.menuRows(s, { now: s.server_now, dir: dirState([listed("b.md")]), path: "pr.md", folder: true }));
  assert.equal(none.find((r) => r.id === "folder").note, "");
});

test("serving one file the menu has no folder row, but keeps the other files' rows", () => {
  const BR = load();
  const s = state("drafts");
  const rows = plain(BR.pure.menuRows(s, { now: s.server_now, dir: TWO_OTHERS, path: "pr.md", folder: false }));
  assert.deepEqual(rows.map((r) => r.text), [COPY, MINE, "b.md", "sub/c.md"]);
});

test("drafts elsewhere make a hand-back on a sent or approved doc, but never where review is off", () => {
  const BR = load();
  const btn = (s, dir) => plain(BR.pure.turnButton(s, { now: s.server_now, dir, path: "pr.md" }));
  const others = dirState([listed("b.md", summary("drafts"))], { drafts: 3 });
  assert.deepEqual(btn(state("sent"), others), B("Hand back · 3"));
  assert.deepEqual(btn(state("approved"), others), B("Hand back · 3"));
  // Review keys do nothing in a folder birta can't write
  const off = btn(bent("drafts", (s) => (s.log_writable = false)), others);
  assert.doesNotMatch(off?.text ?? "", /^Hand back/);
});

test("a file row links its first path, encoded, and never lists this page", () => {
  const BR = load();
  const s = state("fresh");
  const dir = dirState([listed(["notes/plan.md", "plan.md"]), listed("my notes/ä #1.md", null, true)]);
  const rows = plain(BR.pure.menuRows(s, { now: s.server_now, dir, path: "plan.md" })).filter((r) => r.id === "file");
  assert.deepEqual(rows.map((r) => r.text), ["my notes/ä #1.md"]);
  assert.equal(rows[0].href, "/view/my%20notes/%C3%A4%20%231.md");
});

test("with no folder listing the menu is unchanged", () => {
  const BR = load();
  for (const name of ["fresh", "drafts", "sent", "replied", "approved"]) {
    const s = state(name);
    const without = plain(BR.pure.menuRows(s, { now: s.server_now, dir: null, path: "pr.md" }));
    assert.ok(!without.some((r) => r.id === "folder" || r.id === "file"), name);
  }
});

test("a folder birta can't write has no menu rows at all", () => {
  const BR = load();
  const s = bent("drafts", (x) => (x.log_writable = false));
  assert.deepEqual(plain(BR.pure.menuRows(s, { now: s.server_now, dir: TWO_OTHERS, path: "pr.md" })), []);
});

// --- A file row's note ---------------------------------------------------------------------------

test("a file row's note is its summary's phrase", () => {
  const BR = load();
  const note = (sum, news = false) =>
    BR.pure.fileNote(listed("b.md", sum, news), { now: state("drafts").server_now });
  assert.equal(note(summary("drafts")), "1 of 3 decided");
  assert.equal(note(summary("drafts", (x) => (x.counts.decided = 0))), "your turn · 3 notes");
  assert.equal(note(summary("sent")), "sent 14:13 · not picked up yet");
  assert.equal(note(summary("picked")), "Claude picked up 14:13");
  assert.equal(note(summary("replied")), "Claude replied 14:13 · 1 done · 1 answered · 1 not handled");
  // The summary names no drafter
  assert.equal(note(summary("fresh")), "3 items · your turn");
  // The summary carries no approve time
  assert.equal(note(summary("approved")), "approved");
  assert.equal(note(null, true), "changed");
  assert.equal(note(null, false), "");
});

test("a file row's note never leaks a value as text", () => {
  const BR = load();
  for (const name of ["fresh", "drafts", "sent", "picked", "replied", "unseen", "approved", "hold", "lost", "rewrite"]) {
    for (const news of [false, true]) {
      const t = BR.pure.fileNote(listed("b.md", summary(name), news), { now: state(name).server_now });
      assert.doesNotMatch(t, /undefined|null|NaN|\[object/, `${name}: ${JSON.stringify(t)}`);
      assert.doesNotMatch(t, /\s{2}|^\s|\s$|·\s*·/, `${name}: ${JSON.stringify(t)}`);
    }
  }
});

// --- Shift+J and Shift+K -------------------------------------------------------------------------

test("the next file is the next one with news after this one, round the end", () => {
  const BR = load();
  const dir = dirState([listed("a.md", null, true), listed("b.md", null, true), listed("c.md")]);
  const next = (p, step) => BR.pure.nextFile(dir, p, step);
  assert.equal(next("c.md", 1), "a.md");
  assert.equal(next("c.md", -1), "b.md");
  assert.equal(next("a.md", 1), "b.md");
  assert.equal(next("b.md", 1), "a.md");
  assert.equal(next("a.md", -1), "b.md");
  // A page the listing doesn't hold starts from the top, or the bottom going back
  assert.equal(next("zzz.md", 1), "a.md");
  assert.equal(next("zzz.md", -1), "b.md");
});

test("with no other file with news there is nowhere to go", () => {
  const BR = load();
  assert.equal(BR.pure.nextFile(dirState([listed("a.md", null, true), listed("b.md")]), "a.md", 1), null);
  assert.equal(BR.pure.nextFile(dirState([listed("a.md", null, true), listed("b.md")]), "a.md", -1), null);
  assert.equal(BR.pure.nextFile(dirState([]), "a.md", 1), null);
  assert.equal(BR.pure.nextFile(null, "a.md", 1), null);
});

test("the next file goes by the path a link uses and skips this doc's other spelling", () => {
  const BR = load();
  const dir = dirState([listed(["notes/plan.md", "plan.md"], null, true), listed(["b.md"], null, true)]);
  assert.equal(BR.pure.nextFile(dir, "plan.md", 1), "b.md");
  assert.equal(BR.pure.nextFile(dir, "b.md", 1), "notes/plan.md");
});

test("the folder code leaves its inputs alone", () => {
  const BR = load();
  const s = state("drafts");
  const before = JSON.stringify([s, TWO_OTHERS]);
  BR.pure.formatPhrase(s, { now: s.server_now, dir: TWO_OTHERS, path: "pr.md" });
  BR.pure.turnButton(s, { now: s.server_now, dir: TWO_OTHERS, path: "pr.md" });
  BR.pure.menuRows(s, { now: s.server_now, dir: TWO_OTHERS, path: "pr.md" });
  BR.pure.otherFiles(TWO_OTHERS, "pr.md");
  BR.pure.nextFile(TWO_OTHERS, "pr.md", 1);
  for (const f of TWO_OTHERS.files) BR.pure.fileNote(f, { now: s.server_now });
  assert.equal(JSON.stringify([s, TWO_OTHERS]), before);
});
