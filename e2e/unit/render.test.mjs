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

function bent(name, edit) {
  const s = state(name);
  edit(s);
  return s;
}

const plain = (v) => (v == null ? v : JSON.parse(JSON.stringify(v)));

/** The phrase and its tooltip for `s` at its own server time. */
function phrase(BR, s, now = s.server_now) {
  return { phrase: BR.pure.formatPhrase(s, { now }), title: BR.pure.phraseTitle(s, { now }) };
}

const same = (text) => ({ phrase: text, title: text });

/** Nothing of the agent's left unseen: no change and no reply. */
function allSeen(s) {
  s.counts.unseen = 0;
  for (const i of s.items) for (const r of i.replies ?? []) r.seen = true;
}

// --- The phrase never drops counts ------------------------------------------------------------

test("the phrase has a tooltip function", () => {
  assert.equal(typeof load().pure.phraseTitle, "function");
});

const FIXTURE_PHRASE = {
  // No bare item count: a fresh doc says only who drafted it and whose turn it is
  fresh: same("drafted by Claude · your turn"),
  // Notes and decided blocks together, never one in place of the other
  drafts: same("your turn · 3 notes · 1 of 3 blocks decided"),
  dir: same("read-only · notes only"),
  // Drafting after a reply keeps the reply: unseen (changes and replies, as the button counts), notes, decided; the rest in the tooltip
  flow: {
    phrase: "Claude replied 14:13 · 6 unseen · your turn · 1 note · 2 of 3 blocks decided",
    title:
      "Claude replied 14:13 · 6 unseen · your turn · 1 note · 2 of 3 blocks decided · 2 done · 1 answered · 2 for you · 1 without a note · 3 not handled",
  },
  hold: same("your turn · 3 notes · 1 of 3 blocks decided"),
  sent: same("sent 14:13 · not picked up yet"),
  picked: same("Claude picked up 14:13"),
  // With slots left after unseen and decided, the reply's tallies fill them in order
  replied: {
    phrase: "Claude replied 14:13 · 2 unseen · 1 of 3 blocks decided · 1 done",
    title: "Claude replied 14:13 · 2 unseen · 1 of 3 blocks decided · 1 done · 1 answered · 1 not handled",
  },
  unseen: {
    phrase: "Claude replied 14:13 · 4 unseen · 1 of 3 blocks decided · 1 done",
    title: "Claude replied 14:13 · 4 unseen · 1 of 3 blocks decided · 1 done · 1 answered · 2 without a note · 1 not handled",
  },
  rewrite: {
    phrase: "Claude replied 14:13 · 9 unseen · 1 of 3 blocks decided · 1 done",
    title: "Claude replied 14:13 · 9 unseen · 1 of 3 blocks decided · 1 done · 1 answered · 6 without a note",
  },
  lost: {
    phrase: "Claude replied 14:13 · 3 unseen · 1 of 3 blocks decided · 1 done",
    title: "Claude replied 14:13 · 3 unseen · 1 of 3 blocks decided · 1 done · 1 answered · 1 not handled",
  },
  // The clean view: what the approve left stays in the tooltip
  approved: { phrase: "approved 14:13 · c to reopen", title: "approved 14:13 · 1 later · c to reopen" },
};

test("the phrase table covers every committed state", () => {
  const names = fs
    .readdirSync(FIXTURES)
    .filter((n) => n.endsWith(".json"))
    .map((n) => n.slice(0, -5))
    .sort();
  assert.deepEqual(names, Object.keys(FIXTURE_PHRASE).sort());
});

for (const [name, want] of Object.entries(FIXTURE_PHRASE)) {
  test(`phrase and tooltip for ${name}.json`, () => {
    assert.deepEqual(phrase(load(), state(name)), want);
  });
}

/** 23:25 UTC on the fixtures' day. */
const AT_2325 = Date.UTC(2026, 8, 21, 23, 25, 7);

