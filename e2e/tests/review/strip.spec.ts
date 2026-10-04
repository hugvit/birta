import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  State,
  blockBox,
  booted,
  fixtureDoc,
  fixtureState,
  focusMark,
  focusTo,
  focused,
  longDoc,
  mockReview,
  revOf,
  say,
  stopSp,
  strip,
  stripKeys,
  stripMessage,
  stripText,
  stripWhere,
  withRev,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const SHORT = { width: 1280, height: 400 };
const SETTLE_MS = 400;
/** Past the window in which a scroll still counts as the reader's own. */
const QUIET_MS = 300;
/** The x arm's own window on a change row, which an armed message outlasts. */
const CONFIRM_MS = 3_000;

const WHOLE_DOC = "whole doc · c note on the whole doc · j down · ? keys";
const PLAIN_KEYS = "c note · q ask · x cut · y keep as is · ? more";
const CITED = "because you said";
const ARMED = "x = no to this text · u reverts Claude's change · x again for no";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** Serves committed fixture `name` as pr.md with its state, bent by `edit`. */
async function openFixture(page: Page, birta: Start, name: string, edit?: (s: State) => void, viewport = WIDE) {
  const state = fixtureState(name);
  edit?.(state);
  const served = await birta({ "pr.md": fixtureDoc(name) }, GATE_ON);
  await page.setViewportSize(viewport);
  const mock = await mockReview(page, { state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock, state };
}

/** Serves `doc` as pr.md with no review file yet, review on and the socket mocked. */
async function openDoc(page: Page, birta: Start, doc: string, viewport = WIDE) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(viewport);
  const mock = await mockReview(page, { state: withRev(fixtureState("fresh"), revOf(doc)) });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

/** The view a focused stop must sit in: below the sticky header, above the strip. */
function view(page: Page): Promise<{ top: number; bottom: number } | null> {
  return page.evaluate(() => {
    const s = document.querySelector("birta-strip");
    if (!s || !s.getClientRects().length) return null;
    return { top: document.querySelector(".file-header")!.getBoundingClientRect().bottom, bottom: s.getBoundingClientRect().top };
  });
}

/** Where the focus mark sits against the view: "in view", or why not. */
async function focusPlace(page: Page): Promise<string> {
  const v = await view(page);
  if (!v) return "no strip";
  const m = await focusMark(page);
  if (!m) return "no focus mark";
  if (m.top < v.top - 1 || m.bottom > v.bottom + 1) return `focus ${m.top}-${m.bottom}, view ${v.top}-${v.bottom}`;
  return "in view";
}

/** Selects `len` characters of the paragraph holding `has`, from offset `from`, as a drag would. */
async function selectIn(page: Page, has: string, from: number, len: number) {
  await page.evaluate(
    ([has, from, len]) => {
      const p = Array.from(document.querySelectorAll("#content p")).find((e) => e.textContent!.includes(has))!;
      const t = p.firstChild as Text;
      const r = document.createRange();
      r.setStart(t, from);
      r.setEnd(t, from + len);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    },
    [has, from, len] as const,
  );
}

const receipt = (page: Page, has: string) => page.locator('birta-flow[data-kind="receipt"]', { hasText: has });

// --- Where it is, and what it says --------------------------------------------------------------

test.describe("the strip", () => {
  test("a review tab opens with the strip fixed at the bottom, on whole doc", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh", undefined, SHORT);
    await expect.poll(() => stripText(page)).toBe(WHOLE_DOC);
    const s = (await strip(page))!;
    expect(Math.abs(s.bottom - s.innerHeight), `strip ${s.top}-${s.bottom}`).toBeLessThanOrEqual(1);
    expect(s.left).toBeLessThanOrEqual(1);
    expect(s.right).toBeGreaterThanOrEqual(s.innerWidth - 1);
    expect(s.message).toBe(false);
    expect(s.dim).toBe(false);
    // Slim: one line of text, not a panel
    expect(s.height).toBeGreaterThan(0);
    expect(s.height).toBeLessThanOrEqual(48);
    // Fixed: your scroll leaves it where it is
    await page.mouse.move(640, 200);
    await page.mouse.wheel(0, 300);
    await page.waitForTimeout(SETTLE_MS);
    const after = (await strip(page))!;
    expect(after.top).toBeCloseTo(s.top, 0);
  });

  test("a plain page has no strip", async ({ page, birta }) => {
    const served = await birta({ "pr.md": fixtureDoc("fresh") });
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await expect(page.locator("#content")).toBeVisible();
    await page.waitForTimeout(SETTLE_MS);
    expect(await strip(page)).toBeNull();
  });

  test("before review shows (no state, no drafter), the strip shows only to carry a selection's hint", async ({ page, birta }) => {
    const served = await birta({ "pr.md": fixtureDoc("fresh") }, GATE_ON);
    await page.setViewportSize(WIDE);
    await mockReview(page);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await strip(page)).toBeNull();
    await selectIn(page, "When the upload", 9, 6);
    await expect.poll(() => stripKeys(page)).toBe("c note · q ask · x cut these words");
    await page.keyboard.press("Escape");
    await expect.poll(() => strip(page)).toBeNull();
    // A letter no binding uses says nothing on a page that shows no review
    await page.keyboard.press("h");
    await page.waitForTimeout(SETTLE_MS);
    expect(await strip(page)).toBeNull();
  });

  test("before review shows, a mode's way back is a plain 4 s message, not a bar for the whole mode", async ({ page, birta }) => {
    const served = await birta({ "pr.md": fixtureDoc("fresh") }, GATE_ON);
    await page.setViewportSize(WIDE);
    await mockReview(page);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await strip(page)).toBeNull();
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#source-view")).toBeVisible();
    await expect.poll(() => stripMessage(page)).toBe("source view · esc back");
    const t0 = Date.now();
    // A message names no place before review shows
    expect(await stripWhere(page)).toBe("");
    await expect.poll(() => strip(page), { timeout: 3_000 + 4_000 }).toBeNull();
    expect(Date.now() - t0).toBeGreaterThanOrEqual(3_000);
    await expect(page.locator("#source-view")).toBeVisible();
  });

  test("the viewer's status toast shows above the strip, not under it", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await expect.poll(() => stripText(page)).toBe(WHOLE_DOC);
    await page.evaluate(() => {
      const el = document.getElementById("status")!;
      el.textContent = "changed since · try again";
      el.classList.add("visible");
    });
    const toast = (await page.locator("#status").boundingBox())!;
    const s = (await strip(page))!;
    expect(toast.height).toBeGreaterThan(0);
    expect(toast.y + toast.height, `toast ${toast.y}-${toast.y + toast.height}, strip at ${s.top}`).toBeLessThanOrEqual(s.top);
  });

  test("on a block it names the section and the block, and gives the block's keys", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await focusTo(page, await stopSp(page, "When the upload"));
    await expect.poll(() => stripText(page)).toBe(`## Why › paragraph · ${PLAIN_KEYS}`);
    await focusTo(page, await stopSp(page, "retries on 503"));
    await expect.poll(() => stripText(page)).toBe(`## What changes › list item · ${PLAIN_KEYS}`);
    // k back to whole doc
    for (let i = 0; i < 12 && (await focused(page)) !== "doc"; i++) await page.keyboard.press("k");
    await expect.poll(() => stripText(page)).toBe(WHOLE_DOC);
  });

  test("a block with your note", async ({ page, birta }) => {
    await openFixture(page, birta, "drafts");
    await focusTo(page, "5:1-6:54");
    await expect.poll(() => stripText(page)).toBe("## Why › paragraph · your note · c edit note · z undo · ? more");
  });

  test("whole doc and a block holding Claude's news show where they sit in the walk", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    await expect.poll(() => stripText(page)).toBe("1 of 5 · whole doc · Claude replied · y ok · c reply · n next");
    // Reached by j, not by n: the place comes from the focus
    await focusTo(page, "5:1-6:68");
    await expect
      .poll(() => stripText(page))
      .toBe("2 of 5 · ## Why › paragraph · Claude's change · y keep · u revert · c reply · n next");
    await focusTo(page, "10:1-10:16");
    await expect.poll(() => stripText(page)).toBe("3 of 5 · ## What changes › list item · Claude replied · y ok · c reply · n next");
    // A block with none of Claude's news has no place in the walk
    await focusTo(page, await stopSp(page, "metrics dashboard"));
    await expect.poll(() => stripWhere(page)).toBe("## What changes › list item");
  });

  test("with Claude, the strip says so on every stop", async ({ page, birta }) => {
    await openFixture(page, birta, "sent");
    await focusTo(page, "5:1-6:54");
    await expect.poll(() => stripText(page)).toBe("## Why › paragraph · with Claude · c note for next hand-back · ? keys");
    await focusTo(page, "10:1-10:16");
    await expect.poll(() => stripText(page)).toBe("## What changes › list item · with Claude · c note for next hand-back · ? keys");
  });

  test("approved", async ({ page, birta }) => {
    await openFixture(page, birta, "approved");
    await expect.poll(() => stripText(page)).toBe("approved · c reopen · ? keys");
    await focusTo(page, "5:1-6:68");
    await expect.poll(() => stripText(page)).toBe("approved · c reopen · ? keys");
  });

  test("source view and reading mode name their exit; reading mode dims the strip", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await focusTo(page, await stopSp(page, "When the upload"));
    await page.keyboard.press("Alt+KeyU");
    await expect.poll(() => stripText(page)).toBe("source view · esc back");
    expect((await strip(page))!.message).toBe(false);
    expect((await strip(page))!.dim).toBe(false);
    await page.keyboard.press("Escape");
    await expect.poll(() => stripText(page)).toBe(`## Why › paragraph · ${PLAIN_KEYS}`);
    await page.keyboard.press("r");
    await expect.poll(() => stripText(page)).toBe("reading mode · esc back");
    await expect.poll(async () => (await strip(page))?.dim).toBe(true);
    expect((await strip(page))!.message).toBe(false);
    await page.keyboard.press("Escape");
    await expect.poll(() => stripText(page)).toBe(`## Why › paragraph · ${PLAIN_KEYS}`);
    expect((await strip(page))!.dim).toBe(false);
  });

  test("a selection's hint is the keys side, not a message", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await selectIn(page, "When the upload", 9, 6);
    await expect.poll(() => stripKeys(page)).toBe("c note · q ask · x cut these words");
    expect((await strip(page))!.message).toBe(false);
    // The selection's block holds focus
    expect(await stripWhere(page)).toBe("## Why › paragraph");
    await page.keyboard.press("Escape");
    await expect.poll(() => stripKeys(page)).toBe(PLAIN_KEYS);
  });
});

