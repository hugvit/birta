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
  seededRound,
  spAt,
  stopSp,
  stripKeys,
  stripMessage,
  stripWhere,
  turnOf,
  withRev,
} from "./harness";

// The journey's first reply: twelve items of Claude's news on nine stops
const NAME = "journey/round1";
const DOC = fixtureDoc(NAME);
const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
/** How long after the guard's jump a ⌘⏎ is still ignored. */
const DWELL_MS = 1_000;

const WHY = "5:1-7:54";
const MERGED = "11:1-11:80";
const HONOR = "12:1-13:0";
const UNIT = "16:1-16:62";
const SUITE = "17:1-17:31";
// A heading that holds nothing of Claude's
const WHY_HEADING = spAt(DOC, "## Why");

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

interface W {
  BR: { currentState(): State };
}

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for the state. */
async function open(page: Page, birta: Start, state: State, doc = DOC, opts: { acks?: "auto" | "manual" } = {}) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state, acks: opts.acks });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
  return { served, mock, state };
}

/** The journey's reply with `edit` applied. */
function bent(edit?: (s: State) => void, name = NAME): State {
  const s = fixtureState(name);
  edit?.(s);
  return s;
}

const changeOf = (s: State, id: string) => s.changes.find((c: State) => c.id === id);
const newsOf = (s: State, id: string) => s.news.find((n: State) => n.id === id);

/** Checks news items `ids` as the fold would after a seen, with the change records to match. */
function checked(s: State, ids: string[], check?: string) {
  for (const id of ids) {
    const n = newsOf(s, id);
    n.check = check ?? (n.kind === "change" || n.kind === "removal" ? "kept" : "ok");
    const c = changeOf(s, id);
    if (c) {
      c.check = n.check;
      if (n.check === "kept") c.seen = true;
    }
  }
  return s;
}

/** The fold's count after `edit`: the stops still holding unchecked news. */
function recount(s: State) {
  const stopOf = (n: State) => {
    if (n.kind === "removal") return `r:${n.id}`;
    if (n.kind === "question") return `q:${n.id}`;
    if (n.sp) return `b:${n.sp}`;
    const item = s.items.find((i: State) => `#${i.n}` === n.id || (i.replies ?? []).some((r: State) => r.id === n.id));
    return item?.at?.change ? `r:${item.at.change}` : "doc";
  };
  s.counts.to_check = new Set(s.news.filter((n: State) => n.check == null).map(stopOf)).size;
  s.counts.attention = s.counts.to_check;
  return s;
}

/**
 * The merged bullets reverted by your write 23, as the fold shows it: the change stays news,
 * checked `reverted`, on the block its undo put back, and your write is its own change there.
 */
function revertedMerged(s: State, settled = false) {
  const c = changeOf(s, "10.1");
  Object.assign(c, { undone_by: 23, check: "reverted", settled: true, reversible: { ok: false, reason: "later_overlap" } });
  s.changes.push({
    ...JSON.parse(JSON.stringify(c)),
    id: "23.0",
    op: 23,
    idx: 0,
    turn: null,
    attr: "human",
    by: { type: "human", via: "browser" },
    asked: { kind: "location", notes: [] },
    undone_by: null,
    check: null,
    settled: false,
    seen: false,
    reversible: { ok: true, reason: "none" },
  });
  if (settled) s.news = s.news.filter((n: State) => n.id !== "10.1");
  else newsOf(s, "10.1").check = "reverted";
  return recount(s);
}

const opsNamed = (mock: Mock, name: string) => mock.ops().filter((o) => o.op === name);
/** Each seen op's ids, sorted, so the order a stop lists them in is free. */
const seenIds = (mock: Mock) => opsNamed(mock, "seen").map((o) => [...o.ids].sort());

/** The key hint the turn button shows on this platform. */
function hint(page: Page): Promise<string> {
  return page.evaluate(() => (/Mac/.test(navigator.platform) ? "⌘⏎" : "Ctrl⏎"));
}

const turn = (page: Page) => page.keyboard.press("ControlOrMeta+Enter");

/** Presses j or k until focus is on the removal stop of change `id`. */
async function focusRemoval(page: Page, id: string, key: "j" | "k" = "j") {
  for (let i = 0; i < 40; i++) {
    const m = await focusMark(page);
    if (m?.target === "removal" && m.change === id) return;
    await page.keyboard.press(key);
  }
  throw new Error(`40 presses of ${key} never reached the removal stop of ${id}: ${JSON.stringify(await focusMark(page))}`);
}

/** Puts focus on whole doc. */
async function home(page: Page) {
  await page.keyboard.press("Home");
  await expect.poll(() => focused(page)).toBe("doc");
}

/** Where focus is: the block's sp, `doc`, `blank`, or `removal <id>`. */
async function at(page: Page): Promise<string | null> {
  const m = await focusMark(page);
  if (!m) return null;
  return m.target === "removal" ? `removal ${m.change}` : (m.sp ?? m.target);
}

/** Every op the page sent but seen ops, as their names. */
const writes = (mock: Mock) => mock.ops().filter((o) => o.op !== "seen").map((o) => o.op);

// --- Landing never checks -----------------------------------------------------------------------

