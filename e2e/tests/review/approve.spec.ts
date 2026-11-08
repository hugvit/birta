import { Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  GATE_ON,
  Mock,
  REPO,
  State,
  booted,
  changeRows,
  dispatchKey,
  fixtureDoc,
  fixtureState,
  focusTo,
  focused,
  highlighted,
  marks,
  mockReview,
  revOf,
  reviewLog,
  seededRound,
  stops,
  stripMessage,
  stripPlace,
  stripText,
} from "./harness";

test.use({ timezoneId: "UTC" });

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
/** How long after the guard's jump a ⌘⏎ is still ignored. */
const DWELL_MS = 1_000;
const REOPEN = "approved · c to reopen";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** Serves committed fixture `name` as pr.md with its state, bent by `edit`. */
async function openFixture(page: Page, birta: Start, name: string, edit?: (s: State) => void) {
  const state = fixtureState(name);
  edit?.(state);
  const served = await birta({ "pr.md": fixtureDoc(name) }, GATE_ON);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock, state };
}

/** The fields of a committed op frame, without the envelope the core adds. */
function fixtureOp(name: string): State {
  const file = path.join(REPO, "e2e", "fixtures", "review", "ops", `${name}.json`);
  const out = { ...JSON.parse(fs.readFileSync(file, "utf-8")).frame };
  for (const k of ["type", "cid", "path", "rev"]) delete out[k];
  return out;
}

const seenIds = (mock: Mock) => mock.ops().filter((o) => o.op === "seen").map((o) => o.ids);
const opsNamed = (mock: Mock, name: string) => mock.ops().filter((o) => o.op === name);
const hhmm = (ts: number) => new Date(ts).toISOString().slice(11, 16);

/** The key hint the turn button shows on this platform. */
function hint(page: Page): Promise<string> {
  return page.evaluate(() => (/Mac/.test(navigator.platform) ? "⌘⏎" : "Ctrl⏎"));
}

/** The turn button's label without its key hint, whether the hint shows, and whether it acts. */
function turnOf(page: Page): Promise<{ text: string; kbd: boolean; disabled: boolean } | null> {
  return page.evaluate(() => {
    const t = document.querySelector(".file-header birta-turn");
    if (!t || !t.getClientRects().length) return null;
    const btn = (t.shadowRoot ?? t).querySelector("button");
    if (!btn) return null;
    const kbd = btn.querySelector("kbd");
    const all = (btn.textContent ?? "").replace(/\s+/g, " ").trim();
    const k = kbd ? (kbd.textContent ?? "").trim() : "";
    return { text: k && all.endsWith(k) ? all.slice(0, -k.length).trim() : all, kbd: !!kbd, disabled: btn.disabled };
  });
}

/** The header phrase as shown, or null. */
function phraseOf(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const p = document.querySelector(".file-header birta-phrase");
    if (!p || !p.getClientRects().length) return null;
    const text = (n: Node): string =>
      n.nodeType === 3
        ? (n.textContent ?? "")
        : n.nodeType === 1 && (n as Element).localName === "style"
          ? ""
          : Array.from(((n as Element).shadowRoot ?? n).childNodes).map(text).join("");
    return text(p).replace(/\s+/g, " ").trim();
  });
}

/** The kinds of flow hosts that take up room on the page; the whole doc stop's own line is no row. */
function shownHosts(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll('birta-flow:not([data-kind="doc-stop"])'))
      .filter((h) => {
        const r = h.getBoundingClientRect();
        return r.height > 0 && r.width > 0 && getComputedStyle(h).visibility !== "hidden";
      })
      .map((h) => h.getAttribute("data-kind") ?? "?"),
  );
}

/** The drawn lane marks other than focus and dots: bars, glyphs, washes and ticks. */
async function laneMarks(page: Page): Promise<string[]> {
  return (await marks(page))
    .filter((m) => ["bar", "glyph", "wash", "tick"].includes(m.mark) && m.width > 0)
    .map((m) => `${m.mark} ${m.sp ?? m.after ?? "top"}`);
}

async function dotsOn(page: Page): Promise<string[]> {
  return (await marks(page))
    .filter((m) => m.mark === "dot" && m.width > 0)
    .map((m) => m.sp!)
    .sort();
}

