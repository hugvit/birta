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
  focusTo,
  focusTop,
  focused,
  focusMark,
  mockReview,
  revOf,
  say,
  spAt,
  strip,
  stripMessage,
  stripPlace,
  stripWhere,
  withRev,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
/** How long a first x on a many-hunk change waits for the second. */
const CONFIRM_MS = 3_000;

const END = "no more · p back";
const at = (i: number, n: number) => `${i} of ${n}`;


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

/** The stops still holding unchecked news, as the fold counts `to_check`. */
function toCheck(s: State): number {
  const stop = (n: State) => (n.kind === "removal" ? `r:${n.id}` : n.kind === "question" ? `q:${n.id}` : (n.sp ?? "doc"));
  return new Set(s.news.filter((n: State) => n.check == null).map(stop)).size;
}

/** Checks news items `ids` in `s`, as the fold does after a seen naming them. */
function check(s: State, ids: string[]) {
  for (const id of ids) {
    const n = s.news.find((x: State) => x.id === id);
    if (n && n.check == null) n.check = n.kind === "change" || n.kind === "removal" ? "kept" : "ok";
    const c = s.changes.find((x: State) => x.id === id);
    if (c && !c.seen) {
      c.seen = true;
      c.check = "kept";
      s.counts.unseen -= 1;
    }
  }
  s.counts.to_check = toCheck(s);
  s.counts.attention = s.counts.to_check;
}

/** What the server pushes once it has kept `seen` for `ids`. */
function echoSeen(state: State, ids: string[]): State {
  const next = JSON.parse(JSON.stringify(state));
  check(next, ids);
  return next;
}

/** Checks every news item, as y on every stop would. */
function allSeen(s: State) {
  check(s, s.news.map((n: State) => n.id));
}

/** A hand-back after `s` settles what was checked: it leaves news, all but `keep`. */
function settle(s: State, keep: string[] = []) {
  s.news = s.news.filter((n: State) => keep.includes(n.id));
  for (const c of s.changes) c.settled = !keep.includes(c.id);
  s.counts.to_check = toCheck(s);
  s.counts.attention = s.counts.to_check;
}

const seenIds = (mock: Mock) => mock.ops().filter((o) => o.op === "seen").map((o) => o.ids);
const opsNamed = (mock: Mock, name: string) => mock.ops().filter((o) => o.op === name);

const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"]');
const receipt = (page: Page, has: string) => page.locator('birta-flow[data-kind="receipt"]', { hasText: has });

/** The removal 11.0 as `stopOf` names it: a stop of its own, in the gap after `## Stacked on`. */
const REMOVAL = "removal 11.0 after 14:1-14:13";

/** Where focus is: the block's sp, `doc`, or `removal <change> after <sp>` on a removal stop. */
async function stopOf(page: Page): Promise<string | null> {
  const m = await focusMark(page);
  if (m?.target === "removal") return `removal ${m.change} after ${m.after}`;
  return focused(page);
}

// unseen.json's news walks, in doc order: the doc-level reply and the turn note, the paragraph's
// change and its done, the not-handled bullet, the next bullet's change, and the removal at its own
// stop in the gap below the last heading. `ids` are every item on the stop, as y names them.
const UNSEEN_WALK: { ids: string[]; focus: string | null }[] = [
  { ids: ["#13", "#14"], focus: "doc" },
  { ids: ["#4", "9.0"], focus: "5:1-6:68" },
  { ids: ["nh:5"], focus: "10:1-10:16" },
  { ids: ["10.0"], focus: "11:1-11:44" },
  { ids: ["11.0"], focus: REMOVAL },
];

/** Each seen op's ids, sorted, so the order a stop lists them in is free. */
const seenSets = (mock: Mock) => seenIds(mock).map((ids) => [...ids].sort());

// --- One pass in doc order -------------------------------------------------------------------

