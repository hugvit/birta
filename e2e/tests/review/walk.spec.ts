import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  Mock,
  State,
  booted,
  fixtureDoc,
  fixtureState,
  focused,
  mockReview,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
/** How long a first x on a many-hunk change waits for the second. */
const CONFIRM_MS = 3_000;

const END = "no more · j/k to move";
const at = (i: number, n: number) => `${i} of ${n} · n next · p back`;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** Serves committed fixture `name` as pr.md with its state, bent by `edit`. */
async function openFixture(
  page: Page,
  birta: Start,
  name: string,
  edit?: (s: State) => void,
  viewport = WIDE,
) {
  const state = fixtureState(name);
  edit?.(state);
  const served = await birta({ "pr.md": fixtureDoc(name) }, GATE_ON);
  await page.setViewportSize(viewport);
  const mock = await mockReview(page, { state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock, state };
}

/**
 * Turns the mock's own state object into `next` and pushes it. The mock resends that object
 * whenever the server repeats `content`, so a pushed copy alone could be undone.
 */
function swap(mock: Mock, state: State, next: State) {
  const copy = JSON.parse(JSON.stringify(next));
  for (const k of Object.keys(state)) delete state[k];
  Object.assign(state, copy);
  mock.pushState(state);
}

/** What the server pushes once it has kept `seen` for `ids`. */
function echoSeen(state: State, ids: string[]): State {
  const next = JSON.parse(JSON.stringify(state));
  for (const id of ids) {
    const c = next.changes.find((x: State) => x.id === id);
    const r = next.items.flatMap((i: State) => i.replies ?? []).find((x: State) => x.id === id);
    const hit = c ?? r;
    if (hit && !hit.seen) {
      hit.seen = true;
      if (c) next.counts.unseen -= 1;
      next.counts.attention -= 1;
    }
  }
  return next;
}

/** Marks every change, agent reply and not-handled row seen, as walking them all would. */
function allSeen(s: State) {
  for (const c of s.changes) c.seen = true;
  for (const i of s.items) {
    for (const r of i.replies ?? []) if (r.by.type === "agent") r.seen = true;
    if (i.status === "not_handled") i.seen = true;
  }
  s.counts.attention -= s.counts.unseen;
  s.counts.unseen = 0;
}

const seenIds = (mock: Mock) => mock.ops().filter((o) => o.op === "seen").map((o) => o.ids);
const opsNamed = (mock: Mock, name: string) => mock.ops().filter((o) => o.op === name);

/** The pill's text while it is open in the top layer and painted, else null. */
function pillOf(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    for (const host of document.querySelectorAll("birta-pop")) {
      const root = host.shadowRoot ?? host;
      for (const el of root.querySelectorAll('[role="status"]')) {
        let open = false;
        try {
          open = el.matches(":popover-open") && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
        } catch {
          open = false;
        }
        const text = (el.textContent ?? "").replace(/\s+/g, " ").trim();
        if (open && text) return text;
      }
    }
    return null;
  });
}