const composerText = (page: Page) =>
  page.evaluate(() => {
    const h = document.querySelector('birta-flow[data-kind="composer"]');
    if (!h) return null;
    const text = (n: Node): string =>
      n.nodeType === 3
        ? (n.textContent ?? "")
        : n.nodeType === 1 && (n as Element).localName === "style"
          ? ""
          : Array.from(((n as Element).shadowRoot ?? n).childNodes).map(text).join("");
    return text(h).replace(/\s+/g, " ").trim();
  });

/** The open composer's label line and target line, or null with no composer. */
const composerLines = (page: Page) =>
  page.evaluate(() => {
    const root = document.querySelector('birta-flow[data-kind="composer"]')?.shadowRoot;
    if (!root) return null;
    return { label: root.querySelector(".label")?.textContent ?? null, target: root.querySelector(".target")?.textContent ?? null };
  });

/** Checks news items `ids`, as y would, and takes the stops they emptied off `counts.to_check`. */
function check(s: State, ids: string[], stops: number) {
  for (const id of ids) {
    const n = s.news.find((x: State) => x.id === id);
    n.check = n.kind === "change" || n.kind === "removal" ? "kept" : "ok";
  }
  s.counts.to_check -= stops;
  s.counts.attention = s.counts.to_check;
}

/**
 * unseen.json with whole doc checked: its reply seen and okayed, and its turn note okayed, so
 * four stops are left to check, the paragraph under ## Why first.
 */
function changesOnly(s: State) {
  s.items.find((i: State) => i.n === 6).replies[0].seen = true;
  check(s, ["#13", "#14"], 1);
}

/** The page's own count of stops left to check. */
const toCheckCount = (page: Page) =>
  page.evaluate(() => (window as unknown as { BR: { currentState(): State } }).BR.currentState().counts.to_check);

/** The turn button as it reads whenever approving is yours: it never counts. */
const APPROVE = { text: "Approve", kbd: true, disabled: false };

/** What the guard says with `n` stops left to check. */
const toCheck = (n: number, key: string) => `${n} to check · n next · ${key} again approves as is`;

const press = (page: Page) => page.keyboard.press("ControlOrMeta+Enter");

/** Presses j from whole doc until `sp` has focus, failing after a few stops. */
async function focusOn(page: Page, sp: string) {
  for (let i = 0; i < 8 && (await focused(page)) !== "doc"; i++) await page.keyboard.press("k");
  for (let i = 0; i < 4 && (await focused(page)) !== sp; i++) {
    await page.keyboard.press("j");
    await page.waitForTimeout(50);
  }
  await expect.poll(() => focused(page)).toBe(sp);
}

// --- The guard: two steps, never a next ------------------------------------------------------

