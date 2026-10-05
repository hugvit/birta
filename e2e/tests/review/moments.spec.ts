import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  Mock,
  STOPS_DOC,
  State,
  booted,
  fixtureDoc,
  fixtureState,
  focusTo,
  focused,
  marks,
  mockReview,
  say,
  stopSp,
  stops,
  stripMessage,
  turnOf,
} from "./harness";

// Feedback moments: every message a press or an arrival puts in the strip, and how long it stays.

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;

const SENT = "sent · you can keep reading";
const APPROVED = "approved ✓ · this text is final · nothing is published without you";
const REOPENED = "reopened · hand back when ready · z keeps it approved";
const TIP = "• a settled note · ⏎ opens it";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

interface W {
  BR: { currentState(): State };
}

/** Serves `doc` as pr.md with its socket mocked to `state`, and waits for that state. */
async function open(page: Page, birta: Start, doc: string, state?: State): Promise<{ mock: Mock; url: string }> {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  if (state) await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
  // A hello answered late would send its state after one the spec pushes
  await expect.poll(() => mock.answered).toBe(true);
  return { mock, url: served.url };
}

/** Committed fixture `name`, bent by `edit`. */
function bent(name: string, edit?: (s: State) => void): State {
  const s = fixtureState(name);
  edit?.(s);
  return s;
}

const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"] textarea');

/** Writes `body` in the composer `key` opens on the focused stop, and keeps it with ⏎. */
async function write(page: Page, body: string, key = "c") {
  await page.keyboard.press(key);
  await expect(composer(page)).toBeFocused();
  // A new note, never an edit of a draft on the stop
  await expect(composer(page)).toHaveValue("");
  await page.keyboard.type(body);
  await page.keyboard.press("Enter");
  await expect(composer(page)).toHaveCount(0);
}

/** Waits for `want` in the strip, and returns when it was first seen. */
async function shows(page: Page, want: string, timeout?: number): Promise<number> {
  await expect.poll(() => stripMessage(page), { timeout, intervals: [50] }).toBe(want);
  return Date.now();
}

/** `want`, first seen at `t0`, still shows at `there` ms and is gone by `gone` ms. */
async function lasts(page: Page, want: string, t0: number, there: number, gone: number) {
  const wait = t0 + there - Date.now();
  if (wait > 0) await page.waitForTimeout(wait);
  expect(await stripMessage(page), `still there at ${there} ms`).toBe(want);
  await expect
    .poll(() => stripMessage(page), { message: `gone by ${gone} ms`, timeout: Math.max(1, t0 + gone - Date.now()), intervals: [100] })
    .not.toBe(want);
}

/** A plain message: there at 2.5 s, gone by 5.5 s, so neither 6 s nor 2 s passes. */
const fourSeconds = (page: Page, want: string, t0: number) => lasts(page, want, t0, 2_500, 5_500);
/** A moment: there at 5 s, gone by 8 s. */
const sixSeconds = (page: Page, want: string, t0: number) => lasts(page, want, t0, 5_000, 8_000);

/** Marks the page hidden or shown, as the browser does when the tab goes to the back or returns. */
function visible(page: Page, shown: boolean) {
  return page.evaluate((shown) => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => (shown ? "visible" : "hidden") });
    Object.defineProperty(document, "hidden", { configurable: true, get: () => !shown });
    document.dispatchEvent(new Event("visibilitychange"));
  }, shown);
}

/** The replied fixture as it stood before the reply: picked up, nothing to check. */
function beforeReply(): State {
  const picked = fixtureState("picked");
  return bent("replied", (s) => {
    s.phrase = picked.phrase;
    s.counts = { ...s.counts, to_check: 0, attention: 0, unseen: 0, not_handled: 0 };
    s.news = [];
    s.changes = [];
    s.deletions = [];
    s.rounds = picked.rounds;
    for (const i of s.items) i.status = "picked_up";
  });
}

const bars = async (page: Page) => (await marks(page)).filter((m) => m.mark === "bar" && m.width > 0);
const dotsOn = async (page: Page) =>
  (await marks(page))
    .filter((m) => m.mark === "dot" && m.width > 0)
    .map((m) => m.sp!)
    .sort();