const CRAFTED = [
  [
    "the spec's own example reads as written",
    bent("flow", (s) => {
      allSeen(s);
      Object.assign(s.counts, { unseen: 2, drafts: 1, decided: 1, items: 7 });
      Object.assign(s.phrase.replied, { ts: AT_2325, done: 0, answered: 0, asked: 0, unasked: 0, not_handled: 0 });
    }),
    same("Claude replied 23:25 · 2 unseen · your turn · 1 note · 1 of 7 blocks decided"),
  ],
  [
    "notes with no verdict leave decided out",
    bent("drafts", (s) => (s.counts.decided = 0)),
    same("your turn · 3 notes"),
  ],
  ["one note is singular", bent("drafts", (s) => Object.assign(s.counts, { drafts: 1, decided: 0 })), same("your turn · 1 note")],
  [
    "a fresh doc of yours says only whose turn",
    bent("fresh", (s) => (s.owner.drafted_by = null)),
    same("your turn"),
  ],
  [
    "the drafter's name comes from the state",
    bent("fresh", (s) => (s.owner.drafted_by = "codex")),
    same("drafted by Codex · your turn"),
  ],
  [
    "a reply with nothing unseen and nothing decided shows its first three tallies",
    bent("replied", (s) => {
      allSeen(s);
      s.counts.decided = 0;
      Object.assign(s.phrase.replied, { done: 3, answered: 1, asked: 2, unasked: 1, not_handled: 2 });
    }),
    {
      phrase: "Claude replied 14:13 · 3 done · 1 answered · 2 for you",
      title: "Claude replied 14:13 · 3 done · 1 answered · 2 for you · 1 without a note · 2 not handled",
    },
  ],
  [
    "a reply with no tallies at all is just the reply",
    bent("replied", (s) => {
      allSeen(s);
      s.counts.decided = 0;
      Object.assign(s.phrase.replied, { done: 0, answered: 0, asked: 0, unasked: 0, not_handled: 0 });
    }),
    same("Claude replied 14:13"),
  ],
  [
    "a verdict after a reply counts decided and keeps what the turn left alone in the tooltip",
    bent("replied", (s) => {
      allSeen(s);
      Object.assign(s.counts, { drafts: 1, decided: 1 });
    }),
    {
      phrase: "Claude replied 14:13 · your turn · 1 note · 1 of 3 blocks decided · 1 done",
      title: "Claude replied 14:13 · your turn · 1 note · 1 of 3 blocks decided · 1 done · 1 answered · 1 not handled",
    },
  ],
];

for (const [name, s, want] of CRAFTED) {
  test(`phrase: ${name}`, () => {
    assert.deepEqual(phrase(load(), s), want);
  });
}

test("no phrase cuts a count to …, counts items or says unasked", () => {
  const BR = load();
  for (const name of Object.keys(FIXTURE_PHRASE)) {
    const s = state(name);
    const { phrase: p, title } = phrase(BR, s);
    assert.ok(!p.includes("…") && !title.includes("…"), `${name}: ${p}`);
    assert.ok(!/\bitems?\b/.test(p) && !/\bitems?\b/.test(title), `${name}: ${p}`);
    assert.ok(!/unasked/.test(title), `${name}: ${title}`);
  }
});

test("the phrase shows at most three counts, and the tooltip holds every part of it in order", () => {
  const BR = load();
  for (const [name, s] of [...Object.keys(FIXTURE_PHRASE).map((n) => [n, state(n)]), ...CRAFTED.map(([n, s]) => [n, s])]) {
    const { phrase: p, title } = phrase(BR, s);
    const counts = p.split(" · ").filter((part) => /^\d/.test(part));
    assert.ok(counts.length <= 3, `${name}: ${p}`);
    const parts = title.split(" · ");
    let at = -1;
    for (const part of p.split(" · ")) {
      at = parts.indexOf(part, at + 1);
      assert.ok(at >= 0, `${name}: ${part} in ${title}`);
    }
  }
});

// --- Bars by round ---------------------------------------------------------------------------

/** unseen.json with its first change seen and settled, as an earlier round leaves it. */
function rounds(edit = () => {}) {
  return bent("unseen", (s) => {
    s.changes[0].seen = true;
    s.changes[0].settled = true;
    edit(s);
  });
}