test.describe("landing never checks", () => {
  test("n, p, j, k, Home, End, a click on a block and a click on its row send no seen", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    // Every stop of the walk, then back
    for (let i = 0; i < 9; i++) {
      await page.keyboard.press("n");
      await page.waitForTimeout(80);
    }
    for (let i = 0; i < 3; i++) await page.keyboard.press("p");
    for (const key of ["j", "j", "k", "End", "Home"]) await page.keyboard.press(key);
    // A click into the changed paragraph opens its rows; a click on its row is no check either
    await page.locator(`#content [data-sourcepos="${WHY}"]`).click({ position: { x: 20, y: 8 } });
    await expect.poll(async () => (await changeRows(page)).length).toBeGreaterThan(0);
    await page.locator('birta-flow[data-kind="receipt"]').first().click();
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
    expect(mock.ops()).toEqual([]);
  });

  test("the walk lands on whole doc first, and the strip numbers it 1 of 9", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    // A tab opens with the walk's place before its first stop
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("doc");
    await expect.poll(() => stripWhere(page)).toBe("1 of 9 · whole doc · Claude replied");
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe(WHY);
    await expect.poll(() => stripWhere(page)).toBe("2 of 9 · ## Why › paragraph · Claude's change");
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
  });

  test("n walks nine stops, the reply to the cut section on that removal's stop", async ({ page, birta }) => {
    await open(page, birta, bent());
    const visited: (string | null)[] = [];
    for (let i = 0; i < 10; i++) {
      await page.keyboard.press("n");
      await page.waitForTimeout(80);
      visited.push(await at(page));
    }
    expect(visited).toEqual([
      "doc",
      WHY,
      MERGED,
      "removal 10.2",
      HONOR,
      "removal 10.4",
      "removal 10.5",
      UNIT,
      SUITE,
      // past the last stop it stays
      SUITE,
    ]);
    await expect.poll(() => stripMessage(page)).toBe("no more · p back");
  });
});

// --- y on Claude's news -------------------------------------------------------------------------

test.describe("y on Claude's news", () => {
  test("on whole doc: one seen naming its reply and turn note, and ok · Claude's replies", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await home(page);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#21", "#4"]]);
    await expect.poll(() => stripMessage(page)).toBe("ok · Claude's replies");
    // Nothing reaches Claude as a verdict
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("on a change and its done row: one seen naming both, and kept", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusTo(page, WHY);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#5", "10.0"]]);
    await expect.poll(() => stripMessage(page)).toBe("kept");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("on the section cut: one seen naming the removal and the reply to your no on it", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusRemoval(page, "10.5");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#6", "10.5"]]);
    await expect.poll(() => stripMessage(page)).toBe("kept");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("on a single cut bullet: one seen naming it, and kept", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusRemoval(page, "10.2");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["10.2"]]);
    await expect.poll(() => stripMessage(page)).toBe("kept");
  });

  test("on Claude's answer to your question: ok · Claude's answer", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusTo(page, SUITE);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#20"]]);
    await expect.poll(() => stripMessage(page)).toBe("ok · Claude's answer");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("on a reply alone and on a not-handled row: ok · Claude's reply", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureState("flow"), fixtureDoc("flow"));
    // #9 is the only news on its bullet
    await focusTo(page, "12:1-13:0");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#9"]]);
    await expect.poll(() => stripMessage(page)).toBe("ok · Claude's reply");
    // nh:7 is the only news on its bullet
    await focusTo(page, "10:1-10:16");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#9"], ["nh:7"]]);
    await expect.poll(() => stripMessage(page)).toBe("ok · Claude's reply");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("names only what is still unchecked on the stop", async ({ page, birta }) => {
    const { mock } = await open(page, birta, recount(checked(bent(), ["10.0"])));
    await focusTo(page, WHY);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#5"]]);
  });

  test("a kept change says already kept · u reverts it, and sends nothing", async ({ page, birta }) => {
    const { mock } = await open(page, birta, recount(checked(bent(), ["10.0", "#5"])));
    await focusTo(page, WHY);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe("already kept · u reverts it");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("a second y once the keep is back says already kept, and sends one seen in all", async ({ page, birta }) => {
    const { mock, state } = await open(page, birta, bent());
    await focusTo(page, UNIT);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["10.6"]]);
    const next = recount(checked(JSON.parse(JSON.stringify(state)), ["10.6"]));
    Object.assign(state, next);
    mock.pushState(state);
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().counts.to_check)).toBe(8);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe("already kept · u reverts it");
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([["10.6"]]);
  });

  test("y on a plain block is still yes, keep as is, and sends no seen", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusTo(page, WHY_HEADING);
    await page.keyboard.press("y");
    await expect.poll(() => opsNamed(mock, "comment").length).toBe(1);
    expect(opsNamed(mock, "comment")[0]).toMatchObject({ verdict: "yes", at: { k: "block", sp: WHY_HEADING } });
    expect(seenIds(mock)).toEqual([]);
  });

  test("x on news arms first; l and . name the block", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusTo(page, WHY);
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe("x = no to this text · u reverts Claude's change · x again for no");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("l");
    await expect.poll(() => stripMessage(page)).toMatch(/^later · "UploadClient\.put makes[^"]*…" \(paragraph\)/);
    await expect.poll(() => opsNamed(mock, "comment").map((o) => o.verdict)).toEqual(["later"]);
    expect(opsNamed(mock, "comment")[0].at).toMatchObject({ sp: WHY });
    // The ack names the later's line, so . changes that draft
    mock.pushState(
      bent((s) => {
        const like = JSON.parse(JSON.stringify(s.items.find((i: State) => i.n === 5)));
        s.items.push({ ...like, n: 101, id: "#101", cid: opsNamed(mock, "comment")[0].cid, verdict: "later", body: undefined, status: "draft", replies: [],
          at: { kind: "anchor", status: "ok", scope: "block", loc: { sp: WHY } } });
        s.counts.drafts = 1;
      }),
    );
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().counts.drafts)).toBe(1);
    await page.keyboard.press(".");
    await expect.poll(() => stripMessage(page)).toMatch(/^your call · "UploadClient\.put makes[^"]*…" \(paragraph\)/);
    await expect.poll(() => opsNamed(mock, "amend").map((o) => [o.to, o.verdict])).toEqual([[101, "your_call"]]);
    expect(seenIds(mock)).toEqual([]);
  });
});