/** Moves focus to `sp` with j or k. */
const jTo = (page: Page, sp: string) => focusTo(page, sp);

// --- Saves --------------------------------------------------------------------------------------

test.describe("every save says so by kind", () => {
  test("every note saved says note saved, the second as well as the first, for 4 s", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("drafts"), fixtureState("drafts"));
    // Headings that hold none of the fixture's drafts
    await jTo(page, "3:1-3:6");
    await write(page, "first");
    await shows(page, "note saved · z to undo");
    await jTo(page, await stopSp(page, "What changes"));
    await write(page, "second");
    const t0 = await shows(page, "note saved · z to undo");
    await fourSeconds(page, "note saved · z to undo", t0);
  });

  test("an edited note says note updated", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), fixtureState("drafts"));
    await jTo(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet");
    await composer(page).fill("drop this bullet now");
    await page.keyboard.press("Enter");
    const t0 = await shows(page, "note updated · z to undo");
    expect(mock.ops().map((o) => o.op)).toEqual(["amend"]);
    await fourSeconds(page, "note updated · z to undo", t0);
  });

  test("a question says question saved", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await write(page, "is 3 enough?", "q");
    await shows(page, "question saved · z to undo");
    expect(mock.ops()[0]).toMatchObject({ op: "comment", verdict: "ask" });
  });
});

// --- The hand-back and the approve --------------------------------------------------------------

test.describe("the end of a hold", () => {
  test("the hand-back's hold ending says sent for 4 s, and focus stays where it was", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("hold"), bent("hold", (s) => (s.hold.not_before = s.server_now + 2_500)));
    const sp = (await stops(page))[2].sp;
    await jTo(page, sp);
    await expect.poll(() => stripMessage(page)).toMatch(/^handing back · /);
    const t0 = await shows(page, SENT, 6_000);
    expect(await focused(page)).toBe(sp);
    await fourSeconds(page, SENT, t0);
    expect(await focused(page)).toBe(sp);
  });

  test("a page opened after the hold ended says nothing of it", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("sent"), fixtureState("sent"));
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
  });

  test("the approve's hold ending is the approved moment: 6 s, Approved ✓, and the bars fade to dots", async ({ page, birta }) => {
    await open(
      page,
      birta,
      fixtureDoc("approved"),
      bent("approved", (s) => {
        s.hold.not_before = s.server_now + 2_500;
        // An unsettled change, so a bar shows until the clean view
        s.changes[0].seen = false;
        s.changes[0].settled = false;
      }),
    );
    await expect.poll(() => stripMessage(page)).toBe("approving · z to undo");
    const t0 = await shows(page, APPROVED, 6_000);
    await expect.poll(async () => (await turnOf(page)) ?? "").toMatch(/^Approved ✓/);
    expect(await bars(page)).toEqual([]);
    expect(await dotsOn(page)).toEqual(expect.arrayContaining(["10:1-10:16", "5:1-6:68"]));
    // No tip during the moment, even when focus lands on a dot
    await jTo(page, "5:1-6:68");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBe(APPROVED);
    await sixSeconds(page, APPROVED, t0);
  });

  test("a page opened on an approve already past its hold has no moment", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("approved"), bent("approved", (s) => (s.hold.not_before = s.server_now - 1_000)));
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
  });

  test("a note after the approve reopens it, and says so for 6 s", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("approved"), bent("approved", (s) => (s.hold.not_before = s.server_now - 1_000)));
    await jTo(page, "3:1-3:6");
    await write(page, "one more pass");
    const t0 = await shows(page, REOPENED);
    expect(mock.ops().map((o) => o.op)).toEqual(["comment"]);
    await sixSeconds(page, REOPENED, t0);
  });
});

// --- Claude's reply, in view and on the way back ------------------------------------------------