// --- Messages ----------------------------------------------------------------------------------

test.describe("messages", () => {
  test("a message replaces the keys for 4 s, then the keys come back", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await expect.poll(() => stripText(page)).toBe(WHOLE_DOC);
    const before = (await strip(page))!;
    await page.keyboard.press("l");
    await expect.poll(() => stripMessage(page)).toBe("later needs a block · j");
    const t0 = Date.now();
    const during = (await strip(page))!;
    expect(during.where).toBe("whole doc");
    expect(during.message).toBe(true);
    // It never covers the doc: the strip keeps its place and size
    expect(during.top).toBeCloseTo(before.top, 0);
    expect(during.height).toBeCloseTo(before.height, 0);
    await page.waitForTimeout(Math.max(0, 3_000 - (Date.now() - t0)));
    expect(await stripMessage(page)).toBe("later needs a block · j");
    await expect.poll(() => stripMessage(page), { timeout: 3_000 }).toBeNull();
    expect(Date.now() - t0).toBeGreaterThanOrEqual(3_500);
    expect(await stripText(page)).toBe(WHOLE_DOC);
  });

  test("no pill shows a message anywhere but the strip", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await page.keyboard.press("l");
    await expect.poll(() => stripMessage(page)).toBe("later needs a block · j");
    const elsewhere = await page.evaluate(() => {
      const out: string[] = [];
      for (const host of Array.from(document.querySelectorAll("body *"))) {
        if (host.localName === "birta-strip") continue;
        const root = host.shadowRoot;
        if (!root) continue;
        root.querySelectorAll('[role="status"], .pill').forEach((el) => {
          if (el.getClientRects().length && (el.textContent ?? "").includes("later needs a block")) out.push(host.localName);
        });
      }
      return out;
    });
    expect(elsewhere).toEqual([]);
  });

  test("a moment's message lasts 6 s", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await expect.poll(() => stripText(page)).toBe(WHOLE_DOC);
    await say(page, "reconnected · nothing lost", { ms: 6_000 });
    const t0 = Date.now();
    await expect.poll(() => stripMessage(page)).toBe("reconnected · nothing lost");
    await page.waitForTimeout(Math.max(0, 5_000 - (Date.now() - t0)));
    expect(await stripMessage(page)).toBe("reconnected · nothing lost");
    await expect.poll(() => stripMessage(page), { timeout: 3_000 }).toBeNull();
    expect(await stripKeys(page)).toBe("c note on the whole doc · j down · ? keys");
  });

  test("a new message replaces the old one, and runs its own time", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await say(page, "first message");
    await expect.poll(() => stripMessage(page)).toBe("first message");
    await page.waitForTimeout(2_500);
    await say(page, "second message");
    const t0 = Date.now();
    await expect.poll(() => stripMessage(page)).toBe("second message");
    // Past the first one's 4 s, the second still shows
    await page.waitForTimeout(Math.max(0, 3_000 - (Date.now() - t0)));
    expect(await stripMessage(page)).toBe("second message");
    await expect.poll(() => stripMessage(page), { timeout: 3_000 }).toBeNull();
  });

  test("an armed message stays until the next key, past its arm's window and past 4 s", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusTo(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe(ARMED);
    await page.waitForTimeout(Math.max(CONFIRM_MS, 4_000) + 600);
    expect(await stripMessage(page)).toBe(ARMED);
    expect(mock.ops().filter((o) => o.op === "comment")).toEqual([]);
    await page.keyboard.press("j");
    await expect.poll(() => stripMessage(page), { timeout: 500 }).toBeNull();
    expect((await strip(page))!.keys).not.toBe(ARMED);
  });

  test("the arm lasts as long as its message: x again past 4 s says no", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusTo(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe(ARMED);
    await page.waitForTimeout(Math.max(CONFIRM_MS, 4_000) + 600);
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().filter((o) => o.op === "comment").map((o) => o.verdict)).toEqual(["no"]);
    await expect.poll(() => stripMessage(page)).not.toBe(ARMED);
  });

  test("say with no text clears the message", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await say(page, "first message");
    await expect.poll(() => stripMessage(page)).toBe("first message");
    await say(page, "");
    await expect.poll(() => stripMessage(page), { timeout: 500 }).toBeNull();
    expect(await stripText(page)).toBe(WHOLE_DOC);
  });

  test("Esc on a stop leaves a plain message to run out", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await page.keyboard.press("l");
    await expect.poll(() => stripMessage(page)).toBe("later needs a block · j");
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBe("later needs a block · j");
  });
});

