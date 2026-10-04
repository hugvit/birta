import { Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  GATE_ON,
  Mock,
  State,
  booted,
  changeRows,
  fixtureDoc,
  fixtureState,
  focusMark,
  focusTo,
  focused,
  marks,
  mockReview,
  revOf,
  reviewLog,
  stripKeys,
  stripMessage,
  stripWhere,
} from "./harness";

// u on a reverted change restores Claude's text, u again reverts it again, and z takes either back
const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;

const REVERTED = "reverted · u restores Claude's text";
const RESTORED = "restored Claude's text · u reverts again";
const REVERT_UNDONE = "revert undone · Claude's text is back";
const RESTORE_UNDONE = "restore undone · reverted again";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

interface W {
  BR: { currentState(): State };
}

// --- on a real server: the file's bytes and the log after every press ------------------------------

const BASE = [
  "# Upload retries",
  "",
  "## Why",
  "",
  "alpha one",
  "beta two",
  "gamma three",
  "",
  "## What",
  "",
  "- Wrap the put in a retry loop",
  "- Retry only on idempotent failures",
  "",
  "## Testing",
  "",
  "- [ ] Ran the suite",
  "",
];
const MERGED_LINE = "- Retry on network errors, 429 and 502–504";
const human = { type: "human", via: "browser" };
const agent = { type: "agent", name: "claude", via: "cli" };

interface Hunk {
  a: number;
  d: string[];
  i: string[];
}

/** `lines` with `hunks` applied, each at its line in `lines`. */
function applied(lines: string[], hunks: Hunk[]): string[] {
  const out = [...lines];
  for (const h of [...hunks].reverse()) out.splice(h.a, h.d.length, ...h.i.map((l) => l.replace(/\n$/, "")));
  return out;
}

/** Claude merges the two What bullets into one: one change. */
const MERGE: Hunk[] = [{ a: 10, d: [BASE[10] + "\n", BASE[11] + "\n"], i: [MERGED_LINE + "\n"] }];
/** Claude shouts two lines of one paragraph: two changes on one stop. */
const SHOUT: Hunk[] = [
  { a: 4, d: ["alpha one\n"], i: ["ALPHA one\n"] },
  { a: 6, d: ["gamma three\n"], i: ["GAMMA three\n"] },
];

/** Undoes `hunks`, a write of `base`: in the written text's lines. */
function reversed(hunks: Hunk[]): Hunk[] {
  let shift = 0;
  return hunks.map((h) => {
    const r = { a: h.a + shift, d: h.i, i: h.d };
    shift += h.i.length - h.d.length;
    return r;
  });
}

/**
 * A round in which Claude wrote `hunks` over BASE and replied, then `after` lines (each
 * given its own text). Line 7 is Claude's write, so its changes are `7.0`, `7.1`.
 */
function round(hunks: Hunk[], after: (ctx: { base: string; theirs: string }) => { lines: State[]; text: string } = (c) => ({ lines: [], text: c.theirs })) {
  const T0 = Date.now() - 600_000;
  const base = BASE.join("\n");
  const theirs = applied(BASE, hunks).join("\n");
  const lines: State[] = [
    { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
    { by: { type: "birta" }, op: "snap", rev: revOf(base), text: base },
    { by: { type: "birta" }, op: "owner", drafted_by: null },
    { by: human, cid: "c-4", op: "comment", at: "doc", body: "tighten it" },
    { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
    { by: agent, op: "claim", round: 5 },
    { by: { type: "birta" }, op: "external", rev: revOf(base), to_rev: revOf(theirs), patch: hunks },
    { by: agent, op: "done", to: 4, turn: 6 },
    { by: agent, op: "yield", turn: 6 },
  ];
  const more = after({ base, theirs });
  const all = [...lines, ...more.lines];
  return {
    base,
    theirs,
    doc: more.text,
    log: all.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join(""),
  };
}

/** Lines after the reply: your undo of each of `of` in turn, flipping the text, then a whole round. */
function settledAfter(ofs: string[]) {
  return ({ base, theirs }: { base: string; theirs: string }) => {
    const lines: State[] = [];
    let [from, to] = [theirs, base];
    let hunks = reversed(MERGE);
    for (const of of ofs) {
      lines.push({ by: human, op: "edit", act: "undo", of, rev: revOf(from), to_rev: revOf(to), patch: hunks });
      [from, to] = [to, from];
      hunks = reversed(hunks);
    }
    const note = 10 + ofs.length;
    lines.push(
      { by: human, cid: "c-note", op: "comment", at: "doc", body: "thanks" },
      { by: human, op: "submit", upto: note, not_before: Date.now() - 300_000, hold_ms: 0, to: "*" },
      { by: agent, op: "claim", round: note + 1 },
      { by: agent, op: "done", to: note, turn: note + 2 },
      { by: agent, op: "yield", turn: note + 2 },
    );
    return { lines, text: from };
  };
}

async function openReal(page: Page, birta: Start, s: { doc: string; log: string }, args?: string[], extra: Record<string, string> = {}) {
  const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log, ...extra }, GATE_ON, args);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
  return served;
}

