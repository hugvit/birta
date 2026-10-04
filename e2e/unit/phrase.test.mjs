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

/** A copy of fixture `name` with `edit` applied. */
function bent(name, edit) {
  const s = state(name);
  edit(s);
  return s;
}

/** Epoch ms of hh:mm UTC on the fixtures' day, so each fact can carry its own time. */
const at = (hh, mm) => Date.UTC(2026, 8, 21, hh, mm, 7);
const MIN = 60_000;

// Results come from the vm context, whose objects have their own prototypes
const plain = (v) => (v == null ? v : JSON.parse(JSON.stringify(v)));

/** Everything the chrome says for `s` at server time `now`; the menu without the key sheet's row. */
function chrome(BR, s, now = s.server_now, dir = null) {
  const menu = plain(BR.pure.menuRows(s, { now, dir })).map((r) => r.text);
  // Every menu review can act in ends with the key sheet's row, then withdraw where there is one
  const withdraw = menu[menu.length - 1] === "withdraw" ? menu.pop() : null;
  if (s.log_writable !== false) assert.equal(menu.pop(), "keys");
  if (withdraw) menu.push(withdraw);
  return {
    phrase: BR.pure.formatPhrase(s, { now, dir }),
    button: plain(BR.pure.turnButton(s, { now, dir })),
    pill: BR.pure.holdPill(s, { now }),
    menu,
  };
}