/** The painted pill's box, or null while none shows. */
function pillBox(page: Page): Promise<{ left: number; top: number; right: number; bottom: number } | null> {
  return page.evaluate(() => {
    for (const host of document.querySelectorAll("birta-pop")) {
      const root = host.shadowRoot ?? host;
      for (const el of root.querySelectorAll('[role="status"]')) {
        let open = false;
        try {
          open = el.matches(":popover-open") && getComputedStyle(el).visibility !== "hidden";
        } catch {
          open = false;
        }
        const r = el.getBoundingClientRect();
        if (open && r.width > 0 && r.height > 0 && (el.textContent ?? "").trim())
          return { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
      }
    }
    return null;
  });
}

const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"]');
const receipt = (page: Page, has: string) => page.locator('birta-flow[data-kind="receipt"]', { hasText: has });

// unseen.json walks, in doc order: the doc-level reply, the paragraph's change, the not-handled
// bullet, the next bullet's change, and the removal walked at the block above its tick
const UNSEEN_WALK: { id: string; focus: string | null; seen: boolean }[] = [
  { id: "#13", focus: null, seen: true },
  { id: "9.0", focus: "5:1-6:68", seen: true },
  { id: "nh:5", focus: "10:1-10:16", seen: false },
  { id: "10.0", focus: "11:1-11:44", seen: true },
  { id: "11.0", focus: "14:1-14:13", seen: true },
];

// --- One pass in doc order -------------------------------------------------------------------

test.describe("the n walk", () => {
  test("n walks every stop top to bottom, every kind interleaved, and pills where it is", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    const sent: string[][] = [];
    for (const [i, stop] of UNSEEN_WALK.entries()) {
      await page.keyboard.press("n");
      if (stop.seen) sent.push([stop.id]);
      await expect.poll(() => pillOf(page), { message: `step ${i + 1}` }).toBe(at(i + 1, UNSEEN_WALK.length));
      await expect.poll(() => seenIds(mock), { message: `step ${i + 1}` }).toEqual(sent);
      expect(await focused(page), `step ${i + 1}`).toBe(stop.focus);
    }
    expect(mock.ops().filter((o) => o.op !== "seen")).toEqual([]);
  });

  test("the count holds as the server echoes each seen: seen stops stay in the walk", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen");
    for (const [i, stop] of UNSEEN_WALK.entries()) {
      await page.keyboard.press("n");
      await expect.poll(() => pillOf(page), { message: `step ${i + 1}` }).toBe(at(i + 1, UNSEEN_WALK.length));
      if (stop.seen) swap(mock, state, echoSeen(state, [stop.id]));
      await page.waitForTimeout(100);
    }
    expect(await focused(page)).toBe("14:1-14:13");
  });

  test("n past the last stop stays there and says so; it never wraps", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(at(5, 5));
    const before = seenIds(mock).length;
    await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(END);
    await page.keyboard.press("n");
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).toBe(END);
    expect(await focused(page)).toBe("14:1-14:13");
    expect(seenIds(mock)).toHaveLength(before);
  });

  test("p walks back stop by stop, and before the first stop says so; it never wraps", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    for (let i = 0; i < 3; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await page.keyboard.press("p");
    await expect.poll(() => pillOf(page)).toBe(at(2, 5));
    expect(await focused(page)).toBe("5:1-6:68");
    await page.keyboard.press("p");
    await expect.poll(() => pillOf(page)).toBe(at(1, 5));
    expect(await focused(page)).toBeNull();
    await page.keyboard.press("p");
    await expect.poll(() => pillOf(page)).toBe(END);
    expect(await focused(page)).toBeNull();
  });

  test("p on a fresh page has nothing behind it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    await page.keyboard.press("p");
    await expect.poll(() => pillOf(page)).toBe(END);
    expect(await focused(page)).toBeNull();
    expect(mock.ops()).toEqual([]);
  });

  test("with nothing to walk, n says so and moves nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "fresh");
    await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(END);
    expect(await focused(page)).toBeNull();
    expect(mock.ops()).toEqual([]);
  });
});

// --- The second pass -------------------------------------------------------------------------

test.describe("a seen round", () => {
  test("with everything seen, n still walks the whole round, and sends no seen again", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", allSeen);
    for (const [i, stop] of UNSEEN_WALK.entries()) {
      await page.keyboard.press("n");
      await expect.poll(() => pillOf(page), { message: `step ${i + 1}` }).toBe(at(i + 1, UNSEEN_WALK.length));
      expect(await focused(page), `step ${i + 1}`).toBe(stop.focus);
    }
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("after a pass, p walks the whole way back", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", allSeen);
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(at(5, 5));
    for (let i = UNSEEN_WALK.length - 2; i >= 0; i--) {
      await page.keyboard.press("p");
      await expect.poll(() => pillOf(page)).toBe(at(i + 1, 5));
      expect(await focused(page)).toBe(UNSEEN_WALK[i].focus);
    }
  });

  test("what a hand-back settled is not walked; what an earlier round left unseen is", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", (s) => {
      allSeen(s);
      s.rounds.push({ submit: 40, ts: s.server_now, by: "", to: "*", items: [], released: false });
      for (const c of s.changes) c.settled = true;
      const left = s.changes.find((c: State) => c.id === "10.0");
      left.seen = false;
      left.settled = false;
      s.counts.unseen = 1;
      s.counts.attention = 1;
    });
    await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(at(1, 1));
    expect(await focused(page)).toBe("11:1-11:44");
    await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(END);
  });

  test("a new round starts the walk from the top, wherever the last pass ended", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen", allSeen);
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(at(5, 5));
    // The reader hands back, and the agent's next turn rewrites the paragraph again
    const next = JSON.parse(JSON.stringify(state));
    next.rounds.push({ submit: 20, ts: next.server_now, by: "", to: "*", items: [], claim: 21, claimed_by: "claude", yield: 30, released: false });
    for (const c of next.changes) c.settled = true;
    next.changes.push({ ...next.changes.find((c: State) => c.id === "9.0"), id: "25.0", op: 25, turn: 21, seen: false, settled: false });
    next.counts.unseen = 1;
    next.counts.attention = 1;
    swap(mock, state, next);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(at(1, 1));
    expect(await focused(page)).toBe("5:1-6:68");
    await expect.poll(() => seenIds(mock).at(-1)).toEqual(["25.0"]);
  });
});