test.describe("approve guard", () => {
  test("the first ⌘⏎ goes to the first stop to check, opens it, says what a second press does, and checks nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    const key = await hint(page);
    // The button never counts; the guard below does
    await expect.poll(() => turnOf(page)).toEqual({ text: "Approve", kbd: true, disabled: false });
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await expect.poll(() => stripMessage(page)).toBe(toCheck(4, key));
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("a burst of ⌘⏎ never approves and never steps on to the next change", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    for (let i = 0; i < 5; i++) await press(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    expect(seenIds(mock)).toEqual([]);
    expect(await focused(page)).toBe("5:1-6:68");
  });

  test("the dwell runs from the jump, not from the last press: a ⌘⏎ past it approves", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    await press(page);
    const t0 = Date.now();
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    for (const ms of [400, 800]) {
      await page.waitForTimeout(Math.max(0, t0 + ms - Date.now()));
      await press(page);
    }
    await page.waitForTimeout(100);
    expect(opsNamed(mock, "approve")).toEqual([]);
    expect(seenIds(mock)).toEqual([]);
    expect(await focused(page)).toBe("5:1-6:68");
    // An ignored press leaves the guard's pill up
    expect(await stripMessage(page)).toMatch(/^4 to check · n next · /);
    await page.waitForTimeout(Math.max(0, t0 + DWELL_MS + 250 - Date.now()));
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
    expect(seenIds(mock)).toEqual([]);
  });

  test("the second ⌘⏎ approves as is with stops still to check, sending the plain approve", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.waitForTimeout(DWELL_MS + 200);
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
    const op = { ...opsNamed(mock, "approve")[0] };
    for (const k of ["type", "cid", "path", "rev"]) delete op[k];
    expect(op).toEqual(fixtureOp("approve"));
  });

  test("the arm never times out: well past every pill window it still shows and still approves", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    const key = await hint(page);
    await press(page);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(4, key));
    await page.waitForTimeout(4_500);
    expect(await stripMessage(page)).toBe(toCheck(4, key));
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
  });

  test("any other key disarms it: the next ⌘⏎ is a first press again", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    const key = await hint(page);
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.waitForTimeout(DWELL_MS + 200);
    await page.keyboard.press("j");
    await expect.poll(() => stripMessage(page)).not.toBe(toCheck(4, key));
    await press(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    expect(await stripMessage(page)).toBe(toCheck(4, key));
    // The new jump has its own dwell
    await press(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("the modifier's own keydown is part of ⌘⏎ and never disarms", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    const mod = (await hint(page)) === "⌘⏎" ? "Meta" : "Control";
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.waitForTimeout(DWELL_MS + 200);
    // Held down and let go on its own first, as a hand on its way to ⌘⏎ would
    await page.keyboard.down(mod);
    await page.keyboard.up(mod);
    await page.keyboard.down(mod);
    await page.keyboard.press("Enter");
    await page.keyboard.up(mod);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
  });

  test("whole doc's reply alone still stops approve, and the jump lands there checking nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => {
      for (const c of s.changes) c.seen = true;
      s.counts.unseen = 0;
      // Every stop but whole doc, which holds the reply and the turn note
      check(s, ["9.0", "#4", "nh:5", "10.0", "11.0"], 4);
    });
    const key = await hint(page);
    await expect.poll(() => turnOf(page)).toEqual({ text: "Approve", kbd: true, disabled: false });
    await press(page);
    await expect.poll(() => focused(page)).toBe("doc");
    await expect.poll(() => stripMessage(page)).toBe(toCheck(1, key));
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("the jump takes the first stop to check in walk order, whole doc above every change", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    const key = await hint(page);
    // The button never counts: what is left to check is the guard's to say
    await expect.poll(() => turnOf(page)).toEqual({ text: "Approve", kbd: true, disabled: false });
    await press(page);
    await expect.poll(() => focused(page)).toBe("doc");
    await expect.poll(() => stripMessage(page)).toBe(toCheck(5, key));
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("n from the guard walks on from where it landed", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await expect.poll(() => stripPlace(page)).toBe("3 of 5");
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("a click on Approve with stops to check is the first press, and a later click approves", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    const button = page.locator(".file-header birta-turn button");
    await button.click();
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    expect(seenIds(mock)).toEqual([]);
    expect(opsNamed(mock, "approve")).toEqual([]);
    await page.waitForTimeout(DWELL_MS + 200);
    await button.click();
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
  });

  test("an open question is to check: the first ⌘⏎ goes to the first open blank, a later one approves", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      for (const c of s.changes) c.seen = true;
      for (const i of s.items) for (const r of i.replies ?? []) if (r.by.type === "agent") r.seen = true;
      s.counts.unseen = 0;
      s.counts.drafts = 0;
      // Everything of Claude's checked but question #21
      check(s, s.news.filter((n: State) => n.check == null && n.id !== "#21").map((n: State) => n.id), 8);
    });
    const key = await hint(page);
    await expect.poll(() => turnOf(page)).toEqual({ text: "Approve", kbd: true, disabled: false });
    await press(page);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(1, key));
    // #22 above it is answered, so the first open one is #21 under ## Why; a blank is its own stop
    expect(await page.evaluate(() => {
      const a = document.activeElement;
      return a?.getAttribute("data-kind") === "blank" ? (a.shadowRoot?.querySelector(".text bdi")?.textContent ?? "") : null;
    })).toBe("is 503 the only code?");
    expect(await page.evaluate(() =>
      Array.from(document.querySelectorAll('birta-flow[data-kind="blank"]')).some((h) => !!h.shadowRoot?.querySelector("textarea")),
    )).toBe(false);
    await press(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    await page.waitForTimeout(DWELL_MS);
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
    expect(seenIds(mock)).toEqual([]);
  });

  test("with unseen changes and open questions, there is one stop, and the second ⌘⏎ approves", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => {
      changesOnly(s);
      s.counts.open_asks = 1;
    });
    await press(page);
    await expect.poll(() => stripMessage(page)).toMatch(/^4 to check · n next · /);
    await page.waitForTimeout(DWELL_MS + 200);
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
  });

  test("the jump focuses the row of the change it lands on", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", changesOnly);
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await expect
      .poll(() =>
        page.evaluate(() => {
          const a = document.activeElement;
          return a && a.localName === "birta-flow" ? (a.shadowRoot?.textContent ?? "") : null;
        }),
      )
      .toMatch(/↳ Claude · done/);
  });

  test("the guard goes to the first stop to check, not the next one after a walk", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    for (let i = 0; i < 4; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    await press(page);
    // Walking checked nothing, so 9.0's paragraph is still the first stop to check
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
  });

  test("the guard passes over a checked stop and stops at a not-handled row: it is to check too", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => {
      changesOnly(s);
      s.changes.find((c: State) => c.id === "9.0").seen = true;
      s.counts.unseen -= 1;
      check(s, ["9.0", "#4"], 1);
    });
    const key = await hint(page);
    await press(page);
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await expect.poll(() => stripMessage(page)).toBe(toCheck(3, key));
    expect(seenIds(mock)).toEqual([]);
  });

  for (const key of ["n", "Escape"]) {
    test(`${key} after the jump disarms it too`, async ({ page, birta }) => {
      const { mock } = await openFixture(page, birta, "unseen", changesOnly);
      await press(page);
      await expect.poll(() => focused(page)).toBe("5:1-6:68");
      await page.waitForTimeout(DWELL_MS + 200);
      await page.keyboard.press(key);
      await press(page);
      await page.waitForTimeout(SETTLE_MS);
      expect(opsNamed(mock, "approve")).toEqual([]);
      await expect.poll(() => stripMessage(page)).toMatch(/^4 to check · n next · /);
    });
  }

  test("a click that moves focus leaves the arm standing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.waitForTimeout(DWELL_MS + 200);
    await page.locator('#content [data-sourcepos="3:1-3:6"]').click();
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toHaveLength(1);
  });

  test("inside the approve's hold ⌘⏎ is inert, and the pill stays", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved", (s) => (s.hold.not_before = s.server_now + 20_000));
    await expect.poll(() => stripMessage(page)).toBe("approving · z to undo");
    await press(page);
    await page.locator(".file-header birta-turn button").click();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect(await stripMessage(page)).toBe("approving · z to undo");
  });

  test("new news pushed while armed disarms it: the next ⌘⏎ jumps again", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen", changesOnly);
    const key = await hint(page);
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    // An outside write lands while the guard is armed, on 10.0's bullet: a new item, though no new stop
    const next = JSON.parse(JSON.stringify(state));
    next.changes.push({ ...next.changes.find((c: State) => c.id === "10.0"), id: "20.0", op: 20, attr: "outside", turn: null });
    const at = next.news.findIndex((n: State) => n.id === "10.0");
    next.news.splice(at + 1, 0, { id: "20.0", kind: "change", sp: "11:1-11:44", check: null });
    next.counts.unseen += 1;
    for (const k of Object.keys(state)) delete state[k];
    Object.assign(state, next);
    mock.pushState(state);
    await expect
      .poll(() => page.evaluate(() => (window as unknown as { BR: { currentState(): State } }).BR.currentState().news.some((n: { id: string }) => n.id === "20.0")))
      .toBe(true);
    expect(await turnOf(page)).toEqual(APPROVE);
    await page.waitForTimeout(DWELL_MS + 200);
    await press(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(4, key));
  });

  test("a pill with no stop to jump to never approves silently: it pills and arms in place", async ({ page, birta }) => {
    // The count says 4 to check, yet every item is checked: the walk has nowhere to land
    const { mock } = await openFixture(page, birta, "unseen", (s) => {
      changesOnly(s);
      for (const c of s.changes) c.seen = true;
      for (const n of s.news) if (n.check == null) n.check = n.kind === "change" || n.kind === "removal" ? "kept" : "ok";
    });
    const key = await hint(page);
    await press(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    expect(seenIds(mock)).toEqual([]);
    expect(await stripMessage(page)).toBe(toCheck(4, key));
    await page.waitForTimeout(DWELL_MS);
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
  });

  test("a held ⌘⏎'s repeats neither approve nor disarm", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    const key = await hint(page);
    await press(page);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.waitForTimeout(DWELL_MS + 200);
    const mac = key === "⌘⏎";
    for (let i = 0; i < 3; i++) {
      await dispatchKey(page, { key: "Enter", code: "Enter", metaKey: mac, ctrlKey: !mac, repeat: true }, "body");
    }
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    expect(await stripMessage(page)).toBe(toCheck(4, key));
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
  });

  test("a held verdict key acts once", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "fresh");
    // Past whole doc, whose verdict keys only say what to do, and the title
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await expect.poll(async () => focused(page)).toBe((await stops(page))[1].sp);
    await page.keyboard.press("y");
    for (let i = 0; i < 3; i++) await dispatchKey(page, { key: "y", code: "KeyY", repeat: true }, "body");
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "comment")).toHaveLength(1);
  });

  test("a double-click on Approve with stops to check is two presses inside the dwell: it jumps and never approves", async ({
    page,
    birta,
  }) => {
    const { mock } = await openFixture(page, birta, "unseen", changesOnly);
    await page.locator(".file-header birta-turn button").dblclick();
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    expect(seenIds(mock)).toEqual([]);
    expect(await focused(page)).toBe("5:1-6:68");
  });

  test("a hand-back is not guarded: with notes waiting and changes unseen, ⌘⏎ hands back", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => (s.counts.drafts = 1));
    await expect.poll(() => turnOf(page).then((t) => t?.text)).toBe("Hand back · 1");
    await press(page);
    await expect.poll(() => opsNamed(mock, "hand_back")).toHaveLength(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
    expect((await stripMessage(page)) ?? "").not.toMatch(/approves (anyway|as is)/);
  });
});