// --- u on Claude's news -------------------------------------------------------------------------

/** The journey's reply with a second change of Claude's on the Honor bullet, revertible or not. */
function twoOnHonor(overlapped: boolean) {
  return bent((s) => {
    const c = changeOf(s, "10.3");
    s.changes.push({
      ...JSON.parse(JSON.stringify(c)),
      id: "10.7",
      idx: 7,
      reversible: overlapped ? { ok: false, reason: "later_overlap" } : { ok: true, reason: "none" },
    });
    const at = s.news.findIndex((n: State) => n.id === "10.3");
    s.news.splice(at + 1, 0, { id: "10.7", kind: "change", sp: HONOR, check: null });
  });
}

test.describe("u on Claude's news", () => {
  test("two revertible changes on one stop: u reverts both", async ({ page, birta }) => {
    const { mock } = await open(page, birta, twoOnHonor(false));
    await focusTo(page, HONOR);
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change").map((o) => o.change).sort()).toEqual(["10.3", "10.7"]);
  });

  test("two changes, one overlapped: u reverts the other and names the one it couldn't", async ({ page, birta }) => {
    const { mock } = await open(page, birta, twoOnHonor(true));
    await focusTo(page, HONOR);
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change").map((o) => o.change)).toEqual(["10.3"]);
    await expect.poll(() => stripMessage(page)).toContain("can't revert this one: a later change overlaps · c to ask Claude");
    // The revert's own message never hides what it couldn't do
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toContain("can't revert this one: a later change overlaps · c to ask Claude");
    expect(opsNamed(mock, "undo_change").map((o) => o.change)).toEqual(["10.3"]);
  });

  test("a removal is put back", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusRemoval(page, "10.4");
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change").map((o) => o.change)).toEqual(["10.4"]);
  });

  for (const [what, go] of [
    ["only Claude's answer", (page: Page) => focusTo(page, SUITE)],
    ["only whole doc's reply and turn note", home],
    ["nothing of Claude's", (page: Page) => focusTo(page, WHY_HEADING)],
  ] as const) {
    test(`with ${what} on the stop, u says nothing here to revert and writes nothing`, async ({ page, birta }) => {
      const { mock } = await open(page, birta, bent());
      await go(page);
      await page.keyboard.press("u");
      await expect.poll(() => stripMessage(page)).toBe("nothing here to revert");
      await page.waitForTimeout(SETTLE_MS);
      expect(mock.ops()).toEqual([]);
    });
  }

  test("a revert a hand-back settled keeps its ↺, and u there writes nothing", async ({ page, birta }) => {
    const { mock } = await open(page, birta, revertedMerged(bent(), true));
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "revert").map((m) => m.sp)).toEqual([MERGED]);
    await focusTo(page, MERGED);
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("nothing here to revert");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect((await marks(page)).filter((m) => m.mark === "revert").map((m) => m.sp)).toEqual([MERGED]);
  });

  test("a kept change a hand-back settled is no longer news: u there writes nothing", async ({ page, birta }) => {
    const s = recount(
      bent((s) => {
        checked(s, ["10.6"]);
        changeOf(s, "10.6").settled = true;
        s.news = s.news.filter((n: State) => n.id !== "10.6");
      }),
    );
    const { mock } = await open(page, birta, s);
    await focusTo(page, UNIT);
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("nothing here to revert");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });
});

// --- Interim copy until 6.26 and 6.27 -----------------------------------------------------------

test.describe("interim copy", () => {
  test("a revert says reverted, and z on it says a revert stays", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusTo(page, UNIT);
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change").map((o) => o.change)).toEqual(["10.6"]);
    await expect.poll(() => stripMessage(page)).toBe("reverted");
    await page.keyboard.press("z");
    await expect.poll(() => stripMessage(page)).toBe("nothing to undo · a revert stays");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual(["undo_change"]);
  });

  for (const key of ["u", "y"]) {
    test(`${key} on a reverted change says already reverted · c to ask Claude, and sends nothing`, async ({ page, birta }) => {
      const { mock } = await open(page, birta, revertedMerged(bent()));
      await focusTo(page, MERGED);
      await page.keyboard.press(key);
      await expect.poll(() => stripMessage(page)).toBe("already reverted · c to ask Claude");
      await page.waitForTimeout(SETTLE_MS);
      expect(mock.ops()).toEqual([]);
    });
  }

  test("a keep says kept, and z on it says a keep stays kept without reaching back to your note", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    // A note first, so a z that falls through would take it back
    await focusTo(page, WHY_HEADING);
    await page.keyboard.press("c");
    const area = page.locator('birta-flow[data-kind="composer"] textarea');
    await expect(area).toBeFocused();
    await area.fill("shorter");
    await area.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment").length).toBe(1);
    await focusTo(page, UNIT);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["10.6"]]);
    await expect.poll(() => stripMessage(page)).toBe("kept");
    await page.keyboard.press("z");
    await expect.poll(() => stripMessage(page)).toBe("nothing to undo · a keep stays kept");
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "retract")).toEqual([]);
    // The keep's marker is spent; the next z reaches the note
    await page.keyboard.press("z");
    await expect.poll(() => opsNamed(mock, "retract").length).toBe(1);
  });
});