const disk = (root: string) => fs.readFileSync(path.join(root, "pr.md"), "utf-8");
const edits = (root: string) => (reviewLog(root) ?? []).map((l, i) => ({ ...l, n: i + 1 })).filter((l) => l.op === "edit");

/** The focused block's text, its whitespace collapsed. */
async function focusText(page: Page): Promise<string> {
  const sp = await focused(page);
  if (!sp || !/^\d+:/.test(sp)) return "";
  return ((await page.locator(`#content [data-sourcepos="${sp}"]`).first().innerText()) ?? "").replace(/\s+/g, " ").trim();
}

/** The sp of the innermost block on the page now whose text holds `text`, once there is one. */
async function spNow(page: Page, text: string): Promise<string> {
  const find = () =>
    page.evaluate((text) => {
      let hit: string | null = null;
      document.querySelectorAll("#content [data-sourcepos]").forEach((el) => {
        if ((el.textContent ?? "").includes(text)) hit = el.getAttribute("data-sourcepos");
      });
      return hit;
    }, text);
  await expect.poll(find, { message: `a block holds ${JSON.stringify(text)}` }).not.toBeNull();
  return (await find())!;
}

/** Presses `key` until focus is on the removal stop of change `id`. */
async function focusRemoval(page: Page, id: string, key: "j" | "k" = "j") {
  for (let i = 0; i < 40; i++) {
    const m = await focusMark(page);
    if (m?.target === "removal" && m.change === id) return;
    await page.keyboard.press(key);
  }
  throw new Error(`40 presses of ${key} never reached the removal stop of ${id}: ${JSON.stringify(await focusMark(page))}`);
}

async function focusTop(page: Page): Promise<number> {
  return (await focusMark(page))!.top;
}

/** Presses `key` and waits for the file to read `want` and the log to hold `count` edits. */
async function pressAndWrite(page: Page, root: string, key: string, want: string, count: number, label: string) {
  await page.keyboard.press(key);
  await expect.poll(() => disk(root), { message: `${label}: the file's bytes` }).toBe(want);
  await expect.poll(() => edits(root).length, { message: `${label}: one edit line` }).toBe(count);
}