test.describe("the n walk", () => {
  test("n walks every stop top to bottom, every kind interleaved, and the strip says where it is", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    for (const [i, stop] of UNSEEN_WALK.entries()) {
      await page.keyboard.press("n");
      await expect.poll(() => stripPlace(page), { message: `step ${i + 1}` }).toBe(at(i + 1, UNSEEN_WALK.length));
      expect(await stopOf(page), `step ${i + 1}`).toBe(stop.focus);
    }
    // Landing never checks: nothing at all is sent
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("the count holds as each stop is checked with y: checked stops stay in the walk", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen");
    const sent: string[][] = [];
    for (const [i, stop] of UNSEEN_WALK.entries()) {
      await page.keyboard.press("n");
      await expect.poll(() => stripPlace(page), { message: `step ${i + 1}` }).toBe(at(i + 1, UNSEEN_WALK.length));
      await page.keyboard.press("y");
      sent.push([...stop.ids].sort());
      await expect.poll(() => seenSets(mock), { message: `step ${i + 1}` }).toEqual(sent);
      swap(mock, state, echoSeen(state, stop.ids));
      await page.waitForTimeout(100);
    }
    expect(await stopOf(page)).toBe(REMOVAL);
    await expect.poll(() => stripPlace(page)).toBe(at(5, 5));
  });

  test("n past the last stop stays there and says so; it never wraps", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await expect.poll(() => stripPlace(page)).toBe(at(5, 5));
    const before = seenIds(mock).length;
    await page.keyboard.press("n");
    await expect.poll(() => stripMessage(page)).toBe(END);
    await page.keyboard.press("n");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBe(END);
    expect(await stopOf(page)).toBe(REMOVAL);
    expect(seenIds(mock)).toHaveLength(before);
  });

  test("after the end, Esc keeps the focus; k back to whole doc and n go on from there", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await page.keyboard.press("n");
    await expect.poll(() => stripMessage(page)).toBe(END);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stopOf(page)).toBe(REMOVAL);
    await focusTop(page);
    expect(await focused(page)).toBe("doc");
    await page.keyboard.press("n");
    await expect.poll(() => stripPlace(page)).toBe(at(2, 5));
    expect(await focused(page)).toBe("5:1-6:68");
  });

  for (const [key, want] of [
    ["n", at(5, 5)],
    // The place is the focused stop itself, as if n had landed there, so p goes to the stop before it
    ["p", at(3, 5)],
  ] as const) {
    test(`your scroll moves the focus to the first stop in view, and ${key} goes on from it, never from the top of the doc`, async ({
      page,
      birta,
    }) => {
      await openFixture(page, birta, "unseen", undefined, { width: 1280, height: 320 });
      for (let i = 0; i < 2; i++) await page.keyboard.press("n");
      await expect.poll(() => focused(page)).toBe("5:1-6:68");
      // Wheeled so 10.0's bullet is the first block at or below the header
      const by = await page.evaluate(() => {
        const el = document.querySelector('#content [data-sourcepos="11:1-11:44"]')!;
        const header = document.querySelector(".file-header")!.getBoundingClientRect().bottom;
        return el.getBoundingClientRect().top - header - 2;
      });
      await page.mouse.move(640, 200);
      await page.mouse.wheel(0, by);
      await expect.poll(() => focused(page)).toBe("11:1-11:44");
      await page.waitForTimeout(SETTLE_MS);
      await page.keyboard.press(key);
      await expect.poll(() => stripPlace(page)).toBe(want);
    });
  }

  test("with focus moved off the walk, n goes on from the first stop after the focused block", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    for (let i = 0; i < 2; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    for (let i = 0; i < 8 && (await focused(page)) !== "10:1-10:16"; i++) await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await page.keyboard.press("n");
    await expect.poll(() => stripPlace(page)).toBe(at(4, 5));
    expect(await focused(page)).toBe("11:1-11:44");
  });

  test("a write that replaces the landed block keeps the walk's place on it: n goes on to the next stop", async ({ page, birta }) => {
    const doc = fixtureDoc("unseen");
    // Same length, so every sp holds
    const doc2 = doc.replace("we just fail and the user", "we just fail, and the user").replace("start again.", "start over.");
    const first = fixtureState("unseen");
    const second = withRev(JSON.parse(JSON.stringify(first)), revOf(doc2));
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, { states: [first, second] });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("n");
    await page.keyboard.press("n");
    // 9.0 and its done #4 share the paragraph, one stop of five, landed on together and never checked by it
    await expect.poll(() => stripPlace(page)).toBe(at(2, 5));
    expect(await focused(page)).toBe("5:1-6:68");
    expect(seenIds(mock)).toEqual([]);
    served.write("pr.md", doc2);
    await expect.poll(() => page.evaluate(() => document.getElementById("content")!.textContent!.includes("start over."))).toBe(true);
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    // n goes on to the stop after the paragraph, not to its second item, and not from the top
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await expect.poll(() => stripPlace(page)).toBe(at(3, 5));
  });

  test("p walks back stop by stop, and before the first stop says so; it never wraps", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    for (let i = 0; i < 3; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await page.keyboard.press("p");
    await expect.poll(() => stripPlace(page)).toBe(at(2, 5));
    expect(await focused(page)).toBe("5:1-6:68");
    await page.keyboard.press("p");
    await expect.poll(() => stripPlace(page)).toBe(at(1, 5));
    expect(await focused(page)).toBe("doc");
    await page.keyboard.press("p");
    await expect.poll(() => stripMessage(page)).toBe(END);
    expect(await focused(page)).toBe("doc");
  });

  test("p on a fresh page has nothing behind it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    await page.keyboard.press("p");
    await expect.poll(() => stripMessage(page)).toBe(END);
    expect(await focused(page)).toBe("doc");
    expect(mock.ops()).toEqual([]);
  });

  test("with nothing to walk, n says so and moves nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "fresh");
    await page.keyboard.press("n");
    await expect.poll(() => stripMessage(page)).toBe(END);
    expect(await focused(page)).toBe("doc");
    expect(mock.ops()).toEqual([]);
  });
});