// --- What a check shows -------------------------------------------------------------------------

test.describe("what a check shows", () => {
  test("a kept change: tail kept, the strip's kept keys, and a thin checked bar", async ({ page, birta }) => {
    await open(page, birta, recount(checked(bent(), ["10.0", "#5"])));
    // The bar beside it is a new one, for the width to compare with
    const bars = async () => (await marks(page)).filter((m) => m.mark === "bar");
    await expect.poll(async () => (await bars()).find((m) => m.sp === WHY)?.check).toBe("checked");
    const ms = await bars();
    const kept = ms.find((m) => m.sp === WHY)!;
    const fresh = ms.find((m) => m.sp === UNIT)!;
    expect(fresh.check).toBe("new");
    expect(kept.width).toBeLessThan(fresh.width);
    // The thin bar never carries a glyph
    expect((await marks(page)).filter((m) => m.mark === "glyph" && m.sp === WHY)).toEqual([]);
    await focusTo(page, WHY);
    await expect.poll(async () => (await changeRows(page)).map((r) => r.tail)).toContain("kept");
    expect(await stripWhere(page)).toBe("2 of 9 · ## Why › paragraph · Claude's change");
    await expect.poll(() => stripKeys(page)).toBe("kept · u revert · c reply · n next");
  });

  test("a reverted change: tail reverted by you, Claude's version and its text, and a ↺ in place of your bar", async ({ page, birta }) => {
    await open(page, birta, revertedMerged(bent()));
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "revert").map((m) => m.sp)).toEqual([MERGED]);
    expect((await marks(page)).filter((m) => m.mark === "bar" && m.sp === MERGED && m.role === "you")).toEqual([]);
    await focusTo(page, MERGED);
    await expect.poll(async () => (await changeRows(page)).map((r) => r.tail ?? "").find((t) => /^reverted by you/.test(t)) ?? null).not.toBeNull();
    const tail = (await changeRows(page)).map((r) => r.tail ?? "").find((t) => /^reverted by you/.test(t))!;
    expect(tail).toMatch(/^reverted by you\b.*Claude's version:.*Retry on network errors, 429 and 502–504/);
  });

  test("a change you replied to: tail you replied", async ({ page, birta }) => {
    await open(page, birta, recount(checked(bent(), ["10.6"], "replied")));
    await focusTo(page, UNIT);
    await expect.poll(async () => (await changeRows(page)).map((r) => r.tail)).toContain("you replied");
  });

  test("the bars of the journey's reply are all new", async ({ page, birta }) => {
    await open(page, birta, bent());
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "bar").length).toBeGreaterThan(0);
    const checks = (await marks(page)).filter((m) => m.mark === "bar").map((m) => m.check);
    expect(new Set(checks)).toEqual(new Set(["new"]));
  });
});

// --- The approve guard --------------------------------------------------------------------------

test.describe("the approve guard reads to check", () => {
  test("the first ⌘⏎ lands on whole doc, the first stop to check, says how many, and sends no seen", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await focusTo(page, UNIT);
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(`9 to check · n next · ${key} again approves as is`);
    await expect.poll(() => focused(page)).toBe("doc");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("with whole doc and Why checked it lands on the merged bullets", async ({ page, birta }) => {
    const { mock } = await open(page, birta, recount(checked(bent(), ["#4", "#21", "10.0", "#5"])));
    await focusTo(page, SUITE);
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(`7 to check · n next · ${key} again approves as is`);
    await expect.poll(() => focused(page)).toBe(MERGED);
    expect(seenIds(mock)).toEqual([]);
  });

  test("an open question counts, and the guard lands on it", async ({ page, birta }) => {
    const s = fixtureState("flow");
    for (const n of s.news) if (n.id !== "#21" && n.check == null) n.check = n.kind === "change" ? "kept" : "ok";
    s.counts = { ...s.counts, to_check: 1, attention: 1, drafts: 0 };
    s.items = s.items.filter((i: State) => !(i.by?.type === "human" && i.status === "draft"));
    const { mock } = await open(page, birta, s, fixtureDoc("flow"));
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(`1 to check · n next · ${key} again approves as is`);
    await expect.poll(() => focused(page)).toBe("blank");
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("it holds until another key, past the 1 s dwell; then ⌘⏎ approves as is", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(`9 to check · n next · ${key} again approves as is`);
    // Inside the dwell a second press does nothing
    await turn(page);
    await page.waitForTimeout(DWELL_MS + 200);
    expect(opsNamed(mock, "approve")).toEqual([]);
    expect(await stripMessage(page)).toBe(`9 to check · n next · ${key} again approves as is`);
    await turn(page);
    await expect.poll(() => opsNamed(mock, "approve").length).toBe(1);
    expect(seenIds(mock)).toEqual([]);
  });

  test("another key ends the hold: the next ⌘⏎ guards again", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(`9 to check · n next · ${key} again approves as is`);
    await page.keyboard.press("j");
    await page.waitForTimeout(DWELL_MS + 200);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(`9 to check · n next · ${key} again approves as is`);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("with nothing to check, ⌘⏎ approves at once", async ({ page, birta }) => {
    const s = recount(bent((s) => checked(s, s.news.map((n: State) => n.id))));
    const { mock } = await open(page, birta, s);
    await turn(page);
    await expect.poll(() => opsNamed(mock, "approve").length).toBe(1);
  });
});

// --- The tab's ● --------------------------------------------------------------------------------

test.describe("the tab's ● reads to check", () => {
  test("something to check puts ● in the title, whatever attention says", async ({ page, birta }) => {
    await open(
      page,
      birta,
      bent((s) => (s.counts = { ...s.counts, to_check: 2, attention: 0 })),
    );
    await expect(page).toHaveTitle("● pr.md — birta");
  });

  test("nothing to check leaves the title plain, whatever attention says", async ({ page, birta }) => {
    await open(
      page,
      birta,
      bent((s) => {
        checked(s, s.news.map((n: State) => n.id));
        s.counts = { ...s.counts, to_check: 0, attention: 3 };
      }),
    );
    await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page).toHaveTitle("pr.md — birta");
  });
});

// --- The folder-notes row joins the walk --------------------------------------------------------

test.describe("the folder-notes row", () => {
  test("in folder mode, notes on the folder make whole doc a walk stop", async ({ page, birta }) => {
    const doc = "# PR\n\nText.\n";
    const fresh = fixtureState("fresh");
    const summary = { stage: fresh.phrase.stage, phrase: fresh.phrase, counts: fresh.counts, notes: 2 };
    const dir = {
      type: "dir_state",
      root: "",
      truncated: false,
      folder: { summary },
      files: [{ paths: ["b.md"], summary: null, news: false }],
      totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0 },
    };
    const served = await birta({ "pr.md": doc, "b.md": "# B\n" }, GATE_ON, ["."]);
    await page.setViewportSize(WIDE);
    await mockReview(page, { state: withRev(fresh, revOf(doc)), dir });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).not.toBe("doc");
    await page.keyboard.press("p");
    await expect.poll(() => focused(page)).toBe("doc");
  });
});