// --- Never under the strip ---------------------------------------------------------------------

test.describe("the view ends at the strip", () => {
  test("j and k keep every focused block between the header and the strip", async ({ page, birta }) => {
    await openDoc(page, birta, longDoc(), SHORT);
    for (let i = 0; i < 16; i++) {
      await page.keyboard.press("j");
      await page.waitForTimeout(60);
      await expect.poll(() => focusPlace(page), { message: `j ×${i + 1}` }).toBe("in view");
    }
    await page.keyboard.press("End");
    await expect.poll(() => focusPlace(page), { message: "End" }).toBe("in view");
    for (let i = 0; i < 6; i++) {
      await page.keyboard.press("k");
      await expect.poll(() => focusPlace(page), { message: `k ×${i + 1}` }).toBe("in view");
    }
  });

  test("j onto a block the strip covers brings it fully above the strip", async ({ page, birta }) => {
    await openDoc(page, birta, longDoc(), SHORT);
    const n = await stopSp(page, "Filler paragraph 9.");
    const next = await stopSp(page, "Filler paragraph 10.");
    await focusTo(page, n);
    // A scroll from script, so focus stays on n while the next block ends just inside the window, under the strip
    await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      window.scrollBy(0, el.getBoundingClientRect().bottom - (innerHeight - 2));
    }, next);
    await page.waitForTimeout(QUIET_MS);
    expect(await focused(page)).toBe(n);
    const s = (await strip(page))!;
    expect(s, "a strip").not.toBeNull();
    const under = (await blockBox(page, next))!;
    expect(under.bottom, "setup: the next block ends under the strip").toBeGreaterThan(s.top);
    expect(under.bottom).toBeLessThanOrEqual(SHORT.height);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(next);
    await page.waitForTimeout(SETTLE_MS);
    expect((await blockBox(page, next))!.bottom).toBeLessThanOrEqual((await strip(page))!.top);
    expect(await focusPlace(page)).toBe("in view");
  });

  test("your scroll that leaves the focus under the strip moves it to the first stop in view", async ({ page, birta }) => {
    await openDoc(page, birta, longDoc(), SHORT);
    const sp = await stopSp(page, "Filler paragraph 20.");
    await focusTo(page, sp);
    // Page-made: the focused block sits mid-view, and focus stays
    await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      window.scrollBy(0, el.getBoundingClientRect().top - 200);
    }, sp);
    await page.waitForTimeout(QUIET_MS);
    expect(await focused(page)).toBe(sp);
    const s = (await strip(page))!;
    expect(s, "a strip").not.toBeNull();
    // Your wheel moves it down until its top is under the strip, still inside the window. The sticky
    // header changes height as the page scrolls (WebKit lands short), so the wheel aims again from where it got to
    const want = s.top + Math.min(4, (s.innerHeight - s.top) / 2);
    await page.mouse.move(640, 150);
    for (let i = 0; i < 3; i++) {
      const top = (await blockBox(page, sp))!.top;
      if (top >= s.top && top < s.innerHeight) break;
      await page.mouse.wheel(0, -(want - top));
      await page.waitForTimeout(SETTLE_MS);
    }
    const b = (await blockBox(page, sp))!;
    expect(b.top, "setup: the block's top is under the strip").toBeGreaterThanOrEqual(s.top);
    expect(b.top).toBeLessThan(s.innerHeight);
    await expect.poll(() => focused(page)).not.toBe(sp);
    expect(await focusPlace(page)).toBe("in view");
  });
});

