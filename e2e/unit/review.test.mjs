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
const OURS = ["00-core.js", "40-lane.js"];
const ORDER = ["quote.js", "00-core.js", "40-lane.js"];

/** Runs the review files in a context with no DOM, as they must touch none at load. */
function load(files = ORDER, globals = {}) {
  const ctx = vm.createContext(globals);
  for (const f of files) {
    const file = path.join(ASSETS, f);
    vm.runInContext(fs.readFileSync(file, "utf-8"), ctx, { filename: file });
  }
  return ctx;
}

function state(name) {
  return JSON.parse(fs.readFileSync(path.join(FIXTURES, `${name}.json`), "utf-8"));
}

for (const f of OURS) {
  test(`${f} starts by reusing the shared BR namespace`, () => {
    assert.equal(fs.readFileSync(path.join(ASSETS, f), "utf-8").split("\n")[0], SHARED_BR);
  });
}

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the core and the lane load without a DOM, ${name}`, () => {
    const ctx = load(files);
    const { BR } = ctx;
    assert.equal(typeof BR.boot, "function");
    assert.equal(typeof BR.op, "function");
    assert.equal(typeof BR.isHost, "function");
    assert.ok(BR.FLOW_HOSTS instanceof vm.runInContext("WeakSet", ctx));
    assert.equal(typeof BR.pure.laneMarks, "function");
    assert.equal(typeof BR.quote.fromRange, "function");
  });
}

test("isHost trusts FLOW_HOSTS members and nothing plain", () => {
  const { BR } = load();
  const member = { nodeType: 1, localName: "div" };
  BR.FLOW_HOSTS.add(member);
  assert.equal(BR.isHost(member), true);
  assert.equal(BR.isHost({ nodeType: 1, localName: "div" }), false);
  assert.equal(BR.isHost({ nodeType: 1, localName: "p" }), false);
  assert.equal(BR.isHost({ nodeType: 3 }), false);
  assert.equal(BR.isHost(null), false);
});

// --- laneMarks: what the lane draws for a state, before any geometry -------------------------

const LANE = new Set(["bar", "glyph", "wash", "tick"]);

/** `undefined` in an expected mark leaves that field unpinned. */
function bar(sp, style, role) {
  return { mark: "bar", sp, style, role };
}
function glyph(sp, verdict, role) {
  return { mark: "glyph", sp, verdict, role };
}
function wash(sp) {
  return { mark: "wash", sp };
}
function tick(after) {
  return { mark: "tick", after };
}

const where = (m) => `${m.mark} ${m.mark === "tick" ? m.after ?? "top" : m.sp}`;

function assertMarks(got, want, label) {
  // Arrays from the vm context have their own prototypes
  const drawn = JSON.parse(JSON.stringify(got)).filter((m) => LANE.has(m.mark));
  assert.deepEqual(drawn.map(where).sort(), want.map(where).sort(), `${label}: where the marks go`);
  for (const w of want) {
    const g = drawn.find((m) => where(m) === where(w));
    for (const [k, v] of Object.entries(w)) {
      if (v === undefined) continue;
      assert.equal(k === "after" ? g[k] ?? null : g[k], v, `${label}: ${where(w)} ${k}`);
    }
  }
}

const FIXTURE_MARKS = {
  fresh: [],
  dir: [],
  drafts: [glyph("5:1-6:54", "no", "you"), glyph("10:1-10:16", "no", "you")],
  sent: [glyph("5:1-6:54", "no", "agent"), glyph("10:1-10:16", "no", "agent")],
  // Inside the hold nothing has left yet, so the held verdicts keep your colour
  hold: [glyph("5:1-6:54", "no", "you"), glyph("10:1-10:16", "no", "you")],
  picked: [glyph("5:1-6:54", "no", "agent"), glyph("10:1-10:16", "no", "agent")],
  replied: [
    glyph("5:1-6:68", "no", "agent"),
    glyph("10:1-10:16", "no", "agent"),
    bar("5:1-6:68", "solid", "agent"),
    wash("5:1-6:68"),
  ],
  unseen: [
    glyph("5:1-6:68", "no", "agent"),
    glyph("10:1-10:16", "no", "agent"),
    bar("5:1-6:68", "solid", "agent"),
    bar("11:1-11:44", "dotted", "agent"),
    wash("5:1-6:68"),
    wash("11:1-11:44"),
    tick("14:1-14:13"),
  ],
  // The agent's change has settled, so it keeps no bar and no wash
  // A later item has been sent and carries across rounds, so it is grey
  approved: [glyph("5:1-6:68", "no", "agent"), glyph("10:1-10:16", "no", "agent"), glyph("12:1-13:0", "later", "agent")],
  // An asked and a later outside change share the paragraph: the newest unseen one's bar wins
  lost: [glyph("10:1-10:16", "no", "agent"), bar("5:1-6:68", "dotted", "agent"), wash("5:1-6:68")],
  rewrite: [glyph("5:1-6:68", "no", "agent"), glyph("10:1-10:29", "no", "agent")],
  // Cited and asked fixes are solid; the unasked change and both outside writes are dotted
  flow: [
    glyph("11:1-11:44", "no", "agent"),
    glyph("10:1-10:16", "no", "agent"),
    glyph("3:1-3:6", "ask", "agent"),
    glyph("1:1-1:16", "ask", "agent"),
    bar("5:1-6:68", "solid", "agent"),
    bar("11:1-11:44", "solid", "agent"),
    bar("14:1-14:21", "dotted", "agent"),
    bar("8:1-8:15", "dotted", "agent"),
    bar("16:1-16:19", "dotted", "agent"),
    wash("5:1-6:68"),
    wash("11:1-11:44"),
    wash("14:1-14:21"),
    wash("8:1-8:15"),
    wash("16:1-16:19"),
  ],
};

test("the table covers every committed state", () => {
  const names = fs
    .readdirSync(FIXTURES)
    .filter((n) => n.endsWith(".json"))
    .map((n) => n.slice(0, -5))
    .sort();
  assert.deepEqual(names, Object.keys(FIXTURE_MARKS).sort());
});

for (const [name, want] of Object.entries(FIXTURE_MARKS)) {
  test(`laneMarks for ${name}.json`, () => {
    const { BR } = load();
    assertMarks(BR.pure.laneMarks(state(name)), want, name);
  });
}

const UNSEEN = FIXTURE_MARKS.unseen;
const bent = (edit) => {
  const s = state("unseen");
  edit(s);
  return s;
};
const replace = (list, at, ...marks) => [...list.filter((m) => where(m) !== at), ...marks];

const CRAFTED = [
  [
    "a settled change has no bar and no wash",
    bent((s) => (s.changes[0].settled = true)),
    replace(replace(UNSEEN, "bar 5:1-6:68"), "wash 5:1-6:68"),
  ],
  [
    "a seen change keeps its bar and loses its wash",
    bent((s) => (s.changes[0].seen = true)),
    replace(UNSEEN, "wash 5:1-6:68"),
  ],
  [
    "your own change is a solid bar in your colour",
    bent((s) => {
      s.changes[0].attr = "human";
      s.changes[0].by = { type: "human", via: "browser" };
    }),
    replace(UNSEEN, "bar 5:1-6:68", bar("5:1-6:68", "solid", "you")),
  ],
  [
    "a change to your edit is a double rule",
    bent((s) => (s.changes[1].changes_your_edit = "3.0")),
    replace(UNSEEN, "bar 11:1-11:44", bar("11:1-11:44", "double")),
  ],
  [
    "a change that reverts your edit is a double rule",
    bent((s) => (s.changes[1].reverts_your_edit = true)),
    replace(UNSEEN, "bar 11:1-11:44", bar("11:1-11:44", "double")),
  ],
  [
    "a change made outside birta is dotted grey",
    bent((s) => {
      s.changes[1].attr = "outside";
      s.changes[1].asked = { kind: "outside", notes: [] };
    }),
    replace(UNSEEN, "bar 11:1-11:44", bar("11:1-11:44", "dotted", "agent")),
  ],
  [
    "a change a reply cites is solid",
    bent((s) => (s.changes[1].asked = { kind: "cited", notes: [6] })),
    replace(UNSEEN, "bar 11:1-11:44", bar("11:1-11:44", "solid", "agent")),
  ],
  [
    "a change whose text is gone has no bar",
    bent((s) => delete s.changes[1].range.sp),
    replace(replace(UNSEEN, "bar 11:1-11:44"), "wash 11:1-11:44"),
  ],
  [
    "an unseen change's bar wins over a newer seen one on the same block",
    bent((s) => {
      s.changes[1].range.sp = "5:1-6:68";
      s.changes[1].seen = true;
    }),
    replace(replace(UNSEEN, "bar 11:1-11:44"), "wash 11:1-11:44"),
  ],
  [
    "when every change on a block is seen the newest one's bar wins",
    bent((s) => {
      s.changes[1].range.sp = "5:1-6:68";
      s.changes[0].seen = true;
      s.changes[1].seen = true;
    }),
    replace(
      replace(replace(replace(UNSEEN, "bar 11:1-11:44"), "wash 11:1-11:44"), "wash 5:1-6:68"),
      "bar 5:1-6:68",
      bar("5:1-6:68", "dotted", "agent"),
    ),
  ],
  [
    "a block removed at the top leaves a tick above the first block",
    bent((s) => (s.deletions[1].after_sp = null)),
    replace(UNSEEN, "tick 14:1-14:13", tick(null)),
  ],
];

for (const [label, s, want] of CRAFTED) {
  test(`laneMarks: ${label}`, () => {
    const { BR } = load();
    assertMarks(BR.pure.laneMarks(s), want, label);
  });
}

test("laneMarks leaves its state alone", () => {
  const { BR } = load();
  const s = state("unseen");
  const before = JSON.stringify(s);
  BR.pure.laneMarks(s);
  assert.equal(JSON.stringify(s), before);
});

test("laneMarks: the newest change wins in whatever order the list comes", () => {
  const { BR } = load();
  const s = state("lost");
  s.changes.reverse();
  assertMarks(BR.pure.laneMarks(s), FIXTURE_MARKS.lost, "lost, reversed");
});

test("laneMarks: a note with no verdict draws no glyph", () => {
  const { BR } = load();
  const s = state("drafts");
  s.items[1].verdict = null;
  assertMarks(BR.pure.laneMarks(s), [glyph("5:1-6:54", "no", "you")], "no verdict");
});

test("isHost does not trust a birta-* name alone", () => {
  const { BR } = load();
  assert.equal(BR.isHost({ nodeType: 1, localName: "birta-flow" }), false);
  assert.equal(BR.isHost({ nodeType: 1, localName: "birta-over" }), false);
});

test("laneMarks: a retracted item draws no glyph", () => {
  const { BR } = load();
  const s = state("sent");
  s.items[1].status = "retracted";
  assertMarks(BR.pure.laneMarks(s), [glyph("5:1-6:54", "no", "agent")], "retracted");
});

test("laneMarks: a withdrawn item draws as a draft", () => {
  const { BR } = load();
  const s = state("sent");
  s.items[1].status = "withdrawn";
  assertMarks(BR.pure.laneMarks(s), [glyph("5:1-6:54", "no", "agent"), glyph("10:1-10:16", "no", "you")], "withdrawn");
});

test("laneMarks: a rewrite keeps only the glyphs, ticks included", () => {
  const { BR } = load();
  const s = state("unseen");
  s.rewrite = state("rewrite").rewrite;
  assertMarks(BR.pure.laneMarks(s), [glyph("5:1-6:68", "no", "agent"), glyph("10:1-10:16", "no", "agent")], "rewrite");
});

test("laneMarks: glyphs on one block come in note order", () => {
  const { BR } = load();
  const s = state("sent");
  s.items[0].at.loc = { sp: "10:1-10:16" };
  s.items[0].verdict = "ask";
  s.items.reverse();
  const got = JSON.parse(JSON.stringify(BR.pure.laneMarks(s))).filter((m) => m.mark === "glyph");
  assert.deepEqual(
    got.map((m) => [m.sp, m.verdict, m.n]),
    [
      ["10:1-10:16", "ask", 4],
      ["10:1-10:16", "no", 5],
    ],
  );
});

test("laneMarks: two removed blocks in one gap draw one tick with a count", () => {
  const { BR } = load();
  const s = state("unseen");
  s.deletions.push({ ...s.deletions[1], change: "11.1" });
  const ticks = JSON.parse(JSON.stringify(BR.pure.laneMarks(s))).filter((m) => m.mark === "tick");
  assert.deepEqual(ticks, [{ mark: "tick", after: "14:1-14:13", count: 2 }]);
});

test("laneMarks: a settled change's removed block leaves no tick", () => {
  const { BR } = load();
  const s = state("unseen");
  s.changes[2].settled = true;
  assertMarks(BR.pure.laneMarks(s), replace(UNSEEN, "tick 14:1-14:13"), "settled removal");
});

test("laneMarks: held verdicts turn grey when the hold passes, by the clock it is given", () => {
  const { BR } = load();
  const s = state("hold");
  const at = (now) => JSON.parse(JSON.stringify(BR.pure.laneMarks(s, { now }))).filter((m) => m.mark === "glyph").map((m) => m.role);
  assert.deepEqual(at(s.hold.not_before - 1), ["you", "you"]);
  assert.deepEqual(at(s.hold.not_before), ["agent", "agent"]);
  // With no clock given, the state's own server time decides
  assert.deepEqual(at(undefined), ["you", "you"]);
});

test("laneMarks: a held approve leaves sent verdicts grey", () => {
  const { BR } = load();
  const s = state("approved");
  const roles = JSON.parse(JSON.stringify(BR.pure.laneMarks(s, { now: s.hold.not_before - 1 })))
    .filter((m) => m.mark === "glyph")
    .map((m) => m.role);
  assert.deepEqual(roles, ["agent", "agent", "agent"]);
});

test("laneMarks: a later verdict still in your drafts keeps your colour", () => {
  const { BR } = load();
  const s = state("drafts");
  s.items.find((i) => i.at.loc?.sp === "5:1-6:54").verdict = "later";
  const g = JSON.parse(JSON.stringify(BR.pure.laneMarks(s))).filter((m) => m.mark === "glyph" && m.sp === "5:1-6:54");
  assert.deepEqual(g.map((m) => [m.verdict, m.role]), [["later", "you"]]);
});