// --- The second pass -------------------------------------------------------------------------

test.describe("a seen round", () => {
  test("with everything seen, n still walks the whole round, and sends no seen again", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", allSeen);
    for (const [i, stop] of UNSEEN_WALK.entries()) {
      await page.keyboard.press("n");
      await expect.poll(() => stripPlace(page), { message: `step ${i + 1}` }).toBe(at(i + 1, UNSEEN_WALK.length));
      expect(await stopOf(page), `step ${i + 1}`).toBe(stop.focus);
    }
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("after a pass, p walks the whole way back", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", allSeen);
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await expect.poll(() => stripPlace(page)).toBe(at(5, 5));
    for (let i = UNSEEN_WALK.length - 2; i >= 0; i--) {
      await page.keyboard.press("p");
      await expect.poll(() => stripPlace(page)).toBe(at(i + 1, 5));
      expect(await focused(page)).toBe(UNSEEN_WALK[i].focus);
    }
  });

  test("what a hand-back settled is not walked; what an earlier round left unchecked is", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", (s) => {
      allSeen(s);
      s.rounds.push({ submit: 40, ts: s.server_now, by: "", to: "*", items: [], released: false });
      const left = s.changes.find((c: State) => c.id === "10.0");
      left.seen = false;
      left.check = null;
      s.news.find((n: State) => n.id === "10.0").check = null;
      s.counts.unseen = 1;
      settle(s, ["10.0"]);
    });
    await page.keyboard.press("n");
    await expect.poll(() => stripPlace(page)).toBe(at(1, 1));
    expect(await focused(page)).toBe("11:1-11:44");
    await page.keyboard.press("n");
    await expect.poll(() => stripMessage(page)).toBe(END);
  });

  test("a new round starts the walk from the top, wherever the last pass ended", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen", allSeen);
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await expect.poll(() => stripPlace(page)).toBe(at(5, 5));
    // The reader hands back, and the agent's next turn rewrites the paragraph again
    const next = JSON.parse(JSON.stringify(state));
    next.rounds.push({ submit: 20, ts: next.server_now, by: "", to: "*", items: [], claim: 21, claimed_by: "claude", yield: 30, released: false });
    settle(next);
    next.changes.push({ ...next.changes.find((c: State) => c.id === "9.0"), id: "25.0", op: 25, turn: 21, seen: false, settled: false, check: null });
    next.news = [{ id: "25.0", kind: "change", sp: "5:1-6:68", check: null }];
    next.counts.unseen = 1;
    next.counts.to_check = next.counts.attention = 1;
    swap(mock, state, next);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("n");
    await expect.poll(() => stripPlace(page)).toBe(at(1, 1));
    expect(await focused(page)).toBe("5:1-6:68");
    // Landing sends nothing; y there checks the new change
    expect(seenIds(mock)).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["25.0"]]);
  });
});