// --- Narrow widths ---------------------------------------------------------------------------

test.describe("narrow", () => {
  test("keys drop from the right, and ? keys always stays", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await expect.poll(async () => (await strip(page))?.shownKeys).toEqual(["c note on the whole doc", "j down", "? keys"]);
    const wide = (await strip(page))!;
    for (const width of [420, 340, 300, 260]) {
      await page.setViewportSize({ width, height: 700 });
      await page.waitForTimeout(SETTLE_MS);
      const s = (await strip(page))!;
      expect(s.shownKeys.at(-1), `${width}px: ${s.shownKeys.join(" · ")}`).toBe("? keys");
      // What's left is the start of the list: keys drop from the right
      const rest = s.shownKeys.slice(0, -1);
      expect(["c note on the whole doc", "j down"].slice(0, rest.length), `${width}px`).toEqual(rest);
      expect(s.overflows, `${width}px: nothing cut mid-word`).toBe(false);
      expect(s.right, `${width}px`).toBeLessThanOrEqual(s.innerWidth + 1);
      expect(s.height, `${width}px: still one line`).toBeLessThanOrEqual(wide.height + 1);
    }
    const narrowest = (await strip(page))!;
    expect(narrowest.shownKeys.length, "at 260 px something has dropped").toBeLessThan(3);
  });

  test("on a block at a narrow width the ? key stays last", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await focusTo(page, await stopSp(page, "When the upload"));
    await page.setViewportSize({ width: 300, height: 700 });
    await page.waitForTimeout(SETTLE_MS);
    const s = (await strip(page))!;
    expect(s, "a strip").not.toBeNull();
    expect(s.shownKeys.at(-1)).toMatch(/^\? /);
    expect(s.shownKeys.length).toBeLessThan(5);
    expect(s.overflows).toBe(false);
  });
});