// --- adversary ----------------------------------------------------------------------------------

/** Replaces `state` in place with `next`, so the mock's content echo serves `next` too, and pushes it. */
function swap(mock: Mock, state: State, next: State) {
  for (const k of Object.keys(state)) delete state[k];
  Object.assign(state, next);
  mock.pushState(state);
}

const toCheck = (n: number, key: string) => `${n} to check · n next · ${key} again approves as is`;
const toCheckNow = (page: Page) => page.evaluate(() => (window as unknown as W).BR.currentState().counts.to_check);

test.describe("adversary: the guard and the walk read one list", () => {
  test("adversary: a not-handled row pushed onto a stop while armed disarms; the next ⌘⏎ guards again", async ({ page, birta }) => {
    // Whole doc checked: 8 stops left, the first the Why paragraph
    const { mock, state } = await open(page, birta, recount(checked(bent(), ["#4", "#21"])));
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(8, key));
    await expect.poll(() => focused(page)).toBe(WHY);
    // A second note of yours on Why that Claude's turn left not handled: a new item on a stop already to check
    const next = JSON.parse(JSON.stringify(state));
    const five = next.items.find((i: State) => i.n === 5);
    next.items.push({ ...JSON.parse(JSON.stringify(five)), n: 30, body: "and the retry count", status: "not_handled", replies: [] });
    const at = next.news.findIndex((n: State) => n.id === "#5");
    next.news.splice(at + 1, 0, { id: "nh:30", kind: "not_handled", sp: WHY, check: null });
    recount(next);
    expect(next.counts.to_check).toBe(8);
    swap(mock, state, next);
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().news.some((n: State) => n.id === "nh:30"))).toBe(true);
    await page.waitForTimeout(DWELL_MS + 200);
    await turn(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(8, key));
  });

  test("adversary: a turn note pushed onto a checked whole doc while armed disarms, and the guard goes back to whole doc", async ({ page, birta }) => {
    const { mock, state } = await open(page, birta, recount(checked(bent(), ["#4", "#21"])));
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(8, key));
    const next = JSON.parse(JSON.stringify(state));
    const at = next.news.findIndex((n: State) => n.id === "#21");
    next.news.splice(at + 1, 0, { id: "#30", kind: "turn_note", sp: null, check: null });
    recount(next);
    expect(next.counts.to_check).toBe(9);
    // The old unseen count doesn't move: only the news list and to_check say so
    expect(next.counts.unseen).toBe(state.counts.unseen);
    swap(mock, state, next);
    await expect.poll(() => toCheckNow(page)).toBe(9);
    await page.waitForTimeout(DWELL_MS + 200);
    await turn(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(9, key));
    await expect.poll(() => focused(page)).toBe("doc");
  });

  test("adversary: the guard passes over the lost-notes row and a stop whose first item is checked lands there", async ({ page, birta }) => {
    // lost.json: whole doc holds #4, #11, #12 and the lost row; 9.0 and 14.0 share 5:1-6:68
    const s = fixtureState("lost");
    for (const n of s.news) if (["#4", "#11", "#12", "9.0"].includes(n.id)) n.check = n.kind === "change" ? "kept" : "ok";
    Object.assign(s.changes.find((c: State) => c.id === "9.0"), { check: "kept", seen: true });
    s.counts = { ...s.counts, to_check: 2, attention: 2 };
    const { mock } = await open(page, birta, s, fixtureDoc("lost"));
    await expect.poll(() => turnOf(page)).toMatch(/^Approve/);
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(2, key));
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("adversary: with only the reply to your no on the cut section left, the guard lands on that removal", async ({ page, birta }) => {
    const s = recount(checked(bent(), bent().news.map((n: State) => n.id).filter((id: string) => id !== "#6")));
    expect(s.counts.to_check).toBe(1);
    const { mock } = await open(page, birta, s);
    await focusTo(page, SUITE);
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(1, key));
    await expect.poll(() => at(page)).toBe("removal 10.5");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });
});