test.describe("killers: the walk", () => {
  test("a new round resets the walk even while the old stop is still in it", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen", (s) => {
      allSeen(s);
      const last = s.changes.find((c: State) => c.id === "11.0");
      last.seen = false;
      last.check = null;
      s.news.find((n: State) => n.id === "11.0").check = null;
      s.counts.unseen = 1;
      s.counts.to_check = s.counts.attention = 1;
    });
    await page.keyboard.press("End");
    for (let i = 0; i < UNSEEN_WALK.length; i++) await page.keyboard.press("n");
    await expect.poll(() => stopOf(page)).toBe(REMOVAL);
    const next = JSON.parse(JSON.stringify(state));
    next.rounds.push({ submit: 20, ts: next.server_now, by: "", to: "*", items: [], claim: 21, claimed_by: "claude", yield: 30, released: false });
    settle(next, ["11.0"]);
    next.changes.push({ ...next.changes.find((c: State) => c.id === "9.0"), id: "25.0", op: 25, turn: 21, seen: false, settled: false, check: null });
    // The new change sits on the paragraph, above the removal in the list's doc order
    next.news.unshift({ id: "25.0", kind: "change", sp: "5:1-6:68", check: null });
    next.counts.unseen = 2;
    next.counts.to_check = next.counts.attention = 2;
    swap(mock, state, next);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("n");
    await expect.poll(() => stripPlace(page)).toBe(at(1, 2));
    expect(await focused(page)).toBe("5:1-6:68");
  });

  test("the lost-notes row is on whole doc's stop: n lands there once, and sends no seen", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "lost");
    const focus: (string | null)[] = [];
    // One n per stop: an n that landed on an item of the stop it was on would leave focus put
    for (let i = 0; i < 3; i++) {
      await page.keyboard.press("n");
      await page.waitForTimeout(SETTLE_MS);
      focus.push(await focused(page));
    }
    // Whole doc (the doc-level reply and the lost row), the paragraph (both changes), the not-handled bullet
    expect(focus).toEqual(["doc", "5:1-6:68", "10:1-10:16"]);
    // The lost row is no news of Claude's, so the count leaves it out: six items on three stops
    await page.keyboard.press("Home");
    await expect.poll(() => stripPlace(page)).toBe(at(1, 3));
    // The walk ends there, and n past it says so
    await page.keyboard.press("End");
    await page.keyboard.press("n");
    await expect.poll(() => stripMessage(page)).toBe(END);
    await page.waitForTimeout(SETTLE_MS);
    // Landing never checks
    expect(seenIds(mock)).toEqual([]);
  });
});

// --- Message lifetime ------------------------------------------------------------------------

