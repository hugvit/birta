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
  // The phrase never counts your notes or decided blocks; the button counts notes
  drafts: same("drafted by Claude · your turn"),
  dir: same("read-only · notes only"),
  // Drafting after a reply keeps the reply and its one count, to check
  flow: same("Claude replied 14:13 · 9 to check"),
  hold: same("drafted by Claude · your turn"),
  sent: same("sent 14:13 · not picked up yet"),
  picked: same("Claude picked up 14:13"),
  // One event and one count: the reply's own tallies are on its rows, not in the phrase or its tooltip
  replied: same("Claude replied 14:13 · 3 to check"),
  unseen: same("Claude replied 14:13 · 5 to check"),
  rewrite: same("Claude replied 14:13 · 9 to check"),
  lost: same("Claude replied 14:13 · 3 to check"),
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
    "a reply names its time and what is left to check, never notes or decided blocks",
    bent("flow", (s) => {
      allSeen(s);
      Object.assign(s.counts, { unseen: 2, drafts: 1, decided: 1, items: 7, to_check: 2 });
      Object.assign(s.phrase.replied, { ts: AT_2325, done: 0, answered: 0, asked: 0, unasked: 0, not_handled: 0 });
    }),
    same("Claude replied 23:25 · 2 to check"),
  ],
  [
    "notes are never counted in the phrase",
    bent("drafts", (s) => (s.counts.decided = 0)),
    same("drafted by Claude · your turn"),
  ],
  ["one note is not counted either", bent("drafts", (s) => Object.assign(s.counts, { drafts: 1, decided: 0 })), same("drafted by Claude · your turn")],
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
    "a reply's own tallies never show: with nothing left to check it says all checked",
    bent("replied", (s) => {
      allSeen(s);
      Object.assign(s.counts, { decided: 0, to_check: 0 });
      Object.assign(s.phrase.replied, { done: 3, answered: 1, asked: 2, unasked: 1, not_handled: 2 });
    }),
    same("Claude replied 14:13 · all checked"),
  ],
  [
    "a reply with no tallies still carries its one count",
    bent("replied", (s) => {
      allSeen(s);
      s.counts.decided = 0;
      Object.assign(s.phrase.replied, { done: 0, answered: 0, asked: 0, unasked: 0, not_handled: 0 });
    }),
    same("Claude replied 14:13 · 3 to check"),
  ],
  [
    "a verdict and a note after a reply leave the phrase as the reply left it",
    bent("replied", (s) => {
      allSeen(s);
      Object.assign(s.counts, { drafts: 1, decided: 1 });
    }),
    same("Claude replied 14:13 · 3 to check"),
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

test("the phrase shows at most one count, and the tooltip holds every part of it in order", () => {
  const BR = load();
  for (const [name, s] of [...Object.keys(FIXTURE_PHRASE).map((n) => [n, state(n)]), ...CRAFTED.map(([n, s]) => [n, s])]) {
    const { phrase: p, title } = phrase(BR, s);
    const counts = p.split(" · ").filter((part) => /^\d/.test(part));
    assert.ok(counts.length <= 1, `${name}: ${p}`);
    assert.doesNotMatch(p, /unseen|decided|\bdone\b|answered|for you|without a note/, `${name}: ${p}`);
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

test("an earlier round's settled change keeps a 1 px bar; this round's are 3 px, fresh until checked", () => {
  const BR = load();
  assert.deepEqual(bars(BR, rounds()), [
    { sp: "5:1-6:68", weight: 1, fresh: false },
    { sp: "11:1-11:44", weight: 3, fresh: true },
  ]);
  // Seen is kept: the bar thins and mutes
  assert.deepEqual(bars(BR, rounds((s) => Object.assign(s.changes[1], { seen: true, check: "kept" }))), [
    { sp: "5:1-6:68", weight: 1, fresh: false },
    { sp: "11:1-11:44", weight: 1, fresh: false },
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

test("a round with no changes of its own leaves every bar thin, and every settled removal keeps its tick", () => {
  const BR = load();
  const s = bent("unseen", (s) => {
    for (const c of s.changes) Object.assign(c, { seen: true, settled: true, check: "kept" });
    // Settled news leaves the list
    const ids = new Set(s.changes.map((c) => c.id));
    s.news = s.news.filter((n) => !ids.has(n.id));
    s.rounds.push({ ...s.rounds[0], submit: 20, claim: 21, yield: 25, items: [] });
  });
  assert.deepEqual(bars(BR, s), [
    { sp: "5:1-6:68", weight: 1, fresh: false },
    { sp: "11:1-11:44", weight: 1, fresh: false },
  ]);
  const marks = plain(BR.pure.laneMarks(s, { now: 0 }));
  assert.deepEqual(marks.filter((m) => m.mark === "tick" || m.mark === "wash"), [{ mark: "tick", after: "14:1-14:13", count: 1 }]);
  assert.deepEqual(plain(BR.pure.attention(s)).filter((x) => x.kind === "change"), []);
});

test("an outside change's weight follows settled too, and a changed-since approve keeps its words", () => {
  const BR = load();
  const s = rounds((s) => Object.assign(s.changes[1], { attr: "outside", turn: null, seen: true, settled: true }));
  assert.deepEqual(bars(BR, s).map((b) => b.weight), [1, 1]);
  const a = bent("approved", (s) => (s.approve.changed_since = true));
  assert.equal(phrase(BR, a).phrase, "approved 14:13 · changed since");
  // The tooltip is the phrase plus what waits for later
  assert.equal(phrase(BR, a).title, "approved 14:13 · changed since · 1 later");
});

test("a decided block of one is never counted in the phrase", () => {
  const BR = load();
  const s = bent("drafts", (s) => Object.assign(s.counts, { drafts: 1, decided: 1, items: 1 }));
  assert.equal(BR.pure.formatPhrase(s, { now: s.server_now }), "drafted by Claude · your turn");
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

// --- A change row's substance: textDiff and inline -------------------------------------------

// The journey's first write, as `changes[].old_text`/`new_text` carry it
const WHY_OLD =
  "Uploads to the storage service occasionally fail with transient errors —\nconnection resets, 502/503s from the load balancer, and the odd timeout during\ndeploys. Today, when that happens, we just fail: the error bubbles up to the\ncaller and the user has to start the upload over. Most of these failures would\nsucceed on a second attempt a few hundred milliseconds later.\n";
const WHY_NEW =
  "`UploadClient.put` makes a single attempt with no retry logic, so on transient\nerrors (connection resets, 502/503s, deploy timeouts) we just fail and the user\nrestarts the upload. Most would succeed moments later.\n";
const MERGED_OLD =
  "- Wrap `UploadClient.put` in a retry loop with exponential backoff and full jitter\n- Retry only on idempotent failure modes: network errors, 502, 503, 504, and 429\n";
const MERGED_NEW = "- Retry on network errors, 429 and 502–504 with exponential backoff and jitter\n";

const squash = (s) => s.replace(/\s+/g, " ").trim();
const wordsOf = (s) => squash(s).split(" ").filter(Boolean);
// A list item's marker, and a task box, are the row's to draw
const unmarked = (s) => squash(s.replace(/^\s*[-*+]\s+(\[[ xX]\]\s+)?/gm, ""));

/** Every part of `textDiff(old, new)`, block by block, as plain `{kind, text}`. */
function diffParts(BR, before, after) {
  const blocks = plain(BR.pure.textDiff(before, after));
  assert.ok(Array.isArray(blocks) && blocks.length > 0, JSON.stringify(blocks));
  for (const b of blocks) assert.ok(Array.isArray(b.parts), JSON.stringify(b));
  const parts = blocks.flatMap((b) => b.parts);
  for (const p of parts) {
    assert.ok(["same", "cut", "add", "gap"].includes(p.kind), JSON.stringify(p));
    assert.equal(typeof p.text, "string", JSON.stringify(p));
  }
  return parts;
}

const textsOf = (parts, ...kinds) => parts.filter((p) => kinds.includes(p.kind)).map((p) => p.text);

/** Nothing is lost or made up: the old words are the same and cut parts, the new the same and added. */
function assertCovers(parts, before, after) {
  assert.deepEqual(wordsOf(textsOf(parts, "same", "cut").join(" ")), wordsOf(before));
  assert.deepEqual(wordsOf(textsOf(parts, "same", "add").join(" ")), wordsOf(after));
}

test("textDiff on the Why paragraph: the new UploadClient.put sentence is one add among the cuts", () => {
  const parts = diffParts(load(), WHY_OLD, WHY_NEW);
  const put = parts.filter((p) => p.text.includes("UploadClient.put"));
  assert.equal(put.length, 1, JSON.stringify(parts));
  assert.equal(put[0].kind, "add");
  assert.equal(
    squash(put[0].text).replace(/`/g, ""),
    "UploadClient.put makes a single attempt with no retry logic, so on transient errors (connection resets, 502/503s, deploy timeouts) we just fail and the user restarts the upload.",
  );
  // The trimmed sentences are struck whole, so the new one reads apart from them
  for (const sentence of [
    "Uploads to the storage service occasionally fail with transient errors — connection resets, 502/503s from the load balancer, and the odd timeout during deploys.",
    "Today, when that happens, we just fail: the error bubbles up to the caller and the user has to start the upload over.",
  ]) {
    assert.ok(textsOf(parts, "cut").some((t) => squash(t).includes(sentence)), `struck whole: ${sentence}`);
  }
  assert.ok(!textsOf(parts, "cut").some((t) => t.includes("makes a single attempt")));
  assert.ok(!textsOf(parts, "add").some((t) => t.includes("Uploads to the storage")));
  // Every sentence changed, so nothing collapses
  assert.deepEqual(textsOf(parts, "gap"), []);
  assertCovers(parts, WHY_OLD, WHY_NEW);
});

test("textDiff on the merged bullets: the two old items cut, then the new one added", () => {
  const parts = diffParts(load(), MERGED_OLD, MERGED_NEW);
  assert.deepEqual(
    parts.map((p) => [p.kind, unmarked(p.text)]),
    [
      ["cut", "Wrap `UploadClient.put` in a retry loop with exponential backoff and full jitter"],
      ["cut", "Retry only on idempotent failure modes: network errors, 502, 503, 504, and 429"],
      ["add", "Retry on network errors, 429 and 502–504 with exponential backoff and jitter"],
    ],
  );
});

test("textDiff marks just the words of a sentence that changed in a few", () => {
  const before = "The retry loop waits 200 ms between attempts.\n";
  const after = "The retry loop waits 500 ms between attempts.\n";
  const parts = diffParts(load(), before, after);
  assert.deepEqual(textsOf(parts, "cut").map(squash), ["200"]);
  assert.deepEqual(textsOf(parts, "add").map(squash), ["500"]);
  assert.deepEqual(textsOf(parts, "gap"), []);
  assertCovers(parts, before, after);
});

test("textDiff collapses unchanged sentences to …", () => {
  const before = "First stays put. Second goes away entirely now. Third stays as well.\n";
  const after = "First stays put. Totally different words appear here instead. Third stays as well.\n";
  const parts = diffParts(load(), before, after);
  assert.ok(textsOf(parts, "cut").some((t) => squash(t).includes("Second goes away entirely now.")), JSON.stringify(parts));
  assert.ok(textsOf(parts, "add").some((t) => squash(t).includes("Totally different words appear here instead.")));
  for (const kept of ["First stays put", "Third stays as well"]) {
    assert.ok(!parts.some((p) => p.text.includes(kept)), `${kept} collapses: ${JSON.stringify(parts)}`);
  }
  const gaps = parts.filter((p) => p.kind === "gap");
  assert.ok(gaps.length > 0);
  for (const g of gaps) assert.equal(g.text, "…");
});

test("textDiff goes item by item: an unchanged item collapses, a reworded one is cut and added", () => {
  const before = "- alpha stays here\n- beta goes away entirely now\n- gamma stays too\n";
  const after = "- alpha stays here\n- totally different words appear instead\n- gamma stays too\n";
  const parts = diffParts(load(), before, after);
  assert.deepEqual(textsOf(parts, "cut").map(unmarked), ["beta goes away entirely now"]);
  assert.deepEqual(textsOf(parts, "add").map(unmarked), ["totally different words appear instead"]);
  for (const kept of ["alpha stays here", "gamma stays too"]) {
    assert.ok(!parts.some((p) => p.text.includes(kept)), `${kept} collapses: ${JSON.stringify(parts)}`);
  }
  assert.ok(parts.some((p) => p.kind === "gap" && p.text === "…"));
});

/**
 * A DOM with just what building nodes needs. Writing HTML into it throws, so a row's text can
 * only ever be text.
 */
function fakeDocument() {
  const refuse = (what) => () => {
    throw new Error(`${what} is not allowed: rows build their nodes`);
  };
  function text(data) {
    return { nodeType: 3, data: String(data), get textContent() { return this.data; } };
  }
  function parent(node) {
    node.childNodes = [];
    node.appendChild = (n) => {
      node.childNodes.push(n);
      return n;
    };
    node.append = (...ns) => {
      for (const n of ns) node.childNodes.push(typeof n === "string" ? text(n) : n);
    };
    Object.defineProperty(node, "textContent", {
      get: () => node.childNodes.map((n) => n.textContent).join(""),
      set: (v) => (node.childNodes = v === "" || v == null ? [] : [text(v)]),
    });
    for (const p of ["innerHTML", "outerHTML"]) Object.defineProperty(node, p, { get: refuse(p), set: refuse(p) });
    node.insertAdjacentHTML = refuse("insertAdjacentHTML");
    return node;
  }
  function element(tag) {
    const attrs = {};
    const classes = new Set();
    return parent({
      nodeType: 1,
      localName: String(tag).toLowerCase(),
      attrs,
      className: "",
      classList: { add: (...c) => c.forEach((x) => classes.add(x)), remove: (...c) => c.forEach((x) => classes.delete(x)), contains: (c) => classes.has(c), toggle: (c) => (classes.has(c) ? (classes.delete(c), false) : (classes.add(c), true)) },
      dataset: {},
      style: {},
      setAttribute: (k, v) => (attrs[k] = String(v)),
      getAttribute: (k) => (k in attrs ? attrs[k] : null),
      removeAttribute: (k) => delete attrs[k],
      hasAttribute: (k) => k in attrs,
    });
  }
  return {
    createElement: element,
    createTextNode: text,
    createDocumentFragment: () => parent({ nodeType: 11 }),
    createRange: refuse("createRange"),
  };
}

/** What `inline` built, as markup: code, em, strong and a (with its href) kept, wrappers dropped. */
function markup(n) {
  if (n == null) return "";
  if (Array.isArray(n)) return n.map(markup).join("");
  if (n.nodeType === 3) return n.data;
  const inner = n.childNodes.map(markup).join("");
  if (n.nodeType === 11 || !["code", "em", "strong", "a"].includes(n.localName)) return inner;
  const href = n.localName === "a" ? (n.getAttribute("href") ?? n.href ?? null) : null;
  return `<${n.localName}${href != null ? ` href="${href}"` : ""}>${inner}</${n.localName}>`;
}

/** Every element `inline` built, by name. */
function tags(n, out = []) {
  if (n == null) return out;
  if (Array.isArray(n)) n.forEach((x) => tags(x, out));
  else if (n.nodeType !== 3) {
    if (n.nodeType === 1) out.push(n.localName);
    n.childNodes.forEach((x) => tags(x, out));
  }
  return out;
}

/** `BR.pure.inline` over the fake DOM, given only once the review files have loaded without one. */
function inliner() {
  const ctx = vm.createContext({});
  for (const f of ORDER) {
    const file = path.join(ASSETS, f);
    vm.runInContext(fs.readFileSync(file, "utf-8"), ctx, { filename: file });
  }
  ctx.document = fakeDocument();
  assert.equal(typeof ctx.BR.pure.inline, "function");
  return { BR: ctx.BR, inline: (md) => ctx.BR.pure.inline(md) };
}

test("inline renders code spans, emphasis and links as nodes", () => {
  const { inline } = inliner();
  assert.equal(
    markup(inline("Wrap `UploadClient.put` in a *retry* loop with **full** jitter")),
    "Wrap <code>UploadClient.put</code> in a <em>retry</em> loop with <strong>full</strong> jitter",
  );
  assert.equal(markup(inline("see [the docs](https://example.com/a?b=1) now")), 'see <a href="https://example.com/a?b=1">the docs</a> now');
  // A code span's text is literal
  assert.equal(markup(inline("`a*b*c` and `[x](y)`")), "<code>a*b*c</code> and <code>[x](y)</code>");
});

test("inline draws a list marker as • and keeps a heading's ##", () => {
  const { inline } = inliner();
  assert.equal(markup(inline("- Respect `Retry-After` when the server sends it")), "• Respect <code>Retry-After</code> when the server sends it");
  assert.equal(markup(inline("## Stacked on")), "## Stacked on");
});

test("inline keeps raw HTML as text and never links a script URL", () => {
  const { inline } = inliner();
  const raw = 'a <script>alert(1)</script> & <b onclick="x()">bold</b>';
  const nodes = inline(raw);
  assert.equal(markup(nodes), raw);
  assert.deepEqual(tags(nodes), []);
  const js = markup(inline("[click](javascript:alert(1))"));
  assert.ok(!/href="\s*javascript:/i.test(js), js);
});

// --- Adversary: textDiff and inline inputs the cases above let through -------------------------

test("textDiff strikes a merge whole even when the new item is close to the first old one", () => {
  // Pairing items by position would mark only `and 429` in the first item
  const before = "- Retry on network errors with exponential backoff\n- Also retry on 429 responses\n";
  const after = "- Retry on network errors and 429 with exponential backoff\n";
  const parts = diffParts(load(), before, after);
  assert.deepEqual(
    parts.map((p) => [p.kind, unmarked(p.text)]),
    [
      ["cut", "Retry on network errors with exponential backoff"],
      ["cut", "Also retry on 429 responses"],
      ["add", "Retry on network errors and 429 with exponential backoff"],
    ],
  );
});

test("textDiff at the 1/3 line: 2 of 6 words marks just them, 3 of 6 strikes the sentence", () => {
  const BR = load();
  const before = "alpha beta gamma delta epsilon zeta.\n";
  const two = diffParts(BR, before, "alpha BETA GAMMA delta epsilon zeta.\n");
  assert.deepEqual(wordsOf(textsOf(two, "cut").join(" ")), ["beta", "gamma"], JSON.stringify(two));
  assert.deepEqual(wordsOf(textsOf(two, "add").join(" ")), ["BETA", "GAMMA"], JSON.stringify(two));
  assertCovers(two, before, "alpha BETA GAMMA delta epsilon zeta.\n");
  const after = "alpha BETA GAMMA DELTA epsilon zeta.\n";
  const three = diffParts(BR, before, after);
  assert.deepEqual(wordsOf(textsOf(three, "cut").join(" ")), wordsOf(before), JSON.stringify(three));
  assert.deepEqual(wordsOf(textsOf(three, "add").join(" ")), wordsOf(after), JSON.stringify(three));
});

test("textDiff ends a sentence at ? and !, so an unchanged question collapses", () => {
  const before = "Is five enough? The cap rides out short deploys! Keep it.\n";
  const after = "Is five enough? Totally different closing words appear instead here! Keep it.\n";
  const parts = diffParts(load(), before, after);
  for (const kept of ["Is five enough", "Keep it"]) {
    assert.ok(!parts.some((p) => p.text.includes(kept)), `${kept} collapses: ${JSON.stringify(parts)}`);
  }
  assert.ok(textsOf(parts, "cut").some((t) => squash(t).includes("The cap rides out short deploys!")), JSON.stringify(parts));
});

test("textDiff never ends a sentence at e.g. or inside a decimal", () => {
  const BR = load();
  const eg = diffParts(BR, "Use a proxy, e.g. Envoy in front, for local runs.\n", "Use a proxy, e.g. nginx in front, for local runs.\n");
  assert.deepEqual(textsOf(eg, "gap"), [], JSON.stringify(eg));
  assert.deepEqual(wordsOf(textsOf(eg, "cut").join(" ")), ["Envoy"], JSON.stringify(eg));
  assert.deepEqual(wordsOf(textsOf(eg, "add").join(" ")), ["nginx"], JSON.stringify(eg));
  const dec = diffParts(BR, "The cap is 2.5 s between tries.\n", "The cap is 3.5 s between tries.\n");
  assert.deepEqual(textsOf(dec, "gap"), [], JSON.stringify(dec));
  assert.deepEqual(wordsOf(textsOf(dec, "cut").join(" ")), ["2.5"], JSON.stringify(dec));
  assert.deepEqual(wordsOf(textsOf(dec, "add").join(" ")), ["3.5"], JSON.stringify(dec));
});

test("textDiff never splits a sentence inside a code span", () => {
  const before = "Run `a. b` now. Keep this sentence here.\n";
  const after = "Execute `a. b` immediately please. Keep this sentence here.\n";
  const parts = diffParts(load(), before, after);
  for (const p of parts.filter((p) => p.kind !== "gap")) {
    assert.equal((p.text.match(/`/g) ?? []).length % 2, 0, `a part cuts a code span in two: ${JSON.stringify(parts)}`);
  }
  assert.ok(textsOf(parts, "cut").some((t) => squash(t).includes("Run `a. b` now.")), JSON.stringify(parts));
  assert.ok(textsOf(parts, "add").some((t) => squash(t).includes("Execute `a. b` immediately please.")), JSON.stringify(parts));
});

test("textDiff keeps non-ASCII words whole", () => {
  const before = "Die Größe ändert sich um 200 ms — naïve café 🚀 bleibt.\n";
  const after = "Die Größe ändert sich um 500 ms — naïve café 🎉 bleibt.\n";
  const parts = diffParts(load(), before, after);
  assert.deepEqual(wordsOf(textsOf(parts, "cut").join(" ")), ["200", "🚀"], JSON.stringify(parts));
  assert.deepEqual(wordsOf(textsOf(parts, "add").join(" ")), ["500", "🎉"], JSON.stringify(parts));
  assertCovers(parts, before, after);
});

test("inline links no script, data or vbscript URL, whatever its case or leading space", () => {
  const { inline } = inliner();
  for (const md of ["[x](JavaScript:void0)", "[x]( javascript:void0)", "[x](JAVASCRIPT:void0)", "[x](vbscript:msgbox)", "[x](data:text/html,hi)"]) {
    const out = markup(inline(md));
    assert.ok(!/href="\s*(javascript|vbscript|data):/i.test(out), `${md} → ${out}`);
  }
});

test("inline still links a relative URL", () => {
  const { inline } = inliner();
  assert.equal(markup(inline("[the plan](docs/plan.md)")), '<a href="docs/plan.md">the plan</a>');
});

// --- Repair: textDiff stays fast, and a list marker always renders ------------------------------

test("textDiff of a 5,000-word rewrite returns well under 200 ms", () => {
  const BR = load();
  const sentence = (k, w) => Array.from({ length: 20 }, (_, i) => `${w}${k}x${i}`).join(" ") + ".";
  const before = Array.from({ length: 250 }, (_, k) => sentence(k, "old")).join(" ") + "\n";
  const after = Array.from({ length: 250 }, (_, k) => sentence(k, "new")).join(" ") + "\n";
  // One unit of 5,000 words with no sentence end, as a table or a long line gives
  const runOn = (w) => Array.from({ length: 5000 }, (_, i) => `${w}${i}`).join(" ") + "\n";
  for (const [a, b] of [
    [before, after],
    [runOn("a"), runOn("b")],
    [runOn("a"), runOn("a").replace("a7 ", "z7 ")],
  ]) {
    const t0 = performance.now();
    const parts = diffParts(BR, a, b);
    const ms = performance.now() - t0;
    assert.ok(ms < 200, `${ms.toFixed(0)} ms`);
    assertCovers(parts, a, b);
  }
});

test("a list item whose first word changed still renders its marker as •", () => {
  const BR = load();
  const { inline } = inliner();
  const parts = diffParts(BR, "- Retry 3 times on network errors\n", "- Repeat 3 times on network errors\n");
  assert.deepEqual(textsOf(parts, "cut").map(squash), ["Retry"], JSON.stringify(parts));
  assert.deepEqual(textsOf(parts, "add").map(squash), ["Repeat"], JSON.stringify(parts));
  // Rendered as a row does: each part on its own, in order
  const shown = parts.map((p) => markup(inline(p.text.replace(/\s+/g, " ").replace(/^ /, "")))).join(" ");
  assert.match(shown, /^• /, shown);
  assert.ok(!/(^|\s)-(\s|$)/.test(shown), shown);
});