// --- On a real server ------------------------------------------------------------------------

test.describe("approve guard on a real server", () => {
  test("a burst inside the dwell never approves, and the jump writes no seen", async ({ page, birta }) => {
    const s = seededRound(1);
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => turnOf(page)).toEqual(APPROVE);
    expect(await toCheckCount(page)).toBe(2);
    const key = await hint(page);
    await press(page);
    const t0 = Date.now();
    // Whole doc, with the done on your note, then the changed paragraph
    await expect.poll(() => stripMessage(page)).toBe(toCheck(2, key));
    while (Date.now() - t0 < DWELL_MS - 250) {
      await press(page);
      await page.waitForTimeout(100);
    }
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)!.filter((l) => l.op === "approve")).toEqual([]);
    await page.waitForTimeout(Math.max(0, t0 + DWELL_MS + 250 - Date.now()));
    await press(page);
    await expect.poll(() => reviewLog(served.root)!.filter((l) => l.op === "approve").length).toBe(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)!.filter((l) => l.op === "approve").length).toBe(1);
    expect(reviewLog(served.root)!.filter((l) => l.op === "seen")).toEqual([]);
  });

  test("one ⌘⏎ jumps and checks nothing, the next approves as is, and the hold says so", async ({ page, birta }) => {
    const s = seededRound();
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => turnOf(page)).toEqual(APPROVE);
    expect(await toCheckCount(page)).toBe(3);
    const key = await hint(page);
    await press(page);
    // Whole doc with the done on your note, and the two changed blocks
    await expect.poll(() => stripMessage(page)).toBe(toCheck(3, key));
    await expect.poll(() => focused(page)).toBe("doc");
    await page.waitForTimeout(DWELL_MS + 200);
    expect(reviewLog(served.root)!.filter((l) => l.op === "seen")).toEqual([]);
    await press(page);
    await expect.poll(() => reviewLog(served.root)!.some((l) => l.op === "approve")).toBe(true);
    await expect.poll(() => stripMessage(page)).toBe("approving · z to undo");
    expect(reviewLog(served.root)!.filter((l) => l.op === "seen")).toEqual([]);
  });
});

