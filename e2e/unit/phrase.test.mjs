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

/** Everything the chrome says for `s` at server time `now`; the menu without its closing keys row. */
function chrome(BR, s, now = s.server_now, dir = null) {
  const menu = plain(BR.pure.menuRows(s, { now, dir })).map((r) => r.text);
  // Every menu review can act in ends with the key sheet's row
  if (s.log_writable !== false) assert.equal(menu.pop(), "keys");
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

const FIXTURE_CHROME = {
  fresh: {
    phrase: "drafted by Claude · your turn",
    button: B("Approve"),
    pill: null,
    menu: [COPY, MINE],
  },
  // One list item carries a verdict; the paragraph's doesn't count toward the blocks
  drafts: { phrase: "your turn · 3 notes · 1 of 3 blocks decided", button: B("Hand back · 3"), pill: null, menu: [COPY, MINE] },
  // The doc can't be written, so notes are all it takes
  dir: { phrase: "read-only · notes only", button: B("Hand back · 2"), pill: null, menu: [COPY] },
  // Writing again after a reply keeps the reply: unseen, notes and decided, the rest in the tooltip
  flow: {
    phrase: "Claude replied 14:13 · 6 unseen · your turn · 1 note · 2 of 3 blocks decided",
    button: B("Hand back · 1"),
    pill: null,
    menu: [COPY, MINE],
  },
  // Inside the hold nothing has left yet: the phrase and button keep what they said before
  hold: {
    phrase: "your turn · 3 notes · 1 of 3 blocks decided",
    button: B("Hand back · 3"),
    pill: "handing back · 3 notes · z to undo",
    menu: [COPY, MINE],
  },
  sent: {
    phrase: "sent 14:13 · not picked up yet",
    button: B("With Claude", false, true),
    pill: null,
    menu: ["withdraw", COPY, MINE],
  },
  picked: { phrase: "Claude picked up 14:13", button: B("With Claude", false, true), pill: null, menu: [COPY, MINE] },
  replied: {
    phrase: "Claude replied 14:13 · 2 unseen · 1 of 3 blocks decided · 1 done",
    button: B("Approve · 2 unseen", false),
    pill: null,
    menu: [COPY, MINE],
  },
  unseen: {
    phrase: "Claude replied 14:13 · 4 unseen · 1 of 3 blocks decided · 1 done",
    button: B("Approve · 4 unseen", false),
    pill: null,
    menu: [COPY, MINE],
  },
  rewrite: {
    phrase: "Claude replied 14:13 · 6 unseen · 1 of 3 blocks decided · 1 done",
    button: B("Approve · 6 unseen", false),
    pill: null,
    menu: [COPY, MINE],
  },
  lost: {
    phrase: "Claude replied 14:13 · 3 unseen · 1 of 3 blocks decided · 1 done",
    button: B("Approve · 3 unseen", false),
    pill: null,
    menu: [COPY, MINE],
  },
  // The approve's hold has passed and nothing claimed it, so it can still be taken back
  approved: {
    phrase: "approved 14:13 · c to reopen",
    button: B("Approved", false, true),
    pill: null,
    menu: ["withdraw", COPY, MINE],
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
    "notes with no item verdict count notes, not items",
    bent("drafts", notesOnly),
    { phrase: "your turn · 3 notes", button: B("Hand back · 3") },
  ],
  [
    "one note is singular",
    bent("drafts", (s) => {
      notesOnly(s);
      s.items = s.items.slice(0, 1);
      s.counts.drafts = 1;
    }),
    { phrase: "your turn · 1 note", button: B("Hand back · 1") },
  ],
  [
    "a verdict on the whole doc never flips the mode",
    bent("drafts", (s) => {
      s.items = s.items.filter((i) => i.at.kind === "doc").map((i) => ({ ...i, verdict: "no" }));
      s.counts.drafts = 1;
      s.counts.decided = 0;
    }),
    { phrase: "your turn · 1 note", button: B("Hand back · 1") },
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
    { phrase: "your turn · 3 notes · 1 of 3 blocks decided", button: B("Hand back · 3"), pill: "handing back · 3 notes · z to undo" },
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
    { phrase: "your turn · 3 notes", button: B("Hand back · 3"), pill: "handing back · 3 notes · z to undo" },
  ],
  [
    "a held hand-back of one note is singular",
    bent("hold", (s) => {
      notesOnly(s);
      s.hold.count = 1;
    }),
    { phrase: "your turn · 1 note", button: B("Hand back · 1"), pill: "handing back · 1 note · z to undo" },
  ],
  [
    "a note kept during the hold joins the count it shows",
    bent("hold", (s) => {
      notesOnly(s);
      s.counts.drafts = 1;
    }),
    { phrase: "your turn · 4 notes", button: B("Hand back · 4"), pill: "handing back · 3 notes · z to undo" },
  ],
  [
    "a held approve keeps the phrase from before it, and its button",
    state("approved"),
    {
      phrase: "Claude replied 14:13 · 2 of 3 blocks decided · 1 done · 1 answered",
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
    { phrase: "Claude replied 11:45 · 2 unseen · 1 of 3 blocks decided · 1 done" },
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
  ],
  [
    "before pickup a doc of yours names no one",
    bent("sent", (s) => (s.owner.drafted_by = null)),
    { button: B("With the agent", false, true), menu: ["withdraw", COPY] },
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
    { phrase: "Codex replied 14:13 · 2 unseen · 1 of 3 blocks decided · 1 done" },
  ],

  // A long silence after pickup is stated at 15 minutes, by the server's clock
  [
    "a second before 15 minutes of silence nothing is said",
    state("picked"),
    { phrase: "Claude picked up 14:13", menu: [COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 15 * MIN - 1_000,
  ],
  [
    "at 15 minutes of silence the phrase says so and the menu can release",
    state("picked"),
    { phrase: "Claude picked up 14:13 · no reply in 15 min · release", menu: ["release", COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 15 * MIN,
  ],
  [
    "the silence counts whole minutes",
    state("picked"),
    { phrase: "Claude picked up 14:13 · no reply in 40 min · release" },
    (s) => s.phrase.picked_up.silence.since_ts + 40 * MIN + 30_000,
  ],

  // Writing while the agent holds the turn: the waiting count follows the mode (design call 5)
  [
    "with an item decided, what waits for the next hand-back is counted as decided",
    bent("picked", (s) => (s.counts.drafts = 2)),
    { phrase: "Claude picked up 14:13 · 2 decided for next hand-back", button: B("Hand back · 2") },
  ],
  [
    "with no item decided, what waits for the next hand-back is counted as notes",
    bent("picked", (s) => {
      notesOnly(s);
      s.counts.drafts = 2;
    }),
    { phrase: "Claude picked up 14:13 · 2 notes for next hand-back", button: B("Hand back · 2") },
  ],
  [
    "one note waiting is singular",
    bent("picked", (s) => {
      notesOnly(s);
      s.counts.drafts = 1;
    }),
    { phrase: "Claude picked up 14:13 · 1 note for next hand-back", button: B("Hand back · 1") },
  ],

  // The reply's tally fills the slots unseen, notes and decided leave: zeros are left out, the agent's questions are "for you"
  [
    "the reply counts done, questions for you and not handled, in that order",
    bent("replied", (s) => {
      Object.assign(s.counts, { unseen: 0, decided: 0 });
      for (const i of s.items) for (const r of i.replies ?? []) r.seen = true;
      Object.assign(s.phrase.replied, { done: 4, answered: 0, asked: 1, unasked: 0, not_handled: 1 });
    }),
    { phrase: "Claude replied 14:13 · 4 done · 1 for you · 1 not handled" },
  ],
  [
    "a reply with every count shows the first three",
    bent("replied", (s) => {
      Object.assign(s.counts, { unseen: 0, decided: 0 });
      for (const i of s.items) for (const r of i.replies ?? []) r.seen = true;
      Object.assign(s.phrase.replied, { done: 3, answered: 1, asked: 2, unasked: 1, not_handled: 2 });
    }),
    { phrase: "Claude replied 14:13 · 3 done · 1 answered · 2 for you" },
  ],

  // Writing again after a reply
  [
    "notes after a reply keep the reply and count notes",
    bent("replied", (s) => {
      s.counts.drafts = 1;
      s.counts.decided = 0;
    }),
    { phrase: "Claude replied 14:13 · 2 unseen · your turn · 1 note · 1 done", button: B("Hand back · 1") },
  ],
  [
    "a verdict after a reply keeps the reply and counts notes and decided",
    bent("replied", (s) => (s.counts.drafts = 1)),
    { phrase: "Claude replied 14:13 · 2 unseen · your turn · 1 note · 1 of 3 blocks decided" },
  ],

  // Approve
  [
    "an approval whose text changed says so, and can be approved again",
    bent("approved", (s) => {
      s.approve.changed_since = true;
      s.approve.current_matches = false;
      s.approve.later = 0;
    }),
    { phrase: "approved 14:13 · changed since · c to reopen", button: B("Approve") },
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
  // What waits for later goes to the tooltip; the clean view's phrase stays bare
  ["an approval leaves what waits for later out of the phrase", bent("approved", (s) => (s.approve.later = 2)), { phrase: "approved 14:13 · c to reopen" }],
  [
    "unanswered questions show on the approve button",
    bent("replied", (s) => {
      s.counts.unseen = 0;
      s.counts.open_asks = 1;
      s.items.find((i) => i.n === 6).replies[0].seen = true;
    }),
    { button: B("Approve · 1 unanswered", false) },
  ],
  [
    "unseen changes win over unanswered questions",
    bent("replied", (s) => {
      s.counts.unseen = 2;
      s.counts.open_asks = 1;
    }),
    // Two changes and the reply
    { button: B("Approve · 3 unseen", false) },
  ],
  [
    "with nothing unseen or unanswered the button approves",
    bent("replied", (s) => {
      s.counts.unseen = 0;
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
    { phrase: "Claude replied 14:13 · 2 unseen · 1 of 3 blocks decided · 1 done", menu: [COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 20 * MIN,
  ],
  [
    "an approval ends the silence too",
    state("approved"),
    { phrase: "approved 14:13 · c to reopen", menu: ["withdraw", COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 20 * MIN,
  ],
  [
    "silence counts from the agent's last sign of life, not the pickup",
    bent("picked", (s) => {
      s.phrase.picked_up.ts = at(10, 0);
      s.phrase.picked_up.silence.since_ts = at(10, 20);
    }),
    { phrase: "Claude picked up 10:00 · no reply in 16 min · release" },
    () => at(10, 36),
  ],
  [
    "the silence threshold is the fold's, not a fixed 15 minutes",
    bent("picked", (s) => (s.phrase.picked_up.silence.threshold_ms = 5 * MIN)),
    { phrase: "Claude picked up 14:13 · no reply in 5 min · release", menu: ["release", COPY, MINE] },
    (s) => s.phrase.picked_up.silence.since_ts + 5 * MIN,
  ],
  [
    "notes waiting during a long silence keep both the count and the release",
    bent("picked", (s) => (s.counts.drafts = 2)),
    {
      phrase: "Claude picked up 14:13 · 2 decided for next hand-back · no reply in 20 min · release",
      button: B("Hand back · 2"),
      menu: ["release", COPY, MINE],
    },
    (s) => s.phrase.picked_up.silence.since_ts + 20 * MIN,
  ],

  // Ruling 9: "didnt" is not part of the tally
  [
    "what the agent says it didn't do stays out of the tally",
    bent("replied", (s) => {
      Object.assign(s.counts, { unseen: 0, decided: 0 });
      for (const i of s.items) for (const r of i.replies ?? []) r.seen = true;
      s.phrase.replied.didnt = 2;
    }),
    { phrase: "Claude replied 14:13 · 1 done · 1 answered · 1 not handled" },
  ],

  // Ruling 4: the live claimer, not any claimer in the history
  [
    "an earlier round's claimer is not who has the turn now",
    bent("sent", (s) => {
      s.rounds.unshift({ submit: 3, ts: at(9, 0), by: "", to: "*", items: [4], claim: 4, claimed_by: "codex", yield: 5, released: false });
    }),
    { button: B("With Claude", false, true) },
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
    { phrase: "read-only · notes only", button: B("With Claude", false, true), menu: ["withdraw", COPY, MINE] },
  ],
];

for (const [label, s, want, nowOf] of CRAFTED) {
  test(`chrome: ${label}`, () => {
    const got = chrome(load(), s, nowOf ? nowOf(s) : s.server_now);
    for (const k of Object.keys(want)) assert.deepEqual(got[k], want[k], `${label}: ${k}`);
  });
}

// --- Directory mode ------------------------------------------------------------------------------

test("in a folder the button counts every file's drafts and the files with unseen changes", () => {
  const s = bent("drafts", (x) => (x.counts.drafts = 1));
  // The totals count this doc's own draft, which the listing holds
  const here = { paths: ["pr.md"], summary: { stage: "drafts", phrase: s.phrase, counts: s.counts }, news: false };
  const dir = { type: "dir_state", root: ".", truncated: false, folder: null, files: [here], totals: { drafts: 3, unseen_files: 2, decided: 1, items: 5 } };
  const BR = load();
  assert.deepEqual(plain(BR.pure.turnButton(s, { now: s.server_now, dir, path: "pr.md" })), B("Hand back · 3 · 2 files unseen", false));
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