/** On an open change row the first x arms a second, which says no to its text. */
const CITED = "because you said";

/** Walks j to `sp`, whose change rows open while it has focus. */
async function openRowOn(page: Page, sp: string) {
  await focusTo(page, sp);
}
const ARMED = "x = no to this text · u reverts Claude's change · x again for no";

test.describe("message lifetime", () => {
  test("an armed message outlasts its arm's window: it stays until the next key, and sends nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await openRowOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe(ARMED);
    await page.waitForTimeout(CONFIRM_MS + 400);
    expect(await stripMessage(page)).toBe(ARMED);
    expect(opsNamed(mock, "comment")).toEqual([]);
    // The arm lives as long as its message, so x again still says no
    await page.keyboard.press("x");
    await expect.poll(() => opsNamed(mock, "comment").map((o) => o.verdict)).toEqual(["no"]);
  });

  test("an armed message clears on any other key, and that key disarms it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await openRowOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe(ARMED);
    await page.keyboard.press("j");
    await expect.poll(() => stripMessage(page), { timeout: 500 }).toBeNull();
    // Back on the receipt inside the old window, x arms again instead of saying no
    await openRowOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe(ARMED);
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "comment")).toEqual([]);
  });

  test("only another key disarms: x straight to the receipt after j arms again", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await openRowOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe(ARMED);
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
    expect(await stripMessage(page)).not.toBe(ARMED);
  });

  test("a message that comes while the composer is open shows in the strip, which keeps its place", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", undefined, { width: 1280, height: 420 });
    for (let i = 0; i < 4; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    await page.keyboard.press("c");
    await expect(composer(page)).toBeVisible();
    await page.evaluate(() => {
      const c = document.querySelector('birta-flow[data-kind="composer"]')!;
      window.scrollBy(0, c.getBoundingClientRect().bottom - innerHeight + 4);
    });
    const before = (await strip(page))!;
    expect(before, "a strip").not.toBeNull();
    expect(before.message).toBe(false);
    await say(page, "note saved · z to undo");
    await expect.poll(() => stripMessage(page)).toBe("note saved · z to undo");
    const during = (await strip(page))!;
    expect(during.where, "the left side still names the composer").toBe(before.where);
    expect([during.top, during.height]).toEqual([before.top, before.height]);
  });

  test("a message never draws over an open composer: nothing but the strip shows it", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", undefined, { width: 1280, height: 420 });
    for (let i = 0; i < 4; i++) await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    await page.keyboard.press("c");
    await expect(composer(page)).toBeVisible();
    await page.evaluate(() => {
      const c = document.querySelector('birta-flow[data-kind="composer"]')!;
      window.scrollBy(0, c.getBoundingClientRect().bottom - innerHeight + 4);
    });
    const before = (await strip(page))!;
    await page.keyboard.type("words");
    await say(page, "copied · paste it to Claude");
    await expect.poll(() => stripMessage(page)).toBe("copied · paste it to Claude");
    const painted = await page.evaluate(() => {
      const out: string[] = [];
      for (const host of Array.from(document.querySelectorAll("body *"))) {
        if (host.localName === "birta-strip" || !host.shadowRoot) continue;
        host.shadowRoot.querySelectorAll("*").forEach((el) => {
          const own = Array.from(el.childNodes).some((n) => n.nodeType === 3 && (n.textContent ?? "").includes("paste it to Claude"));
          if (own && el.getClientRects().length) out.push(host.localName);
        });
      }
      return out;
    });
    expect(painted).toEqual([]);
    const during = (await strip(page))!;
    expect([during.top, during.height]).toEqual([before.top, before.height]);
    await expect(composer(page)).toBeVisible();
  });
});

// --- The walk's place ------------------------------------------------------------------------

const UNSEEN_DOC = fixtureDoc("unseen");
const WHAT = spAt(UNSEEN_DOC, "## What changes");
// The list's last item runs to the blank line after it
const METRICS = "12:1-13:0";