test.describe("reopen after approve on a real server", () => {
  test("a note written after approve shows as a draft, the phrase says reopened, and hand-back pills", async ({ page, birta }) => {
    const s = seededRound(1);
    const ts = Date.now() - 60_000;
    const human = { type: "human", via: "browser" };
    // The round's one change, seen, and then the approve
    const seen = { by: human, op: "seen", ids: ["7.0"], ts };
    const approve = { by: human, op: "approve", rev: revOf(s.doc), not_before: ts, hold_ms: 0, ts };
    const log = s.log + [seen, approve].map((l) => JSON.stringify(l) + "\n").join("");
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": log }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await page.keyboard.press("c");
    await expect.poll(() => composerText(page)).toContain("new round · note on whole doc");
    expect((await composerText(page))!.split("new round").length).toBe(2);
    await page.keyboard.type("one more pass on the title");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)!.filter((l) => l.op === "comment").length).toBe(2);
    await expect.poll(() => phraseOf(page)).toBe(`approved ${hhmm(ts)} · reopened`);
    await expect.poll(() => turnOf(page)).toEqual({ text: "Hand back · 1", kbd: true, disabled: false });
    await expect.poll(() => shownHosts(page)).toContain("note");
    await press(page);
    await expect.poll(() => stripMessage(page)).toBe("handing back · 1 note · z to undo");
  });

  test("after approve as is and a reopen, Claude's change takes no keys, and y on its block says keep as is", async ({ page, birta }) => {
    const s = seededRound(1);
    const ts = Date.now() - 60_000;
    const human = { type: "human", via: "browser" };
    // The round's one change was never checked
    const approve = { by: human, op: "approve", rev: revOf(s.doc), not_before: ts, hold_ms: 0, ts };
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log + JSON.stringify(approve) + "\n" }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await page.keyboard.press("c");
    await page.keyboard.type("one more pass on the title");
    await page.keyboard.press("Enter");
    await expect.poll(() => phraseOf(page)).toBe(`approved ${hhmm(ts)} · reopened`);
    const sp = (await stops(page)).find((x) => x.sp.startsWith("5:1-"))!.sp;
    await focusOn(page, sp);
    await expect.poll(async () => (await changeRows(page)).filter((r) => r.sp === sp).length).toBe(1);
    expect((await changeRows(page)).filter((r) => r.sp === sp)[0].kbd).toEqual([]);
    const before = reviewLog(served.root)!.length;
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.slice(before).map((l) => [l.op, l.verdict])).toEqual([["comment", "yes"]]);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)!.filter((l) => l.op === "seen")).toEqual([]);
  });
});