test("60-chrome.js starts by reusing the shared BR namespace", () => {
  assert.equal(fs.readFileSync(path.join(ASSETS, "60-chrome.js"), "utf-8").split("\n")[0], SHARED_BR);
});

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the chrome loads without a DOM, ${name}`, () => {
    const BR = load(files);
    for (const fn of ["formatPhrase", "turnButton", "holdPill", "menuRows", "nackText", "laneMarks", "keyAction"])
      assert.equal(typeof BR.pure[fn], "function", fn);
    assert.equal(typeof BR.boot, "function");
  });
}

// --- Every committed state, at its own server time -------------------------------------------

const B = (text, kbd = true, disabled = false) => ({ text, kbd, disabled });
const COPY = "copy hand-off line";
const MINE = "drafted by Claude · mark as yours";
// The hand-off line takes the button once a hand-back sits unclaimed this long
const CUE_MS = 10_000;
/** A time inside the 10 s before a sent state's button turns into the hand-off line. */
const soon = (s) => s.phrase.sent.ts + CUE_MS - 1;

const FIXTURE_CHROME = {
  fresh: {
    phrase: "drafted by Claude · your turn",
    button: B("Approve"),
    pill: null,
    menu: [COPY, MINE],
  },
  // The phrase never counts your notes; the button does
  drafts: { phrase: "drafted by Claude · your turn", button: B("Hand back · 3"), pill: null, menu: [COPY, MINE] },
  // The doc can't be written, so notes are all it takes
  dir: { phrase: "read-only · notes only", button: B("Hand back · 2"), pill: null, menu: [COPY] },
  // Writing again after a reply keeps the reply and what is left to check; the button counts the note
  flow: {
    phrase: "Claude replied 14:13 · 9 to check",
    button: B("Hand back · 1"),
    pill: null,
    menu: [COPY, MINE],
  },
  // Inside the hold nothing has left yet: the phrase and button keep what they said before
  hold: {
    phrase: "drafted by Claude · your turn",
    button: B("Hand back · 3"),
    pill: "handing back · 3 notes · z to undo",
    menu: [COPY, MINE],
  },
  // Its own server time is minutes after the hand-back, which nothing picked up
  sent: {
    phrase: "sent 14:13 · not picked up yet",
    button: B("Copy hand-off line"),
    pill: null,
    menu: [COPY, MINE, "withdraw"],
  },
  picked: { phrase: "Claude picked up 14:13", button: B("With Claude", false, true), pill: null, menu: [COPY, MINE] },
  replied: {
    phrase: "Claude replied 14:13 · 3 to check",
    button: B("Approve"),
    pill: null,
    menu: [COPY, MINE],
  },
  unseen: {
    phrase: "Claude replied 14:13 · 5 to check",
    button: B("Approve"),
    pill: null,
    menu: [COPY, MINE],
  },
  rewrite: {
    phrase: "Claude replied 14:13 · 9 to check",
    button: B("Approve"),
    pill: null,
    menu: [COPY, MINE],
  },
  lost: {
    phrase: "Claude replied 14:13 · 3 to check",
    button: B("Approve"),
    pill: null,
    menu: [COPY, MINE],
  },
  // The approve's hold has passed and nothing claimed it, so it can still be taken back
  approved: {
    phrase: "approved 14:13 · c to reopen",
    button: B("Approved ✓", false, true),
    pill: null,
    menu: [COPY, MINE, "withdraw"],
  },
};

test("the table covers every committed state", () => {
  const names = fs
    .readdirSync(FIXTURES)
    .filter((n) => n.endsWith(".json"))
    .map((n) => n.slice(0, -5))
    .sort();
  assert.deepEqual(names, Object.keys(FIXTURE_CHROME).sort());
});

for (const [name, want] of Object.entries(FIXTURE_CHROME)) {
  test(`chrome for ${name}.json`, () => {
    assert.deepEqual(chrome(load(), state(name)), want);
  });
}

// --- The phase table, row by row, from states ----------------------------------------------------

/** Epoch ms of hh:mm UTC, the table's own times. */
const T = (hh, mm) => Date.UTC(2026, 8, 21, hh, mm, 7);

/** `name` with its hand-back at 16:56. */
function sentAt1656(name, edit) {
  return bent(name, (s) => {
    s.phrase.sent.ts = T(16, 56);
    if (s.hold && s.hold.kind === "hand_back") s.hold.not_before = T(16, 56);
    s.server_now = T(16, 56) + 5_000;
    edit?.(s);
  });
}

/** `name` picked up at 16:56, and quiet since then. */
function pickedAt1656(edit) {
  return bent("picked", (s) => {
    s.phrase.sent.ts = T(16, 55);
    s.phrase.picked_up.ts = T(16, 56);
    s.phrase.picked_up.silence.since_ts = T(16, 56);
    s.server_now = T(16, 57);
    edit?.(s);
  });
}

/** replied.json with its reply at 16:57, `left` stops to check and `drafts` notes written since. */
function repliedAt1657(left, drafts = 0) {
  return bent("replied", (s) => {
    s.phrase.replied.ts = T(16, 57);
    s.counts.to_check = left;
    s.counts.attention = left;
    s.counts.drafts = drafts;
  });
}

/** approved.json approved at 17:03, past its hold. */
function approvedAt1703(edit) {
  return bent("approved", (s) => {
    s.approve.ts = T(17, 3);
    s.hold.not_before = T(17, 3);
    s.server_now = T(17, 10);
    edit?.(s);
  });
}

const PHASES = [
  ["Claude's draft, before you write", state("fresh"), "drafted by Claude · your turn", B("Approve")],
  ["Claude's draft, once you've written", state("drafts"), "drafted by Claude · your turn", B("Hand back · 3")],
  ["your own doc, drafting", bent("drafts", (s) => (s.owner.drafted_by = null)), "your turn", B("Hand back · 3")],
  ["sent, not picked up", sentAt1656("sent"), "sent 16:56 · not picked up yet", B("With Claude", false, true)],
  ["picked up", pickedAt1656(), "Claude picked up 16:56", B("With Claude", false, true)],
  [
    "picked up, you wrote more",
    pickedAt1656((s) => (s.counts.drafts = 1)),
    "Claude picked up 16:56 · 1 note for next hand-back",
    B("With Claude", false, true),
  ],
  [
    "picked up, quiet a long time",
    pickedAt1656((s) => {
      s.phrase.picked_up.silence.threshold_ms = 10 * MIN;
      s.server_now = T(16, 56) + 12 * MIN;
    }),
    "Claude picked up 16:56 · no reply in 12 min",
    B("With Claude", false, true),
  ],
  ["replied, things left", repliedAt1657(9), "Claude replied 16:57 · 9 to check", B("Approve")],
  ["replied, things left, once you've written", repliedAt1657(9, 1), "Claude replied 16:57 · 9 to check", B("Hand back · 1")],
  ["replied, nothing left", repliedAt1657(0), "Claude replied 16:57 · all checked", B("Approve")],
  ["replied, nothing left, once you've written", repliedAt1657(0, 1), "Claude replied 16:57 · all checked", B("Hand back · 1")],
  ["approved", approvedAt1703(), "approved 17:03 · c to reopen", B("Approved ✓", false, true)],
  ["reopened after approving", approvedAt1703((s) => (s.counts.drafts = 1)), "approved 17:03 · reopened", B("Hand back · 1")],
  [
    "approved, text changed since",
    approvedAt1703((s) => {
      s.approve.changed_since = true;
      s.approve.current_matches = false;
    }),
    "approved 17:03 · changed since",
    B("Approve"),
  ],
];

for (const [row, s, phrase, button] of PHASES) {
  test(`phase table: ${row} reads "${phrase}" beside "${button.text}"`, () => {
    const got = chrome(load(), s);
    assert.equal(got.phrase, phrase);
    assert.deepEqual(got.button, button);
  });
}

test("phase table: a hand-back nothing picks up in 10 s turns the button into the hand-off line, in both modes", () => {
  const BR = load([...ORDER, "70-dir.js"]);
  const dir = { type: "dir_state", root: ".", truncated: false, folder: null, files: [], totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0 } };
  const modes = [
    ["one file, Claude's draft", state("sent"), {}],
    ["one file, your doc", bent("sent", (s) => (s.owner.drafted_by = null)), {}],
    ["a folder, Claude's draft", state("sent"), { dir, path: "pr.md", folder: true }],
    ["a folder, your doc", bent("sent", (s) => (s.owner.drafted_by = null)), { dir, path: "pr.md", folder: true }],
  ];
  for (const [mode, s, extra] of modes) {
    const at = (now) => ({
      phrase: BR.pure.formatPhrase(s, { now, ...extra }),
      button: plain(BR.pure.turnButton(s, { now, ...extra })),
    });
    const who = s.owner.drafted_by ? "With Claude" : "With the agent";
    assert.deepEqual(at(s.phrase.sent.ts + CUE_MS - 1), { phrase: "sent 14:13 · not picked up yet", button: B(who, false, true) }, mode);
    assert.deepEqual(at(s.phrase.sent.ts + CUE_MS), { phrase: "sent 14:13 · not picked up yet", button: B("Copy hand-off line") }, mode);
    assert.deepEqual(at(s.phrase.sent.ts + 60 * MIN), { phrase: "sent 14:13 · not picked up yet", button: B("Copy hand-off line") }, mode);
  }
});

test("phase table: with no hand-off line in the state the button stays With Claude", () => {
  const BR = load();
  const s = bent("sent", (x) => (x.hand_off = null));
  assert.deepEqual(plain(BR.pure.turnButton(s, { now: s.phrase.sent.ts + 60 * MIN })), B("With Claude", false, true));
});

test("phase table: once the agent picks up, the hand-off line is gone from the button", () => {
  const BR = load();
  const s = state("picked");
  assert.deepEqual(plain(BR.pure.turnButton(s, { now: s.phrase.sent.ts + 60 * MIN })), B("With Claude", false, true));
});

test("phase table: with birta stopped the phrase says so and the button keeps its words, disabled", () => {
  const BR = load();
  for (const name of ["fresh", "drafts", "replied", "picked", "approved"]) {
    const s = state(name);
    const was = plain(BR.pure.turnButton(s, { now: s.server_now }));
    assert.equal(BR.pure.formatPhrase(s, { now: s.server_now, stopped: true }), "birta stopped · notes wait here", name);
    const now = plain(BR.pure.turnButton(s, { now: s.server_now, stopped: true }));
    assert.equal(now.text, was.text, name);
    assert.equal(now.disabled, true, name);
  }
});

// --- The folder: one menu item after the phrase ----------------------------------------------

// The folder's own helpers, which the phrase reads in a folder
const WITH_DIR = [...ORDER, "70-dir.js"];

/** A dir_state listing pr.md and `others`, each `[path, stage]`. */
function listing(others) {
  const file = (p, stage) => {
    const s = state(stage === "sent" ? "sent" : stage === "picked_up" ? "picked" : stage === "replied" ? "replied" : "drafts");
    // Every file of the listing went back in this doc's ⌘⏎ batch, at 16:55
    if (s.phrase.sent) s.phrase.sent.ts = T(16, 55);
    return { paths: [p], summary: { stage, phrase: s.phrase, counts: s.counts, notes: 1 }, news: false };
  };
  return {
    type: "dir_state",
    root: ".",
    truncated: false,
    folder: null,
    files: [file("pr.md", "picked_up"), ...others.map(([p, stage]) => file(p, stage))],
    totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0 },
  };
}

test("folder: a file of this hand-back still unclaimed is the phrase's one menu item", () => {
  const BR = load(WITH_DIR);
  const s = pickedAt1656();
  const phrase = (others) => BR.pure.formatPhrase(s, { now: s.server_now, dir: listing(others), path: "pr.md", folder: true });
  assert.equal(phrase([["checklist.md", "sent"]]), "Claude picked up 16:56 · checklist.md not picked up ▾");
  // One item, however many files there are
  assert.equal(phrase([["notes.md", "drafts"], ["checklist.md", "sent"]]), "Claude picked up 16:56 · checklist.md not picked up ▾");
});

test("folder: with every file of the hand-back picked up, the item counts the other files", () => {
  const BR = load(WITH_DIR);
  const s = pickedAt1656();
  const phrase = (others) => BR.pure.formatPhrase(s, { now: s.server_now, dir: listing(others), path: "pr.md", folder: true });
  assert.equal(phrase([["checklist.md", "picked_up"]]), "Claude picked up 16:56 · 1 other file ▾");
  assert.equal(phrase([["checklist.md", "picked_up"], ["notes.md", "replied"]]), "Claude picked up 16:56 · 2 other files ▾");
  assert.equal(phrase([]), "Claude picked up 16:56");
});

// --- What the phrase never says ------------------------------------------------------------------

const GONE = [/\bunseen\b/, /blocks? decided/, /\bdecided\b/, /\b\d+ done\b/, /\b\d+ answered\b/, /\bfor you\b/, /without a note/, /copy hand-off line/, /· release$/];

test("the phrase holds one event and at most one count, and none of the tallies the rows carry now", () => {
  const BR = load();
  const states = [
    ...Object.keys(FIXTURE_CHROME).map((n) => [n, state(n)]),
    ...PHASES.map(([row, s]) => [row, s]),
    [
      "a reply with every tally",
      bent("replied", (s) => Object.assign(s.phrase.replied, { done: 3, answered: 1, asked: 2, unasked: 1, not_handled: 2, didnt: 1 })),
    ],
  ];
  for (const [name, s] of states) {
    for (const now of [s.server_now, s.hold ? s.hold.not_before - 1 : s.server_now, s.server_now + 60 * MIN]) {
      const p = BR.pure.formatPhrase(s, { now });
      for (const gone of GONE) assert.doesNotMatch(p, gone, `${name}: ${JSON.stringify(p)}`);
      const counts = p.match(/\b\d+ (to check|notes? for next hand-back)\b|\ball checked\b/g) ?? [];
      assert.ok(counts.length <= 1, `${name}: one count at most in ${JSON.stringify(p)}`);
      assert.doesNotMatch(p, /…$/, `${name}: never cut short`);
    }
  }
});

test("a long agent name is never cut short in the phrase", () => {
  const BR = load();
  const name = "an-agent-with-a-name-far-longer-than-any-header-has-room-for";
  const s = bent("replied", (x) => (x.phrase.picked_up.by = x.phrase.replied.by = x.rounds[0].claimed_by = name));
  assert.equal(BR.pure.formatPhrase(s, { now: s.server_now }), `${name.charAt(0).toUpperCase()}${name.slice(1)} replied 14:13 · 3 to check`);
});

// --- One count, on the button and the hand-back message only -------------------------------------

/** hold.json with your answer to Claude's question going back with the three notes. */
function holdWithAnswer(notes = 3) {
  return bent("hold", (s) => {
    s.hold.count = notes;
    if (!notes) s.items = s.items.filter((i) => !(i.by && i.by.type === "human"));
    s.items.push({
      n: 2,
      by: { type: "agent", name: "claude" },
      status: "answered",
      verdict: "ask",
      body: "which status codes?",
      at: { kind: "doc", loc: null, scope: null },
      replies: [{ n: 3, by: { type: "human", name: "" }, body: "502–504 too", seen: true }],
    });
  });
}

test("a hand-back of notes and an answer: the button counts both, and the message reads the same count", () => {
  const BR = load();
  const s = holdWithAnswer();
  const c = chrome(BR, s);
  assert.equal(c.pill, "handing back · 3 notes · 1 answer · z to undo");
  assert.deepEqual(c.button, B("Hand back · 4"));
});

test("an answer alone is handed back as an answer, and the button counts it", () => {
  const BR = load();
  const s = holdWithAnswer(0);
  const c = chrome(BR, s);
  assert.equal(c.pill, "handing back · 1 answer · z to undo");
  assert.deepEqual(c.button, B("Hand back · 1"));
});

test("inside any hand-back hold the button's count is the sum the message reads", () => {
  const BR = load();
  const sum = (pill) => [...pill.matchAll(/(\d+) (notes?|answers?)\b/g)].reduce((n, m) => n + Number(m[1]), 0);
  for (const [notes, answer] of [[1, false], [3, false], [3, true], [0, true]]) {
    const s = answer ? holdWithAnswer(notes) : bent("hold", (x) => (x.hold.count = notes));
    const c = chrome(BR, s);
    assert.equal(c.button.text, `Hand back · ${sum(c.pill)}`, `${notes} notes${answer ? " and an answer" : ""}: ${c.pill}`);
  }
});

// --- Crafted states: each pins one rule the fixtures can't tell apart --------------------------

const notesOnly = (s) => {
  for (const i of s.items) i.verdict = null;
  s.counts.decided = 0;
};

const CRAFTED = [
  // The phrase's state machine before any hand-back (design call 5)
  ["a fresh doc of yours says only whose turn", bent("fresh", (s) => (s.owner.drafted_by = null)), { phrase: "your turn" }],
  [
    "a fresh agent draft with no items says only whose turn",
    bent("fresh", (s) => (s.counts.items = 0)),
    { phrase: "drafted by Claude · your turn" },
  ],
  [
    "a fresh doc of yours with no items says only whose turn",
    bent("fresh", (s) => {
      s.owner.drafted_by = null;
      s.counts.items = 0;
    }),
    { phrase: "your turn" },
  ],
  [
    "notes are counted on the button, never in the phrase",
    bent("drafts", notesOnly),
    { phrase: "drafted by Claude · your turn", button: B("Hand back · 3") },
  ],
  [
    "one note is singular on the button",
    bent("drafts", (s) => {
      notesOnly(s);
      s.items = s.items.slice(0, 1);
      s.counts.drafts = 1;
    }),
    { phrase: "drafted by Claude · your turn", button: B("Hand back · 1") },
  ],
  [
    "a verdict counts as one note, whatever it is on",
    bent("drafts", (s) => {
      s.items = s.items.filter((i) => i.at.kind === "doc").map((i) => ({ ...i, verdict: "no" }));
      s.counts.drafts = 1;
      s.counts.decided = 0;
    }),
    { phrase: "drafted by Claude · your turn", button: B("Hand back · 1") },
  ],
  [
    "the drafter's name comes from the state",
    bent("fresh", (s) => (s.owner.drafted_by = "codex")),
    { phrase: "drafted by Codex · your turn", menu: [COPY, "drafted by Codex · mark as yours"] },
  ],

  // The hold, by the server's clock
  [
    "a millisecond before not_before the hold still speaks",
    state("hold"),
    { phrase: "drafted by Claude · your turn", button: B("Hand back · 3"), pill: "handing back · 3 notes · z to undo" },
    (s) => s.hold.not_before - 1,
  ],
  [
    "at not_before the hand-back has left",
    state("hold"),
    { phrase: "sent 14:13 · not picked up yet", button: B("With Claude", false, true), pill: null },
    (s) => s.hold.not_before,
  ],
  [
    "a held hand-back of notes counts the held notes, which the counts no longer hold",
    bent("hold", notesOnly),
    { phrase: "drafted by Claude · your turn", button: B("Hand back · 3"), pill: "handing back · 3 notes · z to undo" },
  ],
  [
    "a held hand-back of one note is singular",
    bent("hold", (s) => {
      notesOnly(s);
      s.hold.count = 1;
    }),
    { phrase: "drafted by Claude · your turn", button: B("Hand back · 1"), pill: "handing back · 1 note · z to undo" },
  ],
  [
    "a note kept during the hold joins the button's count and the message's",
    bent("hold", (s) => {
      notesOnly(s);
      s.counts.drafts = 1;
    }),
    { phrase: "drafted by Claude · your turn", button: B("Hand back · 4"), pill: "handing back · 4 notes · z to undo" },
  ],
  [
    "a held approve keeps the phrase from before it, and its button",
    state("approved"),
    {
      phrase: "Claude replied 14:13 · all checked",
      button: B("Approve"),
      pill: "approving · z to undo",
    },
    (s) => s.hold.not_before - 1_000,
  ],

  // Each fact carries its own time
  [
    "sent names the hand-back's time",
    bent("sent", (s) => (s.phrase.sent.ts = at(9, 5))),
    { phrase: "sent 09:05 · not picked up yet" },
  ],
  [
    "picked up names the pickup's time, not the hand-back's",
    bent("picked", (s) => {
      s.phrase.sent.ts = at(9, 5);
      s.phrase.picked_up.ts = at(10, 20);
      s.phrase.picked_up.silence.since_ts = at(10, 20);
      s.server_now = at(10, 25);
    }),
    { phrase: "Claude picked up 10:20" },
  ],
  [
    "replied names the reply's time, not the pickup's",
    bent("replied", (s) => {
      s.phrase.sent.ts = at(9, 5);
      s.phrase.picked_up.ts = at(10, 20);
      s.phrase.replied.ts = at(11, 45);
    }),
    { phrase: "Claude replied 11:45 · 3 to check" },
  ],
  [
    "approved names the approve's time",
    bent("approved", (s) => {
      s.approve.ts = at(16, 30);
      s.phrase.replied.ts = at(11, 45);
    }),
    { phrase: "approved 16:30 · c to reopen" },
  ],

  // The button names whoever holds the turn: the live claimer, else the drafter, else the agent
  [
    "before pickup the button names the drafter",
    state("sent"),
    { button: B("With Claude", false, true) },
    soon,
  ],
  [
    "before pickup a doc of yours names no one",
    bent("sent", (s) => (s.owner.drafted_by = null)),
    { button: B("With the agent", false, true), menu: [COPY, "withdraw"] },
    soon,
  ],

  // The agent's name comes from the state
  [
    "the agent that picked up is named over the drafter",
    bent("picked", (s) => {
      s.phrase.picked_up.by = "codex";
      s.rounds[0].claimed_by = "codex";
    }),
    { phrase: "Codex picked up 14:13", button: B("With Codex", false, true) },
  ],
  [
    "another agent replies under its own name",
    bent("replied", (s) => {
      s.phrase.picked_up.by = "codex";
      s.phrase.replied.by = "codex";
      s.rounds[0].claimed_by = "codex";
    }),
    { phrase: "Codex replied 14:13 · 3 to check" },
  ],

  // A long silence after pickup is stated at 15 minutes, by the server's clock; release lives in the menu
  [
    "a second before 15 minutes of silence nothing is said",
    state("picked"),
    { phrase: "Claude picked up 14:13", menu: [COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 15 * MIN - 1_000,
  ],
  [
    "at 15 minutes of silence the phrase says so and the menu can release",
    state("picked"),
    { phrase: "Claude picked up 14:13 · no reply in 15 min", menu: ["release", COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 15 * MIN,
  ],
  [
    "the silence counts whole minutes",
    state("picked"),
    { phrase: "Claude picked up 14:13 · no reply in 40 min" },
    (s) => s.phrase.picked_up.silence.since_ts + 40 * MIN + 30_000,
  ],

  // Writing while the agent holds the turn: the button can't count, so the phrase does
  [
    "with an item decided, what waits for the next hand-back is still counted as notes",
    bent("picked", (s) => (s.counts.drafts = 2)),
    { phrase: "Claude picked up 14:13 · 2 notes for next hand-back", button: B("With Claude", false, true) },
  ],
  [
    "with no item decided, what waits for the next hand-back is counted as notes",
    bent("picked", (s) => {
      notesOnly(s);
      s.counts.drafts = 2;
    }),
    { phrase: "Claude picked up 14:13 · 2 notes for next hand-back", button: B("With Claude", false, true) },
  ],
  [
    "one note waiting is singular",
    bent("picked", (s) => {
      notesOnly(s);
      s.counts.drafts = 1;
    }),
    { phrase: "Claude picked up 14:13 · 1 note for next hand-back", button: B("With Claude", false, true) },
  ],
  [
    "a note written before pickup waits too: the button stays With Claude",
    bent("sent", (s) => (s.counts.drafts = 1)),
    { button: B("With Claude", false, true) },
    soon,
  ],

  // The reply's tallies are on the rows now; the phrase counts only what is left to check
  [
    "a reply's own tallies stay out of the phrase, which counts the stops left to check",
    bent("replied", (s) => {
      Object.assign(s.phrase.replied, { done: 4, answered: 0, asked: 1, unasked: 0, not_handled: 1 });
      s.counts.to_check = 2;
    }),
    { phrase: "Claude replied 14:13 · 2 to check" },
  ],
  [
    "with every tally and nothing left to check, the reply says all checked",
    bent("replied", (s) => {
      Object.assign(s.phrase.replied, { done: 3, answered: 1, asked: 2, unasked: 1, not_handled: 2 });
      s.counts.to_check = 0;
    }),
    { phrase: "Claude replied 14:13 · all checked" },
  ],
  [
    "one stop left to check reads 1 to check",
    bent("replied", (s) => (s.counts.to_check = 1)),
    { phrase: "Claude replied 14:13 · 1 to check" },
  ],

  // Writing again after a reply
  [
    "notes after a reply keep the reply's phrase, and the button counts them",
    bent("replied", (s) => {
      s.counts.drafts = 1;
      s.counts.decided = 0;
    }),
    { phrase: "Claude replied 14:13 · 3 to check", button: B("Hand back · 1") },
  ],
  [
    "a verdict after a reply keeps the reply's phrase",
    bent("replied", (s) => (s.counts.drafts = 1)),
    { phrase: "Claude replied 14:13 · 3 to check", button: B("Hand back · 1") },
  ],

  // Approve
  [
    "an approval whose text changed says so, and can be approved again",
    bent("approved", (s) => {
      s.approve.changed_since = true;
      s.approve.current_matches = false;
      s.approve.later = 0;
    }),
    { phrase: "approved 14:13 · changed since", button: B("Approve") },
  ],
  [
    "an approval whose text changed hands back the notes written since",
    bent("approved", (s) => {
      s.approve.changed_since = true;
      s.approve.current_matches = false;
      s.counts.drafts = 2;
    }),
    { button: B("Hand back · 2") },
  ],
  [
    "a note after approving reopens it",
    bent("approved", (s) => (s.counts.drafts = 1)),
    { phrase: "approved 14:13 · reopened", button: B("Hand back · 1") },
  ],
  // What waits for later goes to the tooltip; the clean view's phrase stays bare
  ["an approval leaves what waits for later out of the phrase", bent("approved", (s) => (s.approve.later = 2)), { phrase: "approved 14:13 · c to reopen" }],
  [
    "open questions don't change the Approve button; the guard counts them",
    bent("replied", (s) => {
      s.counts.unseen = 0;
      s.counts.open_asks = 1;
      s.items.find((i) => i.n === 6).replies[0].seen = true;
    }),
    { button: B("Approve") },
  ],
  [
    "unchecked changes don't change the Approve button either",
    bent("replied", (s) => {
      s.counts.unseen = 2;
      s.counts.open_asks = 1;
    }),
    { button: B("Approve") },
  ],
  [
    "with nothing to check the button approves",
    bent("replied", (s) => {
      s.counts.unseen = 0;
      s.counts.to_check = 0;
      s.items.find((i) => i.n === 6).replies[0].seen = true;
    }),
    { button: B("Approve") },
  ],

  // What the folder and the file allow
  [
    "a doc that can't be written is read-only, notes only, and still approves and hands back",
    bent("fresh", (s) => (s.writable = false)),
    { phrase: "read-only · notes only", button: B("Approve"), menu: [COPY, MINE] },
  ],
  [
    "a read-only doc with notes hands them back",
    bent("drafts", (s) => (s.writable = false)),
    { phrase: "read-only · notes only", button: B("Hand back · 3"), menu: [COPY, MINE] },
  ],
  [
    "a folder that can't be written has no review, and no button",
    bent("drafts", (s) => (s.log_writable = false)),
    { phrase: "review unavailable · folder not writable", button: null, menu: [] },
  ],
  [
    "an unwritable folder outranks an unwritable doc",
    bent("drafts", (s) => {
      s.log_writable = false;
      s.writable = false;
    }),
    { phrase: "review unavailable · folder not writable" },
  ],

  // The menu's owner row
  [
    "a doc of yours an agent has worked on can be marked as that agent's draft",
    bent("picked", (s) => (s.owner.drafted_by = null)),
    { menu: [COPY, "mark as drafted by Claude"] },
  ],
  [
    "a doc of yours no agent has touched offers no owner row",
    bent("drafts", (s) => (s.owner.drafted_by = null)),
    { menu: [COPY] },
  ],
  ["a hand-back the agent has picked up can't be withdrawn", state("picked"), { menu: [COPY, MINE] }],

  // Withdraw follows the fold's flag, not the stage alone
  ["a hand-back claimed between pushes can't be withdrawn", bent("sent", (s) => (s.phrase.sent.can_withdraw = false)), { menu: [COPY, MINE] }],
  ["an approve the agent has claimed can't be withdrawn", bent("approved", (s) => (s.approve.can_withdraw = false)), { menu: [COPY, MINE] }],
  [
    "inside an approve's hold z is the undo, so the menu has no withdraw",
    state("approved"),
    { menu: [COPY, MINE] },
    (s) => s.hold.not_before - 1_000,
  ],

  // Silence belongs to a pickup that is still open
  [
    "a reply ends the silence, however long ago the pickup was",
    state("replied"),
    { phrase: "Claude replied 14:13 · 3 to check", menu: [COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 20 * MIN,
  ],
  [
    "an approval ends the silence too",
    state("approved"),
    { phrase: "approved 14:13 · c to reopen", menu: [COPY, MINE, "withdraw"] },
    (s) => s.phrase.picked_up.silence.since_ts + 20 * MIN,
  ],
  [
    "silence counts from the agent's last sign of life, not the pickup",
    bent("picked", (s) => {
      s.phrase.picked_up.ts = at(10, 0);
      s.phrase.picked_up.silence.since_ts = at(10, 20);
    }),
    { phrase: "Claude picked up 10:00 · no reply in 16 min" },
    () => at(10, 36),
  ],
  [
    "the silence threshold is the fold's, not a fixed 15 minutes",
    bent("picked", (s) => (s.phrase.picked_up.silence.threshold_ms = 5 * MIN)),
    { phrase: "Claude picked up 14:13 · no reply in 5 min", menu: ["release", COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 5 * MIN,
  ],
  [
    "notes waiting during a long silence keep the release in the menu, and the button With Claude",
    bent("picked", (s) => (s.counts.drafts = 2)),
    { button: B("With Claude", false, true), menu: ["release", COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 20 * MIN,
  ],

  // Ruling 9: "didnt" is not part of the tally
  [
    "what the agent says it didn't do stays out of the phrase",
    bent("replied", (s) => {
      s.counts.to_check = 0;
      s.phrase.replied.didnt = 2;
    }),
    { phrase: "Claude replied 14:13 · all checked" },
  ],

  // Ruling 4: the live claimer, not any claimer in the history
  [
    "an earlier round's claimer is not who has the turn now",
    bent("sent", (s) => {
      s.rounds.unshift({ submit: 3, ts: at(9, 0), by: "", to: "*", items: [4], claim: 4, claimed_by: "codex", yield: 5, released: false });
    }),
    { button: B("With Claude", false, true) },
    soon,
  ],

  // A long name gives way in the button; the page puts the whole name in its title
  [
    "a long agent name is cut to 24 characters in the button",
    bent("picked", (s) => (s.phrase.picked_up.by = s.rounds[0].claimed_by = "agent-" + "x".repeat(54))),
    { button: B("With Agent-" + "x".repeat(18) + "…", false, true) },
  ],
  [
    "a name of 24 characters is shown whole",
    bent("picked", (s) => (s.phrase.picked_up.by = s.rounds[0].claimed_by = "agent-" + "x".repeat(18))),
    { button: B("With Agent-" + "x".repeat(18), false, true) },
  ],

  // Ruling 3: read-only changes the words, not the turn
  [
    "a read-only doc inside a hold still says it is handing back",
    bent("hold", (s) => (s.writable = false)),
    { phrase: "read-only · notes only", button: B("Hand back · 3"), pill: "handing back · 3 notes · z to undo" },
  ],
  [
    "a read-only doc that was sent still waits, and can still be withdrawn",
    bent("sent", (s) => (s.writable = false)),
    { phrase: "read-only · notes only", button: B("With Claude", false, true), menu: [COPY, MINE, "withdraw"] },
    soon,
  ],
];

for (const [label, s, want, nowOf] of CRAFTED) {
  test(`chrome: ${label}`, () => {
    const got = chrome(load(), s, nowOf ? nowOf(s) : s.server_now);
    for (const k of Object.keys(want)) assert.deepEqual(got[k], want[k], `${label}: ${k}`);
  });
}

test("chrome: notes waiting during a long silence still lead with the pickup", () => {
  const s = bent("picked", (x) => (x.counts.drafts = 2));
  const got = chrome(load(), s, s.phrase.picked_up.silence.since_ts + 20 * MIN);
  assert.match(got.phrase, /^Claude picked up 14:13 · /);
  assert.match(got.phrase, /2 notes for next hand-back/);
  assert.doesNotMatch(got.phrase, /release/);
});

// --- Directory mode ------------------------------------------------------------------------------

test("in a folder the button counts every file's drafts, and nothing else", () => {
  const s = bent("drafts", (x) => (x.counts.drafts = 1));
  // The totals count this doc's own draft, which the listing holds
  const here = { paths: ["pr.md"], summary: { stage: "drafts", phrase: s.phrase, counts: s.counts }, news: false };
  const dir = { type: "dir_state", root: ".", truncated: false, folder: null, files: [here], totals: { drafts: 3, unseen_files: 2, decided: 1, items: 5 } };
  const BR = load();
  assert.deepEqual(plain(BR.pure.turnButton(s, { now: s.server_now, dir, path: "pr.md" })), B("Hand back · 3"));
});

// --- Never a blank name, never a leaked value ------------------------------------------------------

test("a nameless human never leaves a blank name, and no value leaks as text", () => {
  const BR = load();
  for (const name of Object.keys(FIXTURE_CHROME)) {
    const s = state(name);
    // The fold gives "" for a human with no name
    for (const r of s.rounds ?? []) r.by = "";
    if (s.approve) s.approve.by = "";
    for (const now of [s.server_now, s.hold ? s.hold.not_before - 1 : s.server_now]) {
      const c = chrome(BR, s, now);
      const texts = [c.phrase, c.button?.text ?? "", c.pill ?? "", ...c.menu];
      for (const t of texts) {
        assert.doesNotMatch(t, /\bby\s*(·|$)/, `${name}: ${JSON.stringify(t)}`);
        assert.doesNotMatch(t, /undefined|null|NaN|\[object/, `${name}: ${JSON.stringify(t)}`);
        assert.doesNotMatch(t, /\s{2}|^\s|\s$|·\s*·/, `${name}: ${JSON.stringify(t)}`);
      }
    }
  }
});

test("the chrome leaves its state alone", () => {
  const BR = load();
  for (const name of Object.keys(FIXTURE_CHROME)) {
    const s = state(name);
    const before = JSON.stringify(s);
    chrome(BR, s);
    chrome(BR, s, s.hold ? s.hold.not_before - 1 : s.server_now);
    assert.equal(JSON.stringify(s), before, name);
  }
});

test("menu rows name the op each sends", () => {
  const BR = load();
  const rows = (s, now = s.server_now) => plain(BR.pure.menuRows(s, { now, dir: null }));
  const sent = state("sent");
  assert.deepEqual(rows(sent).find((r) => r.text === "withdraw").op, { op: "withdraw", to: sent.phrase.sent.n });
  const approved = state("approved");
  assert.deepEqual(rows(approved).find((r) => r.text === "withdraw").op, { op: "withdraw", to: approved.approve.n });
  const picked = state("picked");
  const late = picked.phrase.picked_up.silence.since_ts + 15 * MIN;
  assert.deepEqual(rows(picked, late).find((r) => r.text === "release").op, { op: "release", claim: picked.phrase.picked_up.n });
  assert.deepEqual(rows(picked).find((r) => r.text === MINE).op, { op: "owner", drafted_by: null });
  const yours = bent("picked", (s) => (s.owner.drafted_by = null));
  assert.deepEqual(rows(yours).find((r) => r.text === "mark as drafted by Claude").op, { op: "owner", drafted_by: "claude" });
  assert.equal(rows(sent).find((r) => r.text === COPY).copy, sent.hand_off);
});

test("a refusal reads as words, naming whoever picked the round up", () => {
  const BR = load();
  const text = (reason, s = null) => BR.pure.nackText(reason, s);
  assert.equal(text("changed_since"), "changed since · try again");
  assert.equal(text("nothing_to_send"), "nothing to hand back");
  assert.equal(text("hold_pending"), "already handing back");
  assert.equal(text("bad_anchor"), "bad anchor");
  assert.equal(text("stale_rev"), "stale rev");
  const picked = bent("picked", (s) => (s.phrase.picked_up.by = s.rounds[0].claimed_by = "codex"));
  assert.equal(text("claimed", picked), "Codex already picked it up");
  // Before the state that names the claimer arrives, the drafter is the best guess
  assert.equal(text("claimed", state("sent")), "Claude already picked it up");
  assert.equal(text("claimed", bent("sent", (s) => (s.owner.drafted_by = null))), "the agent already picked it up");
  assert.equal(text("claimed", null), "the agent already picked it up");
});

test("a doc write while Claude has the doc says it works again after the reply", () => {
  assert.equal(load().pure.nackText("held", state("sent")), "Claude has the doc · this works again after its reply");
  assert.equal(load().pure.nackText("held", null), "Claude has the doc · this works again after its reply");
});

// --- The page's own notes, before any state shows them -----------------------------------------

test("a note the page kept that no state shows yet makes the button a hand-back, never Approve", () => {
  const BR = load();
  const s = state("fresh");
  assert.equal(plain(BR.pure.turnButton(s, { now: s.server_now })).text, "Approve");
  assert.equal(plain(BR.pure.turnButton(s, { now: s.server_now, pending: 1, busy: true })).text, "Hand back · 1");
});

test("the page's pending notes add to the state's drafts", () => {
  const BR = load();
  const s = bent("drafts", (x) => (x.counts.drafts = 2));
  assert.equal(plain(BR.pure.turnButton(s, { now: s.server_now, pending: 1, busy: true })).text, "Hand back · 3");
});

test("an amend still on its way is a hand-back too, though it adds no note", () => {
  const BR = load();
  const s = state("fresh");
  assert.equal(plain(BR.pure.turnButton(s, { now: s.server_now, pending: 0, busy: true })).text, "Hand back · 1");
});

test("in a folder the page's pending notes add to the listing's drafts", () => {
  const BR = load();
  const dir = { type: "dir_state", root: ".", truncated: false, folder: null, files: [], totals: { drafts: 2, unseen_files: 0, decided: 0, items: 0 } };
  const s = state("fresh");
  assert.equal(plain(BR.pure.turnButton(s, { now: s.server_now, dir, pending: 1, busy: true })).text, "Hand back · 3");
});

// --- adversary: rulings and stages the phase table leaves out ------------------------------------

test("adversary: a note written during the hold joins the message's count too, so the button and the message agree", () => {
  const BR = load();
  const s = bent("hold", (x) => {
    notesOnly(x);
    x.counts.drafts = 1;
  });
  const c = chrome(BR, s);
  assert.deepEqual(c.button, B("Hand back · 4"));
  assert.equal(c.pill, "handing back · 4 notes · z to undo");
});

/** A dir_state listing pr.md and checklist.md, the latter `stage` with its hand-back at `sentTs`. */
function batchListing(stage, sentTs) {
  const other = bent(stage === "sent" ? "sent" : "picked", (x) => (x.phrase.sent.ts = sentTs));
  const self = pickedAt1656();
  return {
    type: "dir_state",
    root: ".",
    truncated: false,
    folder: null,
    files: [
      { paths: ["pr.md"], summary: { stage: "picked_up", phrase: self.phrase, counts: self.counts, notes: 1 }, news: false },
      { paths: ["checklist.md"], summary: { stage, phrase: other.phrase, counts: other.counts, notes: 1 }, news: false },
    ],
    totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0 },
  };
}

test("adversary: folder: only a file from this same hand-back is named as not picked up", () => {
  const BR = load(WITH_DIR);
  const s = pickedAt1656();
  const phrase = (dir) => BR.pure.formatPhrase(s, { now: s.server_now, dir, path: "pr.md", folder: true });
  // Setup: this doc's hand-back is at 16:55
  assert.equal(s.phrase.sent.ts, T(16, 55));
  assert.equal(phrase(batchListing("sent", T(16, 55))), "Claude picked up 16:56 · checklist.md not picked up ▾");
  // A file still waiting from an hour-old hand-back is not of this one
  assert.equal(phrase(batchListing("sent", T(15, 55))), "Claude picked up 16:56 · 1 other file ▾");
});

test("adversary: with birta stopped, a hold or an unclaimed hand-back still reads stopped, and the button takes no press", () => {
  const BR = load();
  for (const [name, now] of [
    ["hold", (s) => s.hold.not_before - 1_000],
    ["sent", (s) => s.server_now],
    ["sent", soon],
  ]) {
    const s = state(name);
    const t = now(s);
    const was = plain(BR.pure.turnButton(s, { now: t }));
    assert.equal(BR.pure.formatPhrase(s, { now: t, stopped: true }), "birta stopped · notes wait here", `${name} at ${t}`);
    const got = plain(BR.pure.turnButton(s, { now: t, stopped: true }));
    assert.equal(got.text, was.text, `${name} at ${t}`);
    assert.equal(got.disabled, true, `${name} at ${t}`);
  }
});