const bars = (BR, s) =>
  plain(BR.pure.laneMarks(s, { now: s.server_now }))
    .filter((m) => m.mark === "bar")
    .map((m) => ({ sp: m.sp, weight: m.weight, fresh: m.fresh }));

test("an earlier round's settled change keeps a 1 px bar; this round's are 3 px, fresh until seen", () => {
  const BR = load();
  assert.deepEqual(bars(BR, rounds()), [
    { sp: "5:1-6:68", weight: 1, fresh: false },
    { sp: "11:1-11:44", weight: 3, fresh: true },
  ]);
  assert.deepEqual(bars(BR, rounds((s) => (s.changes[1].seen = true))), [
    { sp: "5:1-6:68", weight: 1, fresh: false },
    { sp: "11:1-11:44", weight: 3, fresh: false },
  ]);
});

test("a settled change draws no wash, and a block's newest unsettled change decides its bar", () => {
  const BR = load();
  const s = rounds((s) => {
    // A later, unseen change on the settled one's block
    s.changes.push({ ...JSON.parse(JSON.stringify(s.changes[1])), id: "12.0", op: 12, range: { ...s.changes[0].range } });
  });
  const marks = plain(BR.pure.laneMarks(s, { now: s.server_now }));
  assert.deepEqual(
    marks.filter((m) => m.mark === "bar" && m.sp === "5:1-6:68").map((m) => [m.weight, m.fresh]),
    [[3, true]],
  );
  assert.deepEqual(marks.filter((m) => m.mark === "wash").map((m) => m.sp).sort(), ["11:1-11:44", "5:1-6:68"]);
  assert.deepEqual(
    plain(BR.pure.laneMarks(rounds(), { now: 0 })).filter((m) => m.mark === "wash").map((m) => m.sp),
    ["11:1-11:44"],
  );
});

test("a round with no changes of its own leaves every bar thin and every settled removal unticked", () => {
  const BR = load();
  const s = bent("unseen", (s) => {
    for (const c of s.changes) Object.assign(c, { seen: true, settled: true });
    s.rounds.push({ ...s.rounds[0], submit: 20, claim: 21, yield: 25, items: [] });
  });
  assert.deepEqual(bars(BR, s), [
    { sp: "5:1-6:68", weight: 1, fresh: false },
    { sp: "11:1-11:44", weight: 1, fresh: false },
  ]);
  const marks = plain(BR.pure.laneMarks(s, { now: 0 }));
  assert.deepEqual(marks.filter((m) => m.mark === "tick" || m.mark === "wash"), []);
  assert.deepEqual(plain(BR.pure.attention(s)).filter((x) => x.kind === "change"), []);
});

test("an outside change's weight follows settled too, and a changed-since approve keeps its words", () => {
  const BR = load();
  const s = rounds((s) => Object.assign(s.changes[1], { attr: "outside", turn: null, seen: true, settled: true }));
  assert.deepEqual(bars(BR, s).map((b) => b.weight), [1, 1]);
  const a = bent("approved", (s) => (s.approve.changed_since = true));
  assert.deepEqual(phrase(BR, a), {
    phrase: "approved 14:13 · changed since · c to reopen",
    title: "approved 14:13 · changed since · 1 later · c to reopen",
  });
});

test("one block of one is singular", () => {
  const BR = load();
  const s = bent("drafts", (s) => Object.assign(s.counts, { drafts: 1, decided: 1, items: 1 }));
  assert.equal(BR.pure.formatPhrase(s, { now: s.server_now }), "your turn · 1 note · 1 of 1 block decided");
});

test("a removal of whole blocks draws a tick and no bar", () => {
  const BR = load();
  const marks = plain(BR.pure.laneMarks(state("unseen"), { now: 0 }));
  assert.ok(!marks.some((m) => m.mark === "bar" && m.sp === "14:1-14:13"));
  assert.deepEqual(
    marks.filter((m) => m.mark === "tick").map((m) => m.after),
    ["14:1-14:13"],
  );
});