test.describe("restore on a real server", () => {
  test("u u u: reverted, restored, reverted again, each write exact, with focus and its place kept", async ({ page, birta }) => {
    const s = round(MERGE);
    const served = await openReal(page, birta, s);
    await focusTo(page, await spNow(page, "Retry on network errors"));
    const top = await focusTop(page);
    const presses: [string, string, string][] = [
      [REVERTED, s.base, "Wrap the put in a retry loop"],
      [RESTORED, s.theirs, "Retry on network errors"],
      [REVERTED, s.base, "Wrap the put in a retry loop"],
    ];
    let flip = "7.0";
    for (const [i, [message, file, first]] of presses.entries()) {
      const label = `u ×${i + 1}`;
      await pressAndWrite(page, served.root, "u", file, i + 1, label);
      const last = edits(served.root).at(-1)!;
      expect(last, label).toMatchObject({ act: "undo", of: flip, to_rev: revOf(file), by: { type: "human" } });
      flip = `${last.n}.0`;
      await expect.poll(() => stripMessage(page), { message: label }).toBe(message);
      await expect.poll(() => focusText(page), { message: `${label}: focus stays on the change` }).toContain(first);
      expect(Math.abs((await focusTop(page)) - top), `${label}: focus keeps its place`).toBeLessThanOrEqual(1);
    }
    expect((reviewLog(served.root) ?? []).filter((l) => l.op === "external"), "no write of the page's reads as Claude's").toHaveLength(1);
  });

  test("after a restore the row reads kept, the lane shows a thin checked bar and no ↺, and the strip says kept", async ({ page, birta }) => {
    const s = round(MERGE);
    const served = await openReal(page, birta, s);
    await focusTo(page, await spNow(page, "Retry on network errors"));
    await pressAndWrite(page, served.root, "u", s.base, 1, "revert");
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "revert").length).toBeGreaterThan(0);
    // Wherever the write left focus, the reverted change's own stop shows the reverted keys
    await focusTo(page, await spNow(page, "Wrap the put in a retry loop"));
    await expect.poll(() => stripKeys(page), { timeout: 8_000 }).toBe("reverted · u restore · c reply · n next");
    await pressAndWrite(page, served.root, "u", s.theirs, 2, "restore");
    const sp = await spNow(page, "Retry on network errors");
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "revert").map((m) => m.sp)).toEqual([]);
    await expect.poll(async () => (await marks(page)).find((m) => m.mark === "bar" && m.sp === sp)?.check).toBe("checked");
    await expect.poll(async () => (await changeRows(page)).map((r) => r.tail)).toContain("kept");
    // Messages last 4 s; the keys come back after
    await expect.poll(() => stripKeys(page), { timeout: 8_000 }).toBe("kept · u revert · c reply · n next");
    await expect.poll(() => stripWhere(page)).toMatch(/· ## What › list item · Claude's change$/);
  });

  test("z after u restores; z after u u reverts again; each write exact", async ({ page, birta }) => {
    const s = round(MERGE);
    const served = await openReal(page, birta, s);
    await focusTo(page, await spNow(page, "Retry on network errors"));
    await pressAndWrite(page, served.root, "u", s.base, 1, "u");
    await pressAndWrite(page, served.root, "z", s.theirs, 2, "z");
    expect(edits(served.root).at(-1)).toMatchObject({ act: "undo", of: `${edits(served.root)[0].n}.0` });
    await expect.poll(() => stripMessage(page)).toBe(REVERT_UNDONE);

    await pressAndWrite(page, served.root, "u", s.base, 3, "u again");
    await pressAndWrite(page, served.root, "u", s.theirs, 4, "u restores");
    await pressAndWrite(page, served.root, "z", s.base, 5, "z takes the restore back");
    expect(edits(served.root).at(-1)).toMatchObject({ act: "undo", of: `${edits(served.root)[3].n}.0` });
    await expect.poll(() => stripMessage(page)).toBe(RESTORE_UNDONE);
  });

  test("a reload between: u then u restores from the state's flip", async ({ page, birta }) => {
    const s = round(MERGE);
    const served = await openReal(page, birta, s);
    await focusTo(page, await spNow(page, "Retry on network errors"));
    await pressAndWrite(page, served.root, "u", s.base, 1, "revert");
    await page.reload();
    await booted(page);
    await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
    await focusTo(page, await spNow(page, "Wrap the put in a retry loop"));
    await pressAndWrite(page, served.root, "u", s.theirs, 2, "restore after the reload");
    expect(edits(served.root).at(-1)).toMatchObject({ act: "undo", of: `${edits(served.root)[0].n}.0` });
    await expect.poll(() => stripMessage(page)).toBe(RESTORED);
  });

  test("a reload between: u then z takes the revert back", async ({ page, birta }) => {
    const s = round(MERGE);
    const served = await openReal(page, birta, s);
    await focusTo(page, await spNow(page, "Retry on network errors"));
    await pressAndWrite(page, served.root, "u", s.base, 1, "revert");
    await page.reload();
    await booted(page);
    await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
    await pressAndWrite(page, served.root, "z", s.theirs, 2, "z after the reload");
    expect(edits(served.root).at(-1)).toMatchObject({ act: "undo", of: `${edits(served.root)[0].n}.0` });
    await expect.poll(() => stripMessage(page)).toBe(REVERT_UNDONE);
  });

  for (const [what, ofs] of [
    ["a revert", ["7.0"]],
    ["a restore", ["7.0", "10.0"]],
  ] as const) {
    test(`u on ${what} a hand-back settled says nothing here to revert and writes nothing`, async ({ page, birta }) => {
      const s = round(MERGE, settledAfter([...ofs]));
      const served = await openReal(page, birta, s);
      const state = await page.evaluate(() => (window as unknown as W).BR.currentState());
      expect(state.news.map((n: State) => n.id), "setup: settled, so not news").not.toContain("7.0");
      await focusTo(page, await spNow(page, ofs.length % 2 ? "Wrap the put in a retry loop" : "Retry on network errors"));
      const before = reviewLog(served.root)!.length;
      await page.keyboard.press("u");
      await expect.poll(() => stripMessage(page)).toBe("nothing here to revert");
      await page.waitForTimeout(SETTLE_MS);
      expect(disk(served.root)).toBe(s.doc);
      expect(reviewLog(served.root)!.length).toBe(before);
    });
  }

  test("two changes on one stop: u reverts both, u restores both, u reverts both again, each exact", async ({ page, birta }) => {
    const s = round(SHOUT);
    const served = await openReal(page, birta, s);
    await focusTo(page, await spNow(page, "ALPHA one"));
    await pressAndWrite(page, served.root, "u", s.base, 2, "revert both");
    const reverts = edits(served.root).map((e) => e.n);
    await pressAndWrite(page, served.root, "u", s.theirs, 4, "restore both");
    expect(
      edits(served.root)
        .slice(2)
        .map((e) => e.of)
        .sort(),
    ).toEqual(reverts.map((n) => `${n}.0`).sort());
    await pressAndWrite(page, served.root, "u", s.base, 6, "revert both again");
  });

  test("a cut: u puts it back, u on the put-back block cuts it again, u on the gap puts it back, each exact", async ({ page, birta }) => {
    const s = round([{ a: 11, d: [BASE[11] + "\n"], i: [] }]);
    const served = await openReal(page, birta, s);
    await focusRemoval(page, "7.0");
    await pressAndWrite(page, served.root, "u", s.base, 1, "put back");
    expect(edits(served.root)[0]).toMatchObject({ act: "undo", of: "7.0" });
    await focusTo(page, await spNow(page, "Retry only on idempotent failures"));
    await pressAndWrite(page, served.root, "u", s.theirs, 2, "cut again");
    expect(edits(served.root)[1]).toMatchObject({ act: "undo", of: `${edits(served.root)[0].n}.0` });
    // The gap is a stop only once the state of the cut reaches the page
    await expect
      .poll(() => page.evaluate(() => (window as unknown as W).BR.currentState()?.news.find((n: State) => n.id === "7.0")?.kind))
      .toBe("removal");
    await focusRemoval(page, "7.0", "k");
    await pressAndWrite(page, served.root, "u", s.base, 3, "put back again");
    expect(edits(served.root)[2]).toMatchObject({ act: "undo", of: `${edits(served.root)[1].n}.0` });
  });

  test("in folder mode u u u writes the doc exactly each time", async ({ page, birta }) => {
    const s = round(MERGE);
    const served = await openReal(page, birta, s, ["."], { "b.md": "# B\n\nOther doc.\n" });
    await focusTo(page, await spNow(page, "Retry on network errors"));
    for (const [i, [file, message]] of [
      [s.base, REVERTED],
      [s.theirs, RESTORED],
      [s.base, REVERTED],
    ].entries()) {
      await pressAndWrite(page, served.root, "u", file, i + 1, `u ×${i + 1}`);
      await expect.poll(() => stripMessage(page)).toBe(message);
    }
  });
});