test.describe("adversary: u on a checked change still in news", () => {
  for (const check of ["kept", "replied"]) {
    test(`adversary: u on a change you ${check} reverts it`, async ({ page, birta }) => {
      const { mock } = await open(page, birta, recount(checked(bent(), ["10.6"], check)));
      await focusTo(page, UNIT);
      await page.keyboard.press("u");
      await expect.poll(() => opsNamed(mock, "undo_change").map((o) => o.change)).toEqual(["10.6"]);
      await expect.poll(() => stripMessage(page)).toBe("reverted");
      expect(seenIds(mock)).toEqual([]);
    });
  }

  test("adversary: u on a cut you kept puts it back", async ({ page, birta }) => {
    const { mock } = await open(page, birta, recount(checked(bent(), ["10.4"])));
    await focusRemoval(page, "10.4");
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change").map((o) => o.change)).toEqual(["10.4"]);
    expect(seenIds(mock)).toEqual([]);
  });
});

test.describe("adversary: z on a keep or a revert never reaches an older action", () => {
  /** Saves a note on the ## Why heading, so a z that falls through would retract it. */
  async function noteFirst(page: Page, mock: Mock) {
    await focusTo(page, WHY_HEADING);
    await page.keyboard.press("c");
    const area = page.locator('birta-flow[data-kind="composer"] textarea');
    await expect(area).toBeFocused();
    await area.fill("shorter");
    await area.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment").length).toBe(1);
  }

  test("adversary: two keeps are two markers: two z say a keep stays, and only the third retracts the note", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await noteFirst(page, mock);
    await focusTo(page, WHY);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe("kept");
    await focusTo(page, UNIT);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock).length).toBe(2);
    for (let i = 0; i < 2; i++) {
      await page.keyboard.press("z");
      await expect.poll(() => stripMessage(page)).toBe("nothing to undo · a keep stays kept");
      await page.waitForTimeout(SETTLE_MS);
      expect(opsNamed(mock, "retract")).toEqual([]);
    }
    await page.keyboard.press("z");
    await expect.poll(() => opsNamed(mock, "retract").length).toBe(1);
  });

  test("adversary: z after a revert says a revert stays, and leaves the note before it alone", async ({ page, birta }) => {
    const { mock } = await open(page, birta, bent());
    await noteFirst(page, mock);
    await focusTo(page, UNIT);
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change").map((o) => o.change)).toEqual(["10.6"]);
    await page.keyboard.press("z");
    await expect.poll(() => stripMessage(page)).toBe("nothing to undo · a revert stays");
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "retract")).toEqual([]);
    expect(writes(mock)).toEqual(["comment", "undo_change"]);
  });
});

// --- adversary: a real server, its log and the file's bytes --------------------------------------

const PARA = [
  "# Upload retries",
  "",
  "## Why",
  "",
  "When the upload hits a 503 we just fail.",
  "The user has to start again.",
  "Nobody retries.",
  "S3 answers 503 under load.",
  "",
  "## What changes",
  "",
  "- retries on 503",
  "",
];
/**
 * Claude's two edits on the one paragraph: its first two lines, and its last, the third kept.
 * An outside write over only the first line leaves the first change in the doc, overlapped.
 */
const FIRST = { line: 5, to: ["When the upload hits a 503 we retry.", "The user never sees it."] };
const LAST = { line: 8, to: ["S3 answers 503 under load; three retries fix most of them."] };