test.describe("approve guard on a real server: text that lands after the jump", () => {
  test("a change written after the jump disarms: the next ⌘⏎ jumps again", async ({ page, birta }) => {
    const s = seededRound(1);
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => turnOf(page)).toEqual(APPROVE);
    expect(await toCheckCount(page)).toBe(2);
    const key = await hint(page);
    await press(page);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(2, key));
    served.write("pr.md", s.doc.replace("- metrics dashboard", "- metrics dashboard, per region"));
    // The outside write is news on a block of its own
    await expect
      .poll(() => page.evaluate(() => (window as unknown as { BR: { currentState(): State } }).BR.currentState().counts.to_check))
      .toBe(3);
    await page.waitForTimeout(DWELL_MS + 200);
    await press(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)!.filter((l) => l.op === "approve")).toEqual([]);
    await expect.poll(() => stripMessage(page)).toBe(toCheck(3, key));
    expect(reviewLog(served.root)!.filter((l) => l.op === "seen")).toEqual([]);
  });
});

// --- The clean view after approve ------------------------------------------------------------

/** approved.json with nothing waiting for later, so its phrase is the bare form. */
function approved(s: State) {
  s.approve.later = 0;
}

test.describe("clean view", () => {
  test("the phrase says how to start again, and the button is spent", async ({ page, birta }) => {
    const { state } = await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toBe(`approved ${hhmm(state.approve.ts)} · c to reopen`);
    expect(await turnOf(page)).toEqual({ text: "Approved ✓", kbd: false, disabled: true });
  });

  test("what waits for later stays out of the phrase and goes to its tooltip", async ({ page, birta }) => {
    const { state } = await openFixture(page, birta, "approved");
    await expect.poll(() => phraseOf(page)).toBe(`approved ${hhmm(state.approve.ts)} · c to reopen`);
    expect(await page.locator(".file-header birta-phrase").getAttribute("title")).toBe(
      `approved ${hhmm(state.approve.ts)} · 1 later · c to reopen`,
    );
  });

  test("a doc changed since the approve stays clean and can be approved again", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "approved", (s) => {
      approved(s);
      s.approve.changed_since = true;
      s.approve.current_matches = false;
    });
    await expect.poll(() => phraseOf(page)).toBe(`approved ${hhmm(state.approve.ts)} · changed since`);
    expect(await turnOf(page)).toEqual({ text: "Approve", kbd: true, disabled: false });
    await expect.poll(() => shownHosts(page)).toEqual([]);
    await press(page);
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
  });

  test("the review draws nothing: no rows, marks, dots or quote underline", async ({ page, birta }) => {
    await openFixture(page, birta, "approved", (s) => {
      approved(s);
      // An unsettled change too, so a bar and a wash would be drawn outside the clean view
      s.changes[0].seen = false;
      s.changes[0].settled = false;
    });
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await expect.poll(() => shownHosts(page)).toEqual([]);
    expect(await laneMarks(page)).toEqual([]);
    expect(await dotsOn(page)).toEqual([]);
    expect(await highlighted(page)).toEqual([]);
  });

  test("⏎ on a block that holds a note opens nothing and sends nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    for (let i = 0; i < 4 && (await focused(page)) !== "5:1-6:68"; i++) await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(await shownHosts(page)).toEqual([]);
    expect(await dotsOn(page)).toEqual([]);
    expect(await highlighted(page)).toEqual([]);
    expect(mock.ops()).toEqual([]);
  });

  test("⏎ in the approved view leaves the note folded for when c reopens the round", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    for (let i = 0; i < 4 && (await focused(page)) !== "5:1-6:68"; i++) await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    mock.pushState({ ...state, counts: { ...state.counts, drafts: 1 } });
    await expect.poll(() => phraseOf(page)).toMatch(/reopened$/);
    await expect.poll(() => dotsOn(page)).toContain("5:1-6:68");
  });

  test("a note written after approve brings the earlier notes' underline back", async ({ page, birta }) => {
    await openFixture(page, birta, "approved", (s) => {
      approved(s);
      s.counts.drafts = 1;
    });
    await expect.poll(() => phraseOf(page)).toMatch(/reopened$/);
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
  });

  for (const where of ["on a block", "at the doc"] as const) {
    test(`y x l . u n p ${where} say how to reopen and send nothing`, async ({ page, birta }) => {
      const { mock } = await openFixture(page, birta, "approved", approved);
      await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
      if (where === "on a block") {
        for (let i = 0; i < 3; i++) await page.keyboard.press("j");
        await expect.poll(async () => focused(page)).toBe((await stops(page))[2].sp);
      } else await expect.poll(() => focused(page)).toBe("doc");
      const was = await focused(page);
      for (const key of ["y", "x", "l", ".", "u", "n", "p"]) {
        await page.evaluate(() => {
          // Each key must put the pill up itself
          (window as unknown as { BR: { chrome: { say(t: string): void } } }).BR.chrome.say("");
        });
        await page.keyboard.press(key);
        await expect.poll(() => stripMessage(page), { message: key }).toBe(REOPEN);
      }
      await page.waitForTimeout(SETTLE_MS);
      expect(mock.ops()).toEqual([]);
      expect(await focused(page)).toBe(was);
      expect(await shownHosts(page)).toEqual([]);
    });
  }

  for (const where of ["on a block", "at the doc"] as const) {
    test(`q ${where} opens a new round's question, as c opens a note`, async ({ page, birta }) => {
      const { mock } = await openFixture(page, birta, "approved", approved);
      await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
      if (where === "on a block") await focusOn(page, "3:1-3:6");
      await page.keyboard.press("q");
      await expect.poll(() => composerLines(page)).toEqual({
        label: where === "on a block" ? "new round · question on ## Why (section)" : "new round · question on whole doc",
        target: "",
      });
      expect(await stripMessage(page)).not.toBe(REOPEN);
      await page.keyboard.type("is this final?");
      await page.keyboard.press("Enter");
      await expect.poll(() => opsNamed(mock, "comment")).toHaveLength(1);
      expect(opsNamed(mock, "comment")[0]).toMatchObject({ verdict: "ask", body: "is this final?" });
    });
  }

  test("z after the approve's hold has nothing to undo and sends nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await page.keyboard.press("z");
    await expect.poll(() => stripMessage(page)).toBe("nothing to undo");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("⌥u still toggles the source view, and u shows no source but says how to reopen", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe(REOPEN);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.locator("#source-view")).toBeHidden();
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#source-view")).toBeVisible();
    await expect.poll(() => stripText(page)).toBe("source view · esc back");
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#lane-frame")).toBeVisible();
  });

  test("⌥u on a changed block still only toggles the source view, u there shows none, and neither writes", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved", (s) => {
      approved(s);
      s.changes[0].seen = false;
      s.changes[0].settled = false;
    });
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await focusOn(page, "5:1-6:68");
    await page.keyboard.press("u");
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.locator("#source-view")).toBeHidden();
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#lane-frame")).toBeVisible();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    // The round trip draws the marks again, and the clean view still holds
    expect(await laneMarks(page)).toEqual([]);
    expect(await shownHosts(page)).toEqual([]);
  });

  test("j and k still move", async ({ page, birta }) => {
    await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await page.keyboard.press("j");
    await expect.poll(async () => focused(page)).toBe((await stops(page))[0].sp);
    const first = await focused(page);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).not.toBe(first);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe(first);
  });

  test("c at the doc opens a new round's composer, and keeping it sends a doc note", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await page.keyboard.press("c");
    await expect.poll(() => composerText(page)).toContain("new round · note on whole doc");
    // The label prints once: the target line under it stays empty until a typed quote fills it
    expect(await composerLines(page)).toEqual({ label: "new round · note on whole doc", target: "" });
    expect((await composerText(page))!.split("new round").length).toBe(2);
    await page.keyboard.type("one more pass on the title");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment")).toHaveLength(1);
    expect(opsNamed(mock, "comment")[0]).toMatchObject({ at: { k: "doc" }, body: "one more pass on the title" });
  });

  test("c on a focused block opens a new round on that block", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await focusOn(page, "3:1-3:6");
    await page.keyboard.press("c");
    await expect.poll(() => composerText(page)).toContain("new round");
    await page.keyboard.type("why is this its own section?");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment")).toHaveLength(1);
    expect(opsNamed(mock, "comment")[0]).toMatchObject({ at: { k: "block", sp: "3:1-3:6" } });
  });

  test("a new round's label names kind and scope as every label does: a section, or a block's first words", async ({ page, birta }) => {
    await openFixture(page, birta, "approved", approved);
    await expect.poll(() => phraseOf(page)).toMatch(/c to reopen$/);
    await focusOn(page, "3:1-3:6");
    await page.keyboard.press("c");
    await expect.poll(() => composerLines(page)).toEqual({ label: "new round · note on ## Why (section)", target: "" });
    await page.keyboard.press("Escape");
    await expect.poll(() => composerLines(page)).toBeNull();
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.keyboard.press("c");
    // How many words a label keeps is open; the kind, the scope and the cut are not
    await expect.poll(() => composerLines(page)).toEqual({
      label: expect.stringMatching(/^new round · note on When the upload \S.*… \(paragraph\)$/),
      target: "",
    });
  });

  test("inside the approve's hold the page is not clean yet, and z withdraws", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "approved", (s) => {
      approved(s);
      s.hold.not_before = s.server_now + 20_000;
    });
    await expect.poll(() => stripMessage(page)).toBe("approving · z to undo");
    expect(await phraseOf(page)).not.toMatch(/c to reopen/);
    await expect.poll(async () => (await shownHosts(page)).length).toBeGreaterThan(0);
    await expect.poll(async () => (await laneMarks(page)).length).toBeGreaterThan(0);
    await page.keyboard.press("z");
    await expect.poll(() => opsNamed(mock, "withdraw")).toEqual([expect.objectContaining({ to: state.hold.n })]);
    // The words for it are not in the spec; it must say something, and neither of these
    await expect
      .poll(async () => {
        const m = await stripMessage(page);
        return m !== null && m !== "nothing to undo" && m !== "approving · z to undo";
      })
      .toBe(true);
  });

  test("the page cleans itself up once the hold ends, with no new state", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved", (s) => {
      approved(s);
      s.hold.not_before = s.server_now + 3_000;
    });
    await expect.poll(() => stripMessage(page)).toBe("approving · z to undo");
    await expect.poll(async () => (await shownHosts(page)).length).toBeGreaterThan(0);
    expect(await highlighted(page)).toEqual(["we just fail"]);
    await expect.poll(() => phraseOf(page), { timeout: 8_000 }).toMatch(/c to reopen$/);
    await expect.poll(() => shownHosts(page)).toEqual([]);
    expect(await laneMarks(page)).toEqual([]);
    await expect.poll(() => highlighted(page)).toEqual([]);
    expect(await dotsOn(page)).toEqual([]);
    // The keys follow the hold's end too
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe(REOPEN);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("inside the hold the keys still walk, and c is no new round", async ({ page, birta }) => {
    await openFixture(page, birta, "approved", (s) => {
      approved(s);
      s.hold.not_before = s.server_now + 20_000;
    });
    await expect.poll(() => stripMessage(page)).toBe("approving · z to undo");
    // The approve already settled the round's news, so j moves and n has nothing left, not the clean view's reopen
    await focusTo(page, "10:1-10:16");
    await page.keyboard.press("n");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).not.toBe(REOPEN);
    expect(await focused(page)).toBe("10:1-10:16");
    await page.keyboard.press("c");
    await expect.poll(() => composerText(page)).not.toBeNull();
    expect(await composerText(page)).not.toContain("new round");
  });
});