/** A draft note of yours on `sp`, or on whole doc for null, shaped as drafts.json's are. */
function draft(n: number, sp: string | null): State {
  const d = fixtureState("drafts");
  const base = d.items.find((i: State) => (sp ? i.at.scope === "block" : i.at.kind === "doc"));
  return { ...base, n, cid: `c-${n}`, body: `draft ${n}`, status: "draft", at: sp ? { ...base.at, loc: { sp } } : base.at };
}

/** unseen.json picked up for a second turn, with 10.0 left unchecked from the first: the walk is that one change. */
function pickedUp(s: State) {
  allSeen(s);
  s.rounds.push({ submit: 20, ts: s.server_now, by: "", to: "*", items: [], claim: 21, claimed_by: "claude", released: false });
  const left = s.changes.find((c: State) => c.id === "10.0");
  left.seen = false;
  left.check = null;
  s.news.find((n: State) => n.id === "10.0").check = null;
  settle(s, ["10.0"]);
  s.phrase = {
    stage: "picked_up",
    sent: { n: 20, ts: s.server_now, items: 0, can_withdraw: false },
    picked_up: { n: 21, ts: s.server_now, by: "claude" },
  };
  s.counts.unseen = 1;
}

/** Claude's reply to that turn: the round gets its yield, a new reply on whole doc and a new change on the paragraph. */
function repliedAgain(state: State): State {
  const next = JSON.parse(JSON.stringify(state));
  next.rounds[next.rounds.length - 1].yield = 30;
  const was = fixtureState("unseen").phrase;
  next.phrase = { ...was, sent: next.phrase.sent, picked_up: next.phrase.picked_up, replied: { ...was.replied, turn: 21, ts: next.server_now } };
  next.items.find((i: State) => i.n === 6).replies.push({
    n: 26,
    id: "#26",
    by: { type: "agent", name: "claude", via: "cli" },
    kind: "answer",
    body: "Trimmed it.",
    ts: next.server_now,
    seen: false,
  });
  next.changes.push({ ...next.changes.find((c: State) => c.id === "9.0"), id: "25.0", op: 25, turn: 21, seen: false, settled: false, check: null });
  // News in walk order: the reply on whole doc, the new change on the paragraph, then 10.0
  next.news = [
    { id: "#26", kind: "reply", sp: null, check: null },
    { id: "25.0", kind: "change", sp: "5:1-6:68", check: null },
    ...next.news,
  ];
  next.counts.unseen = 2;
  next.counts.to_check = next.counts.attention = 3;
  return next;
}

test.describe("the walk's place", () => {
  test("when Claude replies, the place goes back to before stop 1: the first n lands on whole doc", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen", pickedUp);
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    // The reply keeps the same round (its claim is unchanged) and only adds the yield
    swap(mock, state, repliedAgain(state));
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page), "a reply never moves the focus").toBe("11:1-11:44");
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("doc");
    await expect.poll(() => stripPlace(page)).toBe(at(1, 3));
    // Landing never checks
    expect(seenIds(mock)).toEqual([]);
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
  });

  test("after the reply, a k puts the place at the focus, and n goes on from there, not from the top", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen", pickedUp);
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    swap(mock, state, repliedAgain(state));
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    await expect.poll(() => stripPlace(page)).toBe(at(3, 3));
  });

  test("on a fresh tab, p after j goes back from the focus to the walk stop above it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    await focusTo(page, WHAT);
    await page.keyboard.press("p");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await expect.poll(() => stripPlace(page)).toBe(at(2, 5));
    // Neither j on its way past the paragraph nor p's landing checks it
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
  });

  test("on a fresh tab, n after j goes on from the focus, not from the top", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    await focusTo(page, WHAT);
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await expect.poll(() => stripPlace(page)).toBe(at(3, 5));
  });

  test("End puts the place at the last stop: p goes back from it", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    await page.keyboard.press("End");
    await expect.poll(() => stopOf(page)).toBe(REMOVAL);
    await page.keyboard.press("p");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    await expect.poll(() => stripPlace(page)).toBe(at(4, 5));
  });

  test("p before the first stop says no more · p back, as n past the last does", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("doc");
    await page.keyboard.press("p");
    await expect.poll(() => stripMessage(page)).toBe(END);
    expect(await focused(page)).toBe("doc");
  });
});