/** A whole-doc note handed back, then one write of Claude's with two hunks on one paragraph, done and yield. */
function twoHunksOneParagraph(): { orig: string; doc: string; log: string } {
  const orig = PARA.join("\n");
  const after = [...PARA];
  for (const e of [FIRST, LAST]) after.splice(e.line - 1, e.to.length, ...e.to);
  const doc = after.join("\n");
  const T0 = Date.now() - 600_000;
  const human = { type: "human", via: "browser" };
  const agent = { type: "agent", name: "claude", via: "cli" };
  const lines: State[] = [
    { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
    { by: { type: "birta" }, op: "snap", rev: revOf(orig), text: orig },
    { by: { type: "birta" }, op: "owner", drafted_by: null },
    { by: human, cid: "c-4", op: "comment", at: "doc", body: "tighten it" },
    { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
    { by: agent, op: "claim", round: 5 },
    {
      by: { type: "birta" },
      op: "external",
      rev: revOf(orig),
      to_rev: revOf(doc),
      patch: [FIRST, LAST].map((e) => ({
        a: e.line - 1,
        d: PARA.slice(e.line - 1, e.line - 1 + e.to.length).map((l) => l + "\n"),
        i: e.to.map((l) => l + "\n"),
      })),
    },
    { by: agent, op: "done", to: 4, turn: 6 },
    { by: agent, op: "yield", turn: 6 },
  ];
  return { orig, doc, log: lines.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join("") };
}

/** Every review_state the page receives for pr.md. Call before `goto`. */
function states(page: Page): State[] {
  const got: State[] = [];
  page.on("websocket", (ws) => {
    ws.on("framereceived", (f) => {
      try {
        const m = JSON.parse(typeof f.payload === "string" ? f.payload : f.payload.toString());
        if (m.type === "review_state" && m.path === "pr.md") got.push(m.state);
      } catch {
        // not a JSON frame
      }
    });
  });
  return got;
}

test.describe("adversary: on a real server", () => {
  test("adversary: u on a paragraph holding Claude's revertible change and an outside write over its other one reverts only the first, byte for byte", async ({ page, birta }) => {
    const s = twoHunksOneParagraph();
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    const got = states(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => got.at(-1)?.changes.map((c: State) => c.id)).toEqual(["7.0", "7.1"]);
    // An outside write over Claude's first line only
    const over = s.doc.replace(FIRST.to[0], "When the upload hits a 503 we retry, then fail.");
    served.write("pr.md", over);
    await expect
      .poll(() => (got.at(-1)?.changes ?? []).map((c: State) => [c.id, c.reversible.reason]))
      .toEqual([["7.0", "later_overlap"], ["7.1", "none"], ["10.0", "outside"]]);
    // The stop holds Claude's overlapped change, its revertible one and the outside write over the first
    const sp = await stopSp(page, "then fail");
    expect(got.at(-1).news.filter((n: State) => n.sp === sp).map((n: State) => n.id)).toEqual(["7.0", "7.1", "10.0"]);
    await focusTo(page, sp);
    const before = reviewLog(served.root)!.length;
    await page.keyboard.press("u");
    await expect.poll(() => reviewLog(served.root)!.slice(before).filter((l) => l.op === "edit")).toHaveLength(1);
    expect(reviewLog(served.root)!.slice(before).filter((l) => l.op === "edit")[0]).toMatchObject({ act: "undo", of: "7.1" });
    // The outside line stays; Claude's last line goes back to the text before its turn
    await expect.poll(() => fs.readFileSync(path.join(served.root, "pr.md"), "utf-8")).toBe(over.replace(LAST.to[0], PARA[LAST.line - 1]));
    // The overlapped one is named, not reverted
    await expect.poll(() => stripMessage(page)).toContain("can't revert this one: a later change overlaps · c to ask Claude");
    await page.waitForTimeout(SETTLE_MS);
    // One revert, no seen from walking there, and nothing else written
    expect(reviewLog(served.root)!.slice(before).map((l) => l.op)).toEqual(["edit"]);
    expect(reviewLog(served.root)!.filter((l) => l.op === "seen")).toEqual([]);
  });

  test("adversary: the guard, then y on each stop, writes one seen per stop and no verdict, and then ⌘⏎ approves at once", async ({ page, birta }) => {
    const s = seededRound(1);
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    const got = states(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    // Setup: whole doc holds the done on your note, and the paragraph Claude's change
    await expect.poll(() => got.at(-1)?.counts.to_check).toBe(2);
    expect(got.at(-1).news.map((n: State) => n.id)).toEqual(["#4", "7.0"]);
    const seeded = reviewLog(served.root)!.length;
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(2, key));
    await expect.poll(() => focused(page)).toBe("doc");
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.slice(seeded).filter((l) => l.op === "seen").map((l) => l.ids)).toEqual([["#4"]]);
    await focusTo(page, await stopSp(page, "we just fail, and the user"));
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.slice(seeded).filter((l) => l.op === "seen").map((l) => l.ids)).toEqual([["#4"], ["7.0"]]);
    await expect.poll(() => got.at(-1)?.counts.to_check).toBe(0);
    await expect.poll(() => toCheckNow(page)).toBe(0);
    await turn(page);
    await expect.poll(() => reviewLog(served.root)!.slice(seeded).filter((l) => l.op === "approve").length).toBe(1);
    // Nothing reached Claude as a verdict
    expect(reviewLog(served.root)!.slice(seeded).map((l) => l.op)).toEqual(["seen", "seen", "approve"]);
  });

  test("adversary: landing and lingering on each stop, then the page going away, writes no seen", async ({ page, birta }) => {
    const s = seededRound(2);
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    const got = states(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => got.at(-1)?.counts.to_check).toBe(3);
    const seeded = reviewLog(served.root)!.length;
    // Longer than any dwell a read-marker could use
    for (const key of ["n", "n", "n", "k"]) {
      await page.keyboard.press(key);
      await page.waitForTimeout(2_500);
    }
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    await page.waitForTimeout(800);
    expect(reviewLog(served.root)!.slice(seeded)).toEqual([]);
    expect(got.at(-1).counts.to_check).toBe(3);
  });
});

// --- repair -------------------------------------------------------------------------------------

/** Presses n until focus is on `sp`, failing after `max` stops. */
async function nTo(page: Page, sp: string, max = 12) {
  for (let i = 0; i < max && (await focused(page)) !== sp; i++) {
    await page.keyboard.press("n");
    await page.waitForTimeout(80);
  }
  await expect.poll(() => focused(page)).toBe(sp);
}