// --- Adversary -------------------------------------------------------------------------------

/** Fixture `name`'s doc with filler after it, so the page scrolls; every sp of the fixture holds. */
async function openLong(page: Page, birta: Start, name: string, viewport: { width: number; height: number }) {
  const doc = fixtureDoc(name) + Array.from({ length: 30 }, (_, i) => `\nFiller paragraph ${i + 1}.\n`).join("");
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(viewport);
  const mock = await mockReview(page, { state: withRev(fixtureState(name), revOf(doc)) });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

test.describe("adversary: messages", () => {
  test("a plain message outlasts a j: only its time ends it, and where follows the focus under it", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await expect.poll(() => stripText(page)).toBe(WHOLE_DOC);
    await page.keyboard.press("l");
    await expect.poll(() => stripMessage(page)).toBe("later needs a block · j");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).not.toBe("doc");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBe("later needs a block · j");
    expect(await stripWhere(page)).not.toBe("whole doc");
  });

  test("a message on a walk stop keeps the place on the left", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    await page.keyboard.press("n");
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await expect.poll(() => stripWhere(page)).toBe("2 of 5 · ## Why › paragraph · Claude's change");
    await say(page, "note saved · z to undo");
    await expect.poll(() => stripMessage(page)).toBe("note saved · z to undo");
    expect(await stripWhere(page)).toBe("2 of 5 · ## Why › paragraph · Claude's change");
  });

  test("the same message said again is a new message and runs its own time", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await say(page, "later needs a block · j");
    await expect.poll(() => stripMessage(page)).toBe("later needs a block · j");
    await page.waitForTimeout(2_500);
    await say(page, "later needs a block · j");
    const t0 = Date.now();
    // Past the first one's 4 s, the second still shows
    await page.waitForTimeout(Math.max(0, 3_000 - (Date.now() - t0)));
    expect(await stripMessage(page)).toBe("later needs a block · j");
  });
});