test.describe("your drafts in the walk", () => {
  test("n visits your unsent notes in doc order, whole doc first; they have no place in the count", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    const want = ["doc", "5:1-6:54", "10:1-10:16"];
    for (const [i, sp] of want.entries()) {
      await page.keyboard.press("n");
      await expect.poll(() => focused(page), { message: `n ×${i + 1}` }).toBe(sp);
      expect(await stripPlace(page), `n ×${i + 1}`).toBeNull();
    }
    await expect.poll(() => stripWhere(page)).toBe("## What changes › list item · your note");
    await page.keyboard.press("n");
    await expect.poll(() => stripMessage(page)).toBe(END);
    await page.keyboard.press("p");
    await expect.poll(() => focused(page)).toBe("5:1-6:54");
    await expect.poll(() => stripWhere(page)).toBe("## Why › paragraph · your note");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("a draft sits among Claude's news in doc order, and the news count leaves it out", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => {
      s.items.push(draft(30, METRICS));
      s.counts.drafts = 1;
    });
    const want = ["doc", "5:1-6:68", "10:1-10:16", "11:1-11:44", METRICS, REMOVAL];
    for (const [i, sp] of want.entries()) {
      await page.keyboard.press("n");
      await expect.poll(() => stopOf(page), { message: `n ×${i + 1}` }).toBe(sp);
    }
    await expect.poll(() => stripPlace(page)).toBe(at(5, 5));
    await page.keyboard.press("p");
    await expect.poll(() => focused(page)).toBe(METRICS);
    expect(await stripPlace(page)).toBeNull();
    await page.waitForTimeout(SETTLE_MS);
    // Landing, on your own note or on Claude's news, marks nothing seen
    expect(seenIds(mock)).toEqual([]);
  });
});

// --- Adversary: the walk's place -------------------------------------------------------------

test.describe("adversary: the walk's place", () => {
  test("right after Claude's reply the place is before stop 1, so p has nothing behind it", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen", pickedUp);
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    swap(mock, state, repliedAgain(state));
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("p");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page), "p from before stop 1 moves nothing").toBe("11:1-11:44");
    expect(seenIds(mock)).toEqual([]);
    await expect.poll(() => stripMessage(page)).toBe(END);
  });

  for (const [key, want, place] of [
    ["n", "10:1-10:16", at(3, 5)],
    ["p", "5:1-6:68", at(2, 5)],
  ] as const) {
    test(`on a fresh tab, a click puts the place at the clicked block, and ${key} goes on from it`, async ({ page, birta }) => {
      await openFixture(page, birta, "unseen");
      await page.locator("#content h2", { hasText: "What changes" }).click();
      await expect.poll(() => focused(page)).toBe(WHAT);
      await page.keyboard.press(key);
      await expect.poll(() => focused(page)).toBe(want);
      await expect.poll(() => stripPlace(page)).toBe(place);
    });
  }

  test("two changes on one paragraph and the lost row on whole doc: the count is of stops, 1 to 3 of 3", async ({ page, birta }) => {
    await openFixture(page, birta, "lost");
    await expect.poll(() => stripPlace(page)).toBe(at(1, 3));
    await focusTo(page, "5:1-6:68");
    await expect.poll(() => stripPlace(page)).toBe(at(2, 3));
    await focusTo(page, "10:1-10:16");
    await expect.poll(() => stripPlace(page)).toBe(at(3, 3));
  });
});