// --- adversary: the same-turn rule, the hold, an overlapped restore -----------------------------

/** Your undo of 7.0 (the merge), on the line after the reply. */
const revertLine = (base: string, theirs: string): State => ({
  by: human,
  op: "edit",
  act: "undo",
  of: "7.0",
  rev: revOf(theirs),
  to_rev: revOf(base),
  patch: reversed(MERGE),
});

test.describe("adversary: restore and its neighbours", () => {
  test("Claude's own rewrite in one turn stays one change through u u u, each write exact", async ({ page, birta }) => {
    const T0 = Date.now() - 600_000;
    const base = BASE.join("\n");
    const first = applied(BASE, [{ a: 4, d: ["alpha one\n"], i: ["alpha, first try\n"] }]);
    const second = applied(first, [{ a: 4, d: ["alpha, first try\n"], i: ["alpha, second try\n"] }]);
    const lines: State[] = [
      { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
      { by: { type: "birta" }, op: "snap", rev: revOf(base), text: base },
      { by: { type: "birta" }, op: "owner", drafted_by: null },
      { by: human, cid: "c-4", op: "comment", at: "doc", body: "tighten it" },
      { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
      { by: agent, op: "claim", round: 5 },
      { by: { type: "birta" }, op: "external", rev: revOf(base), to_rev: revOf(first.join("\n")), patch: [{ a: 4, d: ["alpha one\n"], i: ["alpha, first try\n"] }] },
      { by: { type: "birta" }, op: "external", rev: revOf(first.join("\n")), to_rev: revOf(second.join("\n")), patch: [{ a: 4, d: ["alpha, first try\n"], i: ["alpha, second try\n"] }] },
      { by: agent, op: "done", to: 4, turn: 6 },
      { by: agent, op: "yield", turn: 6 },
    ];
    const log = lines.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join("");
    const served = await openReal(page, birta, { doc: second.join("\n"), log });
    const sp = await spNow(page, "alpha, second try");
    await focusTo(page, sp);
    const files = [first.join("\n"), second.join("\n"), first.join("\n")];
    for (const [i, file] of files.entries()) {
      await pressAndWrite(page, served.root, "u", file, i + 1, `u ×${i + 1}`);
    }
    expect(edits(served.root)[0].of, "the first u names the standing change").toBe("8.0");
    // Restored, the later change still stands for both: one item of news on the paragraph
    await pressAndWrite(page, served.root, "u", second.join("\n"), 4, "u ×4");
    await expect
      .poll(async () => {
        const st = await page.evaluate(() => (window as unknown as W).BR.currentState());
        return st.news.filter((n: State) => n.kind === "change").map((n: State) => [n.id, n.check]);
      })
      .toEqual([["8.0", "restored"]]);
  });

  test("guard: with Claude, u on your revert says Claude has the doc and writes nothing", async ({ page, birta }) => {
    const s = round(MERGE, ({ base, theirs }) => ({
      lines: [
        revertLine(base, theirs),
        { by: human, cid: "c-11", op: "comment", at: "doc", body: "kept them apart" },
        { by: human, op: "submit", upto: 11, not_before: Date.now() - 300_000, hold_ms: 0, to: "*" },
      ],
      text: base,
    }));
    const served = await openReal(page, birta, s);
    await focusTo(page, await spNow(page, "Wrap the put in a retry loop"));
    const before = reviewLog(served.root)!.length;
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("Claude has the doc · this works again after its reply");
    await page.waitForTimeout(SETTLE_MS);
    expect(disk(served.root)).toBe(s.base);
    expect(reviewLog(served.root)!.length).toBe(before);
  });

  test("a restore a later write overlaps is refused: no restored message, nothing written", async ({ page, birta }) => {
    const over = BASE.map((l) => (l === "- Wrap the put in a retry loop" ? "- Wrap the put in a retry loop, edited" : l)).join("\n");
    const s = round(MERGE, ({ base, theirs }) => ({
      lines: [
        revertLine(base, theirs),
        // Your own later write over the line, so the stop holds no other news
        { by: human, op: "edit", act: "edit", rev: revOf(base), to_rev: revOf(over), patch: [{ a: 10, d: ["- Wrap the put in a retry loop\n"], i: ["- Wrap the put in a retry loop, edited\n"] }] },
      ],
      text: over,
    }));
    const served = await openReal(page, birta, s);
    const state = await page.evaluate(() => (window as unknown as W).BR.currentState());
    expect(state.news.find((n: State) => n.id === "7.0")?.check, "setup: still news, reverted").toBe("reverted");
    expect(state.news.filter((n: State) => n.kind === "change").map((n: State) => n.id), "setup: the only change of Claude's").toEqual(["7.0"]);
    await focusTo(page, await spNow(page, "Wrap the put in a retry loop, edited"));
    const before = reviewLog(served.root)!.length;
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toContain("can't");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).not.toBe(RESTORED);
    expect(disk(served.root)).toBe(over);
    expect(reviewLog(served.root)!.length).toBe(before);
  });
});

// --- with the socket mocked: what u, y and z send, and the strip ----------------------------------

const NAME = "journey/round1";
const DOC = fixtureDoc(NAME);
const MERGED = "11:1-11:80";
const HONOR = "12:1-13:0";

async function open(page: Page, birta: Start, state: State) {
  const served = await birta({ "pr.md": DOC }, GATE_ON);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
  return { served, mock, state };
}

const changeOf = (s: State, id: string) => s.changes.find((c: State) => c.id === id);
const newsOf = (s: State, id: string) => s.news.find((n: State) => n.id === id);
const undos = (mock: Mock) => mock.ops().filter((o) => o.op === "undo_change").map((o) => o.change);
const seens = (mock: Mock) => mock.ops().filter((o) => o.op === "seen");

/** The fold's count after `edit`: the stops still holding unchecked news. */
function recount(s: State) {
  const stopOf = (n: State) => (n.kind === "removal" ? `r:${n.id}` : n.kind === "question" ? `q:${n.id}` : n.sp ? `b:${n.sp}` : "doc");
  s.counts.to_check = new Set(s.news.filter((n: State) => n.check == null).map(stopOf)).size;
  s.counts.attention = s.counts.to_check;
  return s;
}

/** Your undo write `n` of change `id`, as its own change on `id`'s block. */
function yourUndo(s: State, id: string, n: number) {
  const c = changeOf(s, id);
  s.changes.push({
    ...JSON.parse(JSON.stringify(c)),
    id: `${n}.0`,
    op: n,
    idx: 0,
    turn: null,
    attr: "human",
    by: { type: "human", via: "browser" },
    asked: { kind: "location", notes: [] },
    undone_by: null,
    check: null,
    settled: false,
    seen: false,
    flip: undefined,
    reversible: { ok: true, reason: "none" },
  });
}

/** Change `id` reverted by your write `n`, as the fold shows it. */
function reverted(s: State, id: string, n: number) {
  Object.assign(changeOf(s, id), { undone_by: n, check: "reverted", flip: `${n}.0`, reversible: { ok: false, reason: "later_overlap" } });
  newsOf(s, id).check = "reverted";
  yourUndo(s, id, n);
  return recount(s);
}

/** Change `id` restored by your write `n`: Claude's again and checked, the undo pair gone from `changes`. */
function restored(s: State, id: string, n: number, reversible = { ok: true, reason: "none" }) {
  Object.assign(changeOf(s, id), { undone_by: null, check: "restored", flip: `${n}.0`, reversible });
  newsOf(s, id).check = "restored";
  return recount(s);
}

const fresh = () => fixtureState(NAME);

test.describe("restore with the socket mocked", () => {
  for (const reversible of [
    { ok: true, reason: "none" },
    { ok: false, reason: "later_overlap" },
  ]) {
    test(`u on a restored change sends the flip, not the change's own id (reversible ${reversible.ok})`, async ({ page, birta }) => {
      const { mock } = await open(page, birta, restored(fresh(), "10.1", 24, reversible));
      await focusTo(page, MERGED);
      await page.keyboard.press("u");
      await expect.poll(() => undos(mock)).toEqual(["24.0"]);
      await expect.poll(() => stripMessage(page)).toBe(REVERTED);
      expect(seens(mock)).toEqual([]);
    });
  }

  test("the flip is read from the latest state: another tab's toggle moves it", async ({ page, birta }) => {
    const { mock, state } = await open(page, birta, reverted(fresh(), "10.1", 23));
    await focusTo(page, MERGED);
    // Another tab restored (24) and reverted again (25)
    const again = JSON.parse(JSON.stringify(state));
    again.changes = again.changes.filter((c: State) => c.id !== "23.0");
    Object.assign(changeOf(again, "10.1"), { undone_by: 25, flip: "25.0" });
    yourUndo(again, "10.1", 25);
    mock.pushState(again);
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().changes.find((c: State) => c.id === "10.1").flip)).toBe("25.0");
    await page.keyboard.press("u");
    await expect.poll(() => undos(mock)).toEqual(["25.0"]);
  });

  test("y on a restored change sends nothing and says it is kept", async ({ page, birta }) => {
    const { mock } = await open(page, birta, restored(fresh(), "10.1", 24));
    await focusTo(page, MERGED);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe("already kept · u reverts it");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("the strip: reverted says u restore, restored reads as kept", async ({ page, birta }) => {
    await open(page, birta, reverted(fresh(), "10.1", 23));
    await focusTo(page, MERGED);
    await expect.poll(() => stripWhere(page)).toBe("3 of 9 · ## What › list item · Claude's change");
    await expect.poll(() => stripKeys(page)).toBe("reverted · u restore · c reply · n next");
  });

  test("guard: a restored change's row tail is kept, its bar thin and checked, with no ↺", async ({ page, birta }) => {
    await open(page, birta, restored(fresh(), "10.1", 24));
    await focusTo(page, MERGED);
    await expect.poll(() => stripKeys(page)).toBe("kept · u revert · c reply · n next");
    await expect.poll(async () => (await marks(page)).find((m) => m.mark === "bar" && m.sp === MERGED)?.check).toBe("checked");
    expect((await marks(page)).filter((m) => m.mark === "revert")).toEqual([]);
    await expect.poll(async () => (await changeRows(page)).map((r) => r.tail)).toContain("kept");
  });

  test("two reverted changes on one stop: u sends each one's flip", async ({ page, birta }) => {
    const s = fresh();
    const c = changeOf(s, "10.3");
    s.changes.push({ ...JSON.parse(JSON.stringify(c)), id: "10.7", idx: 7 });
    s.news.splice(s.news.findIndex((n: State) => n.id === "10.3") + 1, 0, { id: "10.7", kind: "change", sp: HONOR, check: null });
    reverted(s, "10.3", 23);
    reverted(s, "10.7", 24);
    const { mock } = await open(page, birta, s);
    await focusTo(page, HONOR);
    await page.keyboard.press("u");
    await expect.poll(() => undos(mock).sort()).toEqual(["23.0", "24.0"]);
  });

  test("two restored changes on one stop: u sends each one's flip", async ({ page, birta }) => {
    const s = fresh();
    const c = changeOf(s, "10.3");
    s.changes.push({ ...JSON.parse(JSON.stringify(c)), id: "10.7", idx: 7 });
    s.news.splice(s.news.findIndex((n: State) => n.id === "10.3") + 1, 0, { id: "10.7", kind: "change", sp: HONOR, check: null });
    restored(s, "10.3", 25);
    restored(s, "10.7", 26);
    const { mock } = await open(page, birta, s);
    await focusTo(page, HONOR);
    await page.keyboard.press("u");
    await expect.poll(() => undos(mock).sort()).toEqual(["25.0", "26.0"]);
  });

  test("u on a cut you put back and cut again sends the flip from its gap", async ({ page, birta }) => {
    const s = fresh();
    Object.assign(changeOf(s, "10.4"), { check: "restored", flip: "25.0" });
    newsOf(s, "10.4").check = "restored";
    const { mock } = await open(page, birta, recount(s));
    await focusRemoval(page, "10.4");
    await page.keyboard.press("u");
    await expect.poll(() => undos(mock)).toEqual(["25.0"]);
  });

  test("a settled revert that still names a flip: u says nothing here to revert and sends nothing", async ({ page, birta }) => {
    const s = reverted(fresh(), "10.1", 23);
    changeOf(s, "10.1").settled = true;
    s.news = s.news.filter((n: State) => n.id !== "10.1");
    const { mock } = await open(page, birta, recount(s));
    await focusTo(page, MERGED);
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("nothing here to revert");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });
});

// --- repair: z after an approve, a toggle another tab made stale, a refusal part way --------------

test.describe("repair: the z history around a revert", () => {
  test("once an approve lands, z takes no revert back: it says nothing to undo and writes nothing", async ({ page, birta }) => {
    const s = round(MERGE);
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "300" });
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await focusTo(page, await spNow(page, "Retry on network errors"));
    await pressAndWrite(page, served.root, "u", s.base, 1, "revert");
    // Claude's reply checked too, so the approve goes out at once
    await page.keyboard.press("Home");
    await page.keyboard.press("y");
    await expect.poll(() => (reviewLog(served.root) ?? []).filter((l) => l.op === "seen").length).toBe(1);
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState()?.counts.to_check)).toBe(0);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => stripMessage(page), { timeout: 8_000 }).toBe("approved ✓ · this text is final · nothing is published without you");
    const before = reviewLog(served.root)!.length;
    await page.keyboard.press("z");
    await expect.poll(() => stripMessage(page)).toBe("nothing to undo");
    await page.waitForTimeout(SETTLE_MS);
    expect(disk(served.root)).toBe(s.base);
    expect(reviewLog(served.root)!.length).toBe(before);
  });

  test("a revert another tab restored is spent: z says nothing to undo, and a later note's state never takes the note back", async ({ page, birta }) => {
    const { mock, state } = await open(page, birta, fresh());
    await focusTo(page, MERGED);
    await page.keyboard.press("u");
    await expect.poll(() => undos(mock)).toEqual(["10.1"]);
    const back = reverted(JSON.parse(JSON.stringify(state)), "10.1", 23);
    mock.pushState(back);
    await expect.poll(() => stripKeys(page)).toBe("reverted · u restore · c reply · n next");
    // Another tab restores it
    const other = JSON.parse(JSON.stringify(back));
    other.changes = other.changes.filter((c: State) => c.id !== "23.0");
    restored(other, "10.1", 24);
    mock.pushState(other);
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().changes.find((c: State) => c.id === "10.1").check)).toBe("restored");
    await page.keyboard.press("z");
    await expect.poll(() => stripMessage(page)).toBe("nothing to undo");
    await focusTo(page, HONOR);
    await page.keyboard.press("c");
    const area = page.locator('birta-flow[data-kind="composer"] textarea');
    await expect(area).toBeFocused();
    await area.fill("shorter");
    await area.press("Enter");
    await expect.poll(() => mock.ops().filter((o) => o.op === "comment").length).toBe(1);
    mock.pushState(other);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().filter((o) => o.op === "retract")).toEqual([]);
    expect(undos(mock)).toEqual(["10.1"]);
  });

  test("a restore refused part way keeps what it wrote on the z history: z takes back exactly that", async ({ page, birta }) => {
    const t1 = applied(applied(BASE, SHOUT), [{ a: 4, d: ["ALPHA one\n"], i: ["alpha one\n"] }]);
    const t2 = applied(t1, [{ a: 6, d: ["GAMMA three\n"], i: ["gamma three\n"] }]);
    const t3 = applied(t2, [{ a: 6, d: ["gamma three\n"], i: ["gamma three, edited\n"] }]);
    const s = round(SHOUT, ({ theirs }) => ({
      lines: [
        { by: human, op: "edit", act: "undo", of: "7.0", rev: revOf(theirs), to_rev: revOf(t1.join("\n")), patch: [{ a: 4, d: ["ALPHA one\n"], i: ["alpha one\n"] }] },
        { by: human, op: "edit", act: "undo", of: "7.1", rev: revOf(t1.join("\n")), to_rev: revOf(t2.join("\n")), patch: [{ a: 6, d: ["GAMMA three\n"], i: ["gamma three\n"] }] },
        // Your own later write over the second line, so its restore is refused
        { by: human, op: "edit", act: "edit", rev: revOf(t2.join("\n")), to_rev: revOf(t3.join("\n")), patch: [{ a: 6, d: ["gamma three\n"], i: ["gamma three, edited\n"] }] },
      ],
      text: t3.join("\n"),
    }));
    const served = await openReal(page, birta, s);
    await focusTo(page, await spNow(page, "alpha one"));
    const partly = applied(t3, [{ a: 4, d: ["alpha one\n"], i: ["ALPHA one\n"] }]).join("\n");
    await pressAndWrite(page, served.root, "u", partly, 4, "restore, refused part way");
    expect(edits(served.root)[3]).toMatchObject({ act: "undo", of: "10.0" });
    await expect.poll(() => stripMessage(page)).toContain(RESTORED);
    expect(await stripMessage(page)).toContain("can't restore this one: a later change overlaps · c to ask Claude");
    await page.waitForTimeout(SETTLE_MS);
    expect(edits(served.root)).toHaveLength(4);
    await pressAndWrite(page, served.root, "z", t3.join("\n"), 5, "z");
    expect(edits(served.root)[4]).toMatchObject({ act: "undo", of: `${edits(served.root)[3].n}.0` });
    await expect.poll(() => stripMessage(page)).toBe(RESTORE_UNDONE);
  });
});