test.describe("Claude's reply", () => {
  const REPLIED = "Claude replied · 3 to check · n starts at the top";

  test("a reply landing while the tab is in view says so for 6 s, and focus stays", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("replied"), beforeReply());
    const sp = (await stops(page))[1].sp;
    await jTo(page, sp);
    mock.pushState(fixtureState("replied"));
    const t0 = await shows(page, REPLIED);
    expect(await focused(page)).toBe(sp);
    await sixSeconds(page, REPLIED, t0);
    expect(await focused(page)).toBe(sp);
  });

  test("the same reply pushed again, with fewer to check, says nothing new", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("replied"), beforeReply());
    mock.pushState(fixtureState("replied"));
    await shows(page, REPLIED);
    await say(page, "");
    // A keep in another tab: the same reply, one stop fewer
    mock.pushState(bent("replied", (s) => (s.counts.to_check = 2)));
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().counts.to_check)).toBe(2);
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
  });

  test("a reply that landed while the tab was away waits for the tab, then says so for 6 s", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("replied"), beforeReply());
    await visible(page, false);
    mock.pushState(fixtureState("replied"));
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().phrase.stage)).toBe("replied");
    await page.waitForTimeout(1_000);
    expect(await stripMessage(page)).toBeNull();
    await visible(page, true);
    const t0 = await shows(page, REPLIED);
    await sixSeconds(page, REPLIED, t0);
  });

  test("a pickup while the tab was away says still with Claude on the way back", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("sent"), fixtureState("sent"));
    await visible(page, false);
    mock.pushState(fixtureState("picked"));
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().phrase.stage)).toBe("picked_up");
    await visible(page, true);
    const t0 = await shows(page, "still with Claude");
    await sixSeconds(page, "still with Claude", t0);
  });

  test("back to the tab with nothing changed says nothing", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("picked"), fixtureState("picked"));
    await visible(page, false);
    await page.waitForTimeout(SETTLE_MS);
    await visible(page, true);
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
  });
});

// --- Copies -------------------------------------------------------------------------------------

/** Swaps the clipboard for a list the page's copies land in; returns a reader. Call before `goto`. */
async function captureClipboard(page: Page): Promise<() => Promise<string[]>> {
  await page.addInitScript(() => {
    const w = window as unknown as { __copied: string[] };
    w.__copied = [];
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: (text: string) => (w.__copied.push(text), Promise.resolve()) },
    });
  });
  return () => page.evaluate(() => (window as unknown as { __copied: string[] }).__copied);
}

test.describe("copies", () => {
  test("copying the hand-off line from the menu says paste it to Claude", async ({ page, birta }) => {
    const copied = await captureClipboard(page);
    await open(page, birta, fixtureDoc("sent"), fixtureState("sent"));
    await page.locator(".file-header birta-phrase").click();
    await page.locator('birta-pop [role="menuitem"]', { hasText: "copy hand-off line" }).click();
    await expect.poll(copied).toEqual([fixtureState("sent").hand_off]);
    const t0 = await shows(page, "copied · paste it to Claude");
    await fourSeconds(page, "copied · paste it to Claude", t0);
  });

  test("the viewer's copy button says markdown copied", async ({ page, birta }) => {
    const copied = await captureClipboard(page);
    await open(page, birta, STOPS_DOC);
    await page.locator("#copy-md-btn").click();
    await expect.poll(async () => (await copied()).length).toBe(1);
    await shows(page, "markdown copied");
  });
});

// --- The settled-note tip ------------------------------------------------------------------------

test.describe("the settled-note tip", () => {
  const clean = () => bent("approved", (s) => (s.hold.not_before = s.server_now - 1_000));

  test("shows when focus first lands on a dotted block, not when the dots are drawn", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("approved"), clean());
    await expect.poll(async () => (await dotsOn(page)).length).toBeGreaterThan(0);
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
    await jTo(page, "5:1-6:68");
    const t0 = await shows(page, TIP);
    await fourSeconds(page, TIP, t0);
  });

  test("shows once ever: not on the next dot, nor after a reload", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("approved"), clean());
    await jTo(page, "5:1-6:68");
    await shows(page, TIP);
    await say(page, "");
    await jTo(page, "10:1-10:16");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
    await page.reload();
    await booted(page);
    await expect.poll(async () => (await dotsOn(page)).length).toBeGreaterThan(0);
    await jTo(page, "5:1-6:68");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
  });
});