test.describe("repair: every change on a stop shows before y keeps it", () => {
  test("n onto a block with two changes opens a row showing both, and y keeps both", async ({ page, birta }) => {
    const s = twoOnHonor(false);
    Object.assign(changeOf(s, "10.7"), { old_text: "the old second words\n", new_text: "the new second words\n" });
    const { mock } = await open(page, birta, s);
    await nTo(page, HONOR);
    // The second change's substance: "old" struck, "new" underlined
    await expect.poll(async () => (await changeRows(page)).filter((r) => r.sp === HONOR).flatMap((r) => r.ins)).toContain("new");
    expect((await changeRows(page)).filter((r) => r.sp === HONOR).flatMap((r) => r.del)).toContain("old");
    const shown = (await changeRows(page)).filter((r) => r.sp === HONOR).map((r) => r.text).join(" | ");
    expect(shown).toContain("Retry-After");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["10.3", "10.7"]]);
  });

  test("n onto the stop of a change a later write overlapped opens its row there, not under whole doc", async ({ page, birta }) => {
    const s = bent((s) => {
      const c = changeOf(s, "10.6");
      s.changes.push({
        ...JSON.parse(JSON.stringify(c)),
        id: "10.8",
        idx: 8,
        range: { status: "superseded", lines: [16, 16], bytes: [0, 0], sp: null },
        reversible: { ok: false, reason: "later_overlap" },
        old_text: "- [ ] Unit tests\n",
        new_text: "- [x] Overlapped words of Claude\n",
      });
      const at = s.news.findIndex((n: State) => n.id === "10.6");
      s.news.splice(at + 1, 0, { id: "10.8", kind: "change", sp: UNIT, check: null });
    });
    const { mock } = await open(page, birta, s);
    await nTo(page, UNIT);
    await expect.poll(async () => (await changeRows(page)).filter((r) => r.sp === UNIT).map((r) => r.text).join(" | ")).toContain("Overlapped words of Claude");
    expect((await changeRows(page)).filter((r) => r.sp !== UNIT).map((r) => r.text).join(" | ")).not.toContain("Overlapped words of Claude");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["10.6", "10.8"]]);
  });
});

test.describe("repair: the guard after an approve", () => {
  test("approved, then changed since with something to check: the first ⌘⏎ guards", async ({ page, birta }) => {
    const s = fixtureState("approved");
    Object.assign(s.approve, { changed_since: true, current_matches: false, later: 0 });
    s.changes.push({ ...JSON.parse(JSON.stringify(s.changes[0])), id: "17.0", op: 17, attr: "outside", seen: false, settled: false, check: null });
    s.news = [{ id: "17.0", kind: "change", sp: s.changes[0].range.sp, check: null }];
    s.counts = { ...s.counts, to_check: 1, attention: 1 };
    const { mock } = await open(page, birta, s, fixtureDoc("approved"));
    await expect.poll(() => turnOf(page)).toMatch(/^Approve/);
    const key = await hint(page);
    await turn(page);
    await expect.poll(() => stripMessage(page)).toBe(`1 to check · n next · ${key} again approves as is`);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
  });
});

/** Claude writes the paragraph's first line, then rewrites that same line, in one turn. */
function rewroteItsOwnLine(): { doc: string; first: string; log: string } {
  const orig = PARA.join("\n");
  const firstTry = [...PARA];
  firstTry[4] = "When the upload hits a 503 we retry.";
  const second = [...PARA];
  second[4] = "When the upload hits a 503 we retry three times.";
  const T0 = Date.now() - 600_000;
  const human = { type: "human", via: "browser" };
  const agent = { type: "agent", name: "claude", via: "cli" };
  const write = (from: string[], to: string[]) => ({
    by: { type: "birta" },
    op: "external",
    rev: revOf(from.join("\n")),
    to_rev: revOf(to.join("\n")),
    patch: [{ a: 4, d: [from[4] + "\n"], i: [to[4] + "\n"] }],
  });
  const lines: State[] = [
    { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
    { by: { type: "birta" }, op: "snap", rev: revOf(orig), text: orig },
    { by: { type: "birta" }, op: "owner", drafted_by: null },
    { by: human, cid: "c-4", op: "comment", at: "doc", body: "tighten it" },
    { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
    { by: agent, op: "claim", round: 5 },
    write(PARA, firstTry),
    write(firstTry, second),
    { by: agent, op: "done", to: 4, turn: 6 },
    { by: agent, op: "yield", turn: 6 },
  ];
  return {
    doc: second.join("\n"),
    first: firstTry.join("\n"),
    log: lines.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join(""),
  };
}

test.describe("repair: on a real server", () => {
  test("Claude's own rewrite of its line in one turn is one change: u reverts it and names nothing it couldn't", async ({ page, birta }) => {
    const s = rewroteItsOwnLine();
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    const got = states(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => got.at(-1)?.changes.map((c: State) => c.id)).toEqual(["7.0", "8.0"]);
    const sp = await stopSp(page, "three times");
    expect(got.at(-1).news.filter((n: State) => n.sp === sp).map((n: State) => n.id)).toEqual(["8.0"]);
    await focusTo(page, sp);
    const before = reviewLog(served.root)!.length;
    await page.keyboard.press("u");
    await expect.poll(() => reviewLog(served.root)!.slice(before).filter((l) => l.op === "edit")).toHaveLength(1);
    expect(reviewLog(served.root)!.slice(before).filter((l) => l.op === "edit")[0]).toMatchObject({ act: "undo", of: "8.0" });
    await expect.poll(() => fs.readFileSync(path.join(served.root, "pr.md"), "utf-8")).toBe(s.first);
    await expect.poll(() => stripMessage(page)).toBe("reverted");
    await page.waitForTimeout(SETTLE_MS);
    expect((await stripMessage(page)) ?? "").not.toContain("can't revert");
  });
});