test.describe("killers: the walk", () => {
  test("a new round resets the walk even while the old stop is still in it", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen", (s) => {
      allSeen(s);
      const last = s.changes.find((c: State) => c.id === "11.0");
      last.seen = false;
      s.counts.unseen = 1;
      s.counts.attention = 1;
    });
    await page.keyboard.press("End");
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("14:1-14:13");
    const next = JSON.parse(JSON.stringify(state));
    next.rounds.push({ submit: 20, ts: next.server_now, by: "", to: "*", items: [], claim: 21, claimed_by: "claude", yield: 30, released: false });
    for (const c of next.changes) if (c.id !== "11.0") c.settled = true;
    next.changes.push({ ...next.changes.find((c: State) => c.id === "9.0"), id: "25.0", op: 25, turn: 21, seen: false, settled: false });
    next.counts.unseen = 2;
    next.counts.attention = 2;
    swap(mock, state, next);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe(at(1, 2));
    expect(await focused(page)).toBe("5:1-6:68");
  });

  test("the lost-notes row is a stop: n lands there with no seen", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "lost");
    const pills: string[] = [];
    const focus: (string | null)[] = [];
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press("n");
      await expect.poll(() => pillOf(page), { message: `step ${i + 1}` }).toBe(at(i + 1, 5));
      pills.push((await pillOf(page))!);
      focus.push(await focused(page));
    }
    // The doc-level reply, the lost row, both changes on the paragraph, the not-handled bullet
    expect(focus).toEqual([null, null, "5:1-6:68", "5:1-6:68", "10:1-10:16"]);
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([["#11"], ["9.0"], ["14.0"]]);
  });
});

// --- Pill lifetime ---------------------------------------------------------------------------

/** On an open change row the first x arms a second, which says no to its text. */
const CITED = "because you said";
const ARMED = "x = no to this text · u reverts Claude's change · x again for no";

test.describe("pill lifetime", () => {
  test("an armed pill clears when its window ends", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ARMED);
    await page.waitForTimeout(CONFIRM_MS + 400);
    expect(await pillOf(page)).toBeNull();
    expect(opsNamed(mock, "comment")).toEqual([]);
  });

  test("an armed pill clears on any other key, and that key disarms it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ARMED);
    await page.keyboard.press("j");
    await expect.poll(() => pillOf(page), { timeout: 500 }).toBeNull();
    // Back on the receipt inside the old window, x arms again instead of saying no
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ARMED);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "comment")).toEqual([]);
  });

  test("only another key disarms: x straight to the receipt after j arms again", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ARMED);
    await page.keyboard.press("j");
    // An x the receipt gets with no click or focus change in between
    await page.evaluate((has) => {
      for (const h of document.querySelectorAll('birta-flow[data-kind="receipt"]')) {
        const box = h.shadowRoot?.querySelector(".flow");
        if (!box || !(h.shadowRoot?.textContent ?? "").includes(has)) continue;
        for (const type of ["keydown", "keyup"])
          box.dispatchEvent(new KeyboardEvent(type, { key: "x", code: "KeyX", bubbles: true, composed: true, cancelable: true }));
      }
    }, CITED);
    // j moved the focus off the row, so that x is the focus's own and never the armed no
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "comment")).toEqual([]);
    expect(await pillOf(page)).not.toBe(ARMED);
  });

  test("a pill that comes while the composer is open shows clear of it", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", undefined, { width: 1280, height: 420 });
    for (let i = 0; i < 4; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    await page.keyboard.press("c");
    await expect(composer(page)).toBeVisible();
    await page.evaluate(() => {
      const c = document.querySelector('birta-flow[data-kind="composer"]')!;
      window.scrollBy(0, c.getBoundingClientRect().bottom - innerHeight + 4);
    });
    await page.evaluate(() => (window as unknown as { BR: { chrome: { pill(t: string): void } } }).BR.chrome.pill("note saved · z to undo"));
    await expect.poll(() => pillOf(page)).toBe("note saved · z to undo");
    const box = (await composer(page).boundingBox())!;
    const pill = (await pillBox(page))!;
    const apart =
      pill.right <= box.x || pill.left >= box.x + box.width || pill.bottom <= box.y || pill.top >= box.y + box.height;
    expect(apart, `pill ${JSON.stringify(pill)} over composer ${JSON.stringify(box)}`).toBe(true);
  });

  test("a pill never covers an open composer", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", undefined, { width: 1280, height: 420 });
    for (let i = 0; i < 4; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    // Setup: a pill is up before the composer opens
    await expect.poll(() => pillOf(page)).toBe(at(4, 5));
    await page.keyboard.press("c");
    await expect(composer(page)).toBeVisible();
    // Bring the composer down to where the pill is drawn, bottom centre
    await page.evaluate(() => {
      const c = document.querySelector('birta-flow[data-kind="composer"]')!;
      window.scrollBy(0, c.getBoundingClientRect().bottom - innerHeight + 4);
    });
    const box = (await composer(page).boundingBox())!;
    expect(box.y + box.height, "the composer reaches the pill's row").toBeGreaterThan(420 - 40);
    await page.keyboard.type("words");
    await page.waitForTimeout(SETTLE_MS);
    const pill = await pillBox(page);
    if (pill) {
      const apart =
        pill.right <= box.x || pill.left >= box.x + box.width || pill.bottom <= box.y || pill.top >= box.y + box.height;
      expect(apart, `pill ${JSON.stringify(pill)} over composer ${JSON.stringify(box)}`).toBe(true);
    }
  });
});