test.describe("adversary: the view ends at the strip", () => {
  // A draft opens no row on landing, so only land()'s own reveal can lift it clear of the strip.
  // The paragraph draft's box is the element's own, so no nested note lifts it either.
  test("n onto a draft the strip covers brings it fully above the strip", async ({ page, birta }) => {
    await openLong(page, birta, "drafts", { width: 1280, height: 180 });
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("doc");
    const height = page.viewportSize()!.height;
    // Aimed by blockBox, the box the assertions read; the sticky header shrinks as the page scrolls, so aim twice
    for (let i = 0; i < 2; i++) {
      const b = (await blockBox(page, "5:1-6:54"))!;
      await page.evaluate((d) => window.scrollBy(0, d), b.bottom - (height - 2));
      await page.waitForTimeout(QUIET_MS);
    }
    expect(await focused(page)).toBe("doc");
    const s = (await strip(page))!;
    expect(s, "a strip").not.toBeNull();
    const under = (await blockBox(page, "5:1-6:54"))!;
    expect(under.bottom, "setup: the draft's block ends under the strip").toBeGreaterThan(s.top);
    expect(under.bottom).toBeLessThanOrEqual(s.innerHeight);
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("5:1-6:54");
    await page.waitForTimeout(SETTLE_MS);
    expect((await blockBox(page, "5:1-6:54"))!.bottom).toBeLessThanOrEqual((await strip(page))!.top);
    expect(await focusPlace(page)).toBe("in view");
  });
});
