import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  STOPS_DOC,
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
  spAt,
  stopSp,
  stops,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
/** Past the window in which a scroll still counts as the reader's own. */
const QUIET_MS = 300;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, opts: { state?: State } = {}) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state: opts.state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

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

/** The view a stop must sit in: below the sticky header, above the window's bottom. */
function view(page: Page): Promise<{ top: number; bottom: number }> {
  return page.evaluate(() => ({ top: document.querySelector(".file-header")!.getBoundingClientRect().bottom, bottom: innerHeight }));
}

/** Whether the block `sp` lies wholly out of the view. */
async function outOfView(page: Page, sp: string): Promise<boolean> {
  const b = (await blockBox(page, sp))!;
  const v = await view(page);
  return b.bottom <= v.top || b.top >= v.bottom;
}

/** The first stop, in doc order, whose top is at or below the header and inside the window; `doc` for whole doc, which comes first. */
async function firstInView(page: Page): Promise<string | null> {
  const sps = (await stops(page)).map((s) => s.sp);
  return page.evaluate((sps) => {
    const top = document.querySelector(".file-header")!.getBoundingClientRect().bottom - 1;
    // Whole doc's room above the first block, its empty line included while it is unfocused
    const doc = document.querySelector('birta-flow[data-kind="doc-stop"]');
    if (doc && doc.getClientRects().length) {
      const r = doc.getBoundingClientRect();
      if (r.top >= top && r.top < innerHeight) return "doc";
    }
    for (const sp of sps) {
      const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
      const el = all[all.length - 1];
      if (!el || !el.getClientRects().length) continue;
      const r = el.getBoundingClientRect();
      if (r.top >= top && r.top < innerHeight) return sp;
    }
    return null;
  }, sps);
}

/** Waits until the window stops scrolling, so a smooth scroll has landed before anything is read. */
async function scrollSettled(page: Page) {
  let was = -1;
  await expect
    .poll(async () => {
      const y = await page.evaluate(() => scrollY);
      const still = y === was;
      was = y;
      return still;
    }, { intervals: [100] })
    .toBe(true);
}

/** A state for `doc` with a note on each block in `notes` and nothing else, from the drafts fixture. */
function notesState(doc: string, notes: { sp: string; body: string }[]): State {
  const s = fixtureState("drafts");
  const block = s.items.find((i: State) => i.at.scope === "block");
  s.rev = revOf(doc);
  Object.assign(s, { changes: [], deletions: [], lost: [], later: [], rewrite: null });
  s.items = notes.map((note, i) => {
    const n = 40 + i;
    const base = JSON.parse(JSON.stringify(block));
    return { ...base, n, cid: `c-${n}`, body: note.body, status: "draft", at: { ...base.at, scope: "block", loc: { sp: note.sp } } };
  });
  return s;
}

/** The latest round handed back at `submit`, as the hand-back that folds a draft answer in. */
function handedBack(state: State, submit: number): State {
  const round = { ...JSON.parse(JSON.stringify(state.rounds.at(-1))), submit, ts: state.server_now - 60_000, items: [] };
  for (const k of ["claim", "claimed_by", "yield"]) delete round[k];
  return round;
}

// --- Only your own scrolling moves focus ------------------------------------------------------

test.describe("your scroll", () => {
  test("a wheel scroll that takes the focus out of view moves it to the first stop in view", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const why = await stopSp(page, "When the upload");
    await focusTo(page, why);
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, 1500);
    await scrollSettled(page);
    expect(await outOfView(page, why)).toBe(true);
    const want = await firstInView(page);
    expect(want).not.toBeNull();
    await expect.poll(() => focused(page)).toBe(want);
    // It lands without a scroll of its own
    const y = await page.evaluate(() => scrollY);
    await page.waitForTimeout(SETTLE_MS);
    expect(await page.evaluate(() => scrollY)).toBe(y);
    expect(await focused(page)).toBe(want);
  });

  test("a wheel scroll back up moves the focus to the first stop in view again", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    await page.keyboard.press("End");
    const last = (await stops(page)).at(-1)!.sp;
    await expect.poll(() => focused(page)).toBe(last);
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, -100_000);
    await scrollSettled(page);
    expect(await page.evaluate(() => scrollY)).toBe(0);
    await expect.poll(() => focused(page)).toBe(await firstInView(page));
  });

  test("a wheel scroll that leaves the focus in view keeps it", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const sp = await stopSp(page, "Filler paragraph 3.");
    await focusTo(page, sp);
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, 60);
    await scrollSettled(page);
    expect(await outOfView(page, sp)).toBe(false);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(sp);
  });

  for (const key of ["PageDown", "Space", "ArrowDown"]) {
    test(`${key} outside a field scrolls the focus out of view, and the focus follows`, async ({ page, birta }) => {
      await open(page, birta, longDoc());
      const why = await stopSp(page, "Why");
      await focusTo(page, why);
      for (let i = 0; i < 40 && !(await outOfView(page, why)); i++) {
        await page.keyboard.press(key);
        await scrollSettled(page);
      }
      expect(await outOfView(page, why)).toBe(true);
      const want = await firstInView(page);
      expect(want).not.toBeNull();
      await expect.poll(() => focused(page)).toBe(want);
    });
  }

  test("PageUp scrolls back, and the focus follows", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    await page.keyboard.press("End");
    const last = (await stops(page)).at(-1)!.sp;
    await expect.poll(() => focused(page)).toBe(last);
    await page.waitForTimeout(QUIET_MS);
    for (let i = 0; i < 3; i++) await page.keyboard.press("PageUp");
    await scrollSettled(page);
    expect(await outOfView(page, last)).toBe(true);
    await expect.poll(() => focused(page)).toBe(await firstInView(page));
  });
});

// --- Page-made scrolls never move focus -------------------------------------------------------

test.describe("page-made scrolls", () => {
  test("a scrollTo from script keeps the focus, though it leaves the view, and back again", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const why = await stopSp(page, "When the upload");
    await focusTo(page, why);
    await page.waitForTimeout(QUIET_MS);
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await scrollSettled(page);
    expect(await outOfView(page, why)).toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(why);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(why);
  });

  test("a scroll from script right after j is still the page's: j is no scroll key", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const why = await stopSp(page, "When the upload");
    await focusTo(page, await stopSp(page, "Why"));
    await page.waitForTimeout(QUIET_MS);
    await page.keyboard.press("j");
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await page.waitForTimeout(SETTLE_MS);
    expect(await outOfView(page, why)).toBe(true);
    expect(await focused(page)).toBe(why);
  });

  test("a pushed review_state that grows a row above keeps the focus, and its top within 2 px", async ({ page, birta }) => {
    const doc = longDoc();
    const { mock } = await open(page, birta, doc, { state: notesState(doc, []) });
    const above = await stopSp(page, "Filler paragraph 6.");
    const sp = await stopSp(page, "Filler paragraph 9.");
    await focusTo(page, sp);
    // The row lands between the window's top and the focus, where the browser's own anchoring won't hold it
    await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      window.scrollBy(0, el.getBoundingClientRect().top - innerHeight / 2);
    }, sp);
    await page.waitForTimeout(QUIET_MS);
    const before = (await blockBox(page, sp))!.top;
    const body = Array.from({ length: 8 }, (_, i) => `line ${i + 1} of a long note on an earlier block`).join("\n");
    mock.pushState(notesState(doc, [{ sp: above, body }]));
    await expect(page.locator('birta-flow[data-kind="note"]')).toHaveCount(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(sp);
    const after = (await blockBox(page, sp))!.top;
    expect(Math.abs(after - before), `top ${before} → ${after}`).toBeLessThanOrEqual(2);
  });

  test("a doc edit that adds a block above keeps the focus on its block, and its top within 2 px", async ({ page, birta }) => {
    const doc = longDoc();
    const { served } = await open(page, birta, doc);
    const sp = await stopSp(page, "Filler paragraph 9.");
    await focusTo(page, sp);
    await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      window.scrollBy(0, el.getBoundingClientRect().top - innerHeight / 2);
    }, sp);
    await page.waitForTimeout(QUIET_MS);
    const before = (await blockBox(page, sp))!.top;
    const added = Array.from({ length: 6 }, (_, i) => `An added paragraph, number ${i + 1}.`).join("\n\n");
    served.write("pr.md", doc.replace("\nFiller paragraph 7.\n", `\nFiller paragraph 7.\n\n${added}\n`));
    const live = () =>
      page.evaluate(() => {
        for (const el of document.querySelectorAll("#content p[data-sourcepos]"))
          if ((el.textContent ?? "").trim() === "Filler paragraph 9.") return el.getAttribute("data-sourcepos");
        return null;
      });
    await expect.poll(live).not.toBe(sp);
    const moved = (await live())!;
    await expect.poll(() => focused(page)).toBe(moved);
    await page.waitForTimeout(SETTLE_MS);
    const after = (await blockBox(page, moved))!.top;
    expect(Math.abs(after - before), `top ${before} → ${after}`).toBeLessThanOrEqual(2);
  });
});

// --- When a stop goes away --------------------------------------------------------------------

test.describe("a stop that goes away", () => {
  test("a question that folds into its note hands focus to the note's block", async ({ page, birta }) => {
    // Item 22 asks on the title and has a draft answer; a hand-back makes it a settled note
    const state = fixtureState("flow");
    const { mock } = await open(page, birta, fixtureDoc("flow"), { state });
    const blank = page.locator('birta-flow[data-kind="blank"]', { hasText: "keep this title?" });
    await expect(blank).toBeVisible();
    await focusTo(page, "1:1-1:16");
    await page.keyboard.press("j");
    // The blank holds the keyboard, as j gives it
    await expect
      .poll(() => page.evaluate(() => document.activeElement?.getAttribute("data-kind") ?? null))
      .toBe("blank");
    state.rounds.push(handedBack(state, 26));
    mock.pushState(state);
    await expect(blank).toHaveCount(0);
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
  });

  test("a question holding focus marks it as a blank", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("flow"), { state: fixtureState("flow") });
    await focusTo(page, "3:1-3:6");
    await page.keyboard.press("j");
    await expect.poll(() => focusMark(page)).not.toBeNull();
    const m = (await focusMark(page))!;
    expect({ target: m.target, sp: m.sp }).toEqual({ target: "blank", sp: null });
    const host = (await page.locator('birta-flow[data-kind="blank"]', { hasText: "is 503 the only code?" }).boundingBox())!;
    expect(Math.abs(m.top - (host.y - 2)), `mark ${m.top}, blank ${host.y}`).toBeLessThanOrEqual(2);
  });
});

// --- Keys on a stop ---------------------------------------------------------------------------

test.describe("keys on a stop", () => {
  test("Esc on a stop does nothing: focus stays, and nothing is said", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "When the upload");
    await focusTo(page, sp);
    for (let i = 0; i < 2; i++) {
      await page.keyboard.press("Escape");
      await page.waitForTimeout(SETTLE_MS);
      expect(await focused(page)).toBe(sp);
      expect(await pillOf(page)).toBeNull();
    }
    // The next key still acts on that stop
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].at).toStrictEqual({ k: "block", sp, scope: "block" });
  });

  test("End goes to the last stop and Home to whole doc, each brought into view", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const all = await stops(page);
    const inView = async (sp: string) => {
      const b = (await blockBox(page, sp))!;
      const v = await view(page);
      return b.top >= v.top - 1 && b.bottom <= v.bottom + 1;
    };
    // The whole doc line, which shows while it has focus
    const docInView = async () => {
      const m = (await focusMark(page))!;
      const v = await view(page);
      return m.top >= v.top - 1 && m.bottom <= v.bottom + 1;
    };
    await page.keyboard.press("End");
    await expect.poll(() => focused(page)).toBe(all.at(-1)!.sp);
    await scrollSettled(page);
    expect(await inView(all.at(-1)!.sp)).toBe(true);
    await page.keyboard.press("Home");
    await expect.poll(() => focused(page)).toBe("doc");
    await scrollSettled(page);
    expect(await docInView()).toBe(true);
    // From the middle too
    await focusTo(page, await stopSp(page, "Filler paragraph 5."));
    await page.keyboard.press("End");
    await expect.poll(() => focused(page)).toBe(all.at(-1)!.sp);
    await focusTo(page, await stopSp(page, "Filler paragraph 30."));
    await page.keyboard.press("Home");
    await expect.poll(() => focused(page)).toBe("doc");
  });

  test("Home and End in a text field stay the field's", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const sp = await stopSp(page, "Filler paragraph 5.");
    await focusTo(page, sp);
    await page.keyboard.press("c");
    const field = page.locator("textarea");
    await expect(field).toBeFocused();
    await page.keyboard.type("words");
    for (const key of ["Home", "End"]) {
      await page.keyboard.press(key);
      await page.waitForTimeout(SETTLE_MS);
      await expect(field).toBeFocused();
      expect(await focused(page)).toBe(sp);
    }
    await expect(field).toHaveValue("words");
  });

  test("j on the last stop says end of the doc and stays; nothing wraps", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const all = await stops(page);
    await focusTo(page, all.at(-1)!.sp);
    await page.keyboard.press("j");
    await expect.poll(() => pillOf(page)).toBe("end of the doc");
    expect(await focused(page)).toBe(all.at(-1)!.sp);
    await page.keyboard.press("j");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(all.at(-1)!.sp);
  });

  test("k on the first stop says top of the doc and stays; nothing wraps", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const all = await stops(page);
    // The H1 spans the doc, and whole doc is still the stop above it
    await focusTo(page, all[1].sp);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe(all[0].sp);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("doc");
    expect(await pillOf(page)).not.toBe("top of the doc");
    await page.keyboard.press("k");
    await expect.poll(() => pillOf(page)).toBe("top of the doc");
    expect(await focused(page)).toBe("doc");
  });

  test("with no H1 over the doc, k from the first block goes to the doc, and k there says top of the doc", async ({
    page,
    birta,
  }) => {
    await open(page, birta, "Intro words.\n\n" + STOPS_DOC);
    const all = await stops(page);
    await focusTo(page, all[1].sp);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe(all[0].sp);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("doc");
    expect(await pillOf(page)).not.toBe("top of the doc");
    await page.keyboard.press("k");
    await expect.poll(() => pillOf(page)).toBe("top of the doc");
    expect(await focused(page)).toBe("doc");
  });
});

// --- A selection outranks the stop ------------------------------------------------------------

test.describe("a selection", () => {
  test("makes its block the focus, which stays there once the selection goes", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    await focusTo(page, await stopSp(page, "Detail words."));
    await page.evaluate((sp) => {
      const t = document.querySelector(`#content [data-sourcepos="${sp}"]`)!.firstChild as Text;
      const at = t.data.indexOf("upload");
      const r = document.createRange();
      r.setStart(t, at);
      r.setEnd(t, at + "upload".length);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    }, para);
    await expect.poll(() => focused(page)).toBe(para);
    await page.keyboard.press("Escape");
    await expect.poll(() => page.evaluate(() => getSelection()!.toString())).toBe("");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(para);
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].at).toStrictEqual({ k: "block", sp: para, scope: "block" });
  });

  test("a caret is no selection, and moves no focus", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    const detail = await stopSp(page, "Detail words.");
    await focusTo(page, detail);
    await page.evaluate((sp) => {
      const t = document.querySelector(`#content [data-sourcepos="${sp}"]`)!.firstChild as Text;
      const r = document.createRange();
      r.setStart(t, 3);
      r.setEnd(t, 3);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    }, para);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(detail);
  });
});

// --- Adversary: gaps the tests above leave open -----------------------------------------------

test.describe("adversary", () => {
  test("a wheel scroll long done opens no window: a script scroll 500 ms later keeps the focus", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const sp = await stopSp(page, "Filler paragraph 3.");
    await focusTo(page, sp);
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, 60);
    await scrollSettled(page);
    expect(await focused(page)).toBe(sp);
    await page.waitForTimeout(500);
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await scrollSettled(page);
    expect(await outOfView(page, sp)).toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(sp);
  });

  for (const key of ["ArrowUp", "PageUp", "Shift+Space", "ArrowRight"]) {
    test(`${key} that scrolls nothing leaves no user scroll behind for a later script scroll`, async ({ page, birta }) => {
      await open(page, birta, longDoc());
      await page.keyboard.press("j");
      const first = (await stops(page))[0].sp;
      await expect.poll(() => focused(page)).toBe(first);
      expect(await page.evaluate(() => scrollY)).toBe(0);
      await page.keyboard.press(key);
      await page.waitForTimeout(500);
      // The key found nothing to scroll, so no scroll event consumed it
      expect(await page.evaluate(() => [scrollX, scrollY])).toEqual([0, 0]);
      await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
      await scrollSettled(page);
      expect(await outOfView(page, first)).toBe(true);
      await page.waitForTimeout(SETTLE_MS);
      expect(await focused(page)).toBe(first);
    });
  }

  for (const key of ["ArrowUp", "Shift+Space"]) {
    test(`${key} outside a field scrolls up, the focus leaves the view, and it follows`, async ({ page, birta }) => {
      await open(page, birta, longDoc());
      const low = await stopSp(page, "Filler paragraph 30.");
      await focusTo(page, low);
      for (let i = 0; i < 60 && !(await outOfView(page, low)); i++) {
        await page.keyboard.press(key);
        await scrollSettled(page);
      }
      expect(await outOfView(page, low)).toBe(true);
      const want = await firstInView(page);
      expect(want).not.toBeNull();
      await expect.poll(() => focused(page)).toBe(want);
    });
  }

  test("a reply that lands right after your wheel is still held in place", async ({ page, birta }) => {
    const doc = longDoc();
    const { mock } = await open(page, birta, doc, { state: notesState(doc, []) });
    const above = await stopSp(page, "Filler paragraph 6.");
    const sp = await stopSp(page, "Filler paragraph 9.");
    await focusTo(page, sp);
    await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      window.scrollBy(0, el.getBoundingClientRect().top - innerHeight / 2);
    }, sp);
    await page.waitForTimeout(QUIET_MS);
    const before = (await blockBox(page, sp))!.top;
    // Taller than the window, so an uncorrected push takes the focus off screen
    const body = Array.from({ length: 60 }, (_, i) => `line ${i + 1} of a long note`).join("\n");
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, 40);
    mock.pushState(notesState(doc, [{ sp: above, body }]));
    await expect(page.locator('birta-flow[data-kind="note"]')).toHaveCount(1);
    await scrollSettled(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(sp);
    const after = (await blockBox(page, sp))!.top;
    // Only the wheel's own 40 px may move it
    expect(after, `top ${before} → ${after}`).toBeGreaterThanOrEqual(before - 48);
    expect(after, `top ${before} → ${after}`).toBeLessThanOrEqual(before + 2);
    expect(await outOfView(page, sp)).toBe(false);
  });

  test("Esc on a question does nothing: it keeps the keyboard and the focus", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("flow"), { state: fixtureState("flow") });
    await focusTo(page, "3:1-3:6");
    await page.keyboard.press("j");
    const kind = () => page.evaluate(() => document.activeElement?.getAttribute("data-kind") ?? null);
    await expect.poll(kind).toBe("blank");
    await expect.poll(() => focusMark(page)).not.toBeNull();
    const was = (await focusMark(page))!;
    for (let i = 0; i < 2; i++) {
      await page.keyboard.press("Escape");
      await page.waitForTimeout(SETTLE_MS);
      expect(await kind()).toBe("blank");
      const m = await focusMark(page);
      expect(m, "focus mark after Esc").not.toBeNull();
      expect(Math.abs(m!.top - was.top)).toBeLessThanOrEqual(2);
    }
    expect((await focusMark(page))!.target).toBe("blank");
  });

  // The quote belongs to the block the range starts in, whichever way the drag went
  for (const backward of [false, true]) {
    test(`a selection across two blocks, made ${backward ? "backward" : "forward"}, focuses the block its quote is in`, async ({
      page,
      birta,
    }) => {
      const { mock } = await open(page, birta, STOPS_DOC);
      const para = await stopSp(page, "When the upload");
      const head = await stopSp(page, "What changes");
      await focusTo(page, await stopSp(page, "Detail words."));
      await page.evaluate(
        ({ para, head, backward }) => {
          const p = document.querySelector(`#content [data-sourcepos="${para}"]`)!.firstChild as Text;
          const h = document.querySelector(`#content [data-sourcepos="${head}"]`)!;
          const ht = Array.from(h.childNodes).find((n) => n.nodeType === 3 && n.textContent!.trim()) as Text;
          const at = p.data.indexOf("upload");
          const s = getSelection()!;
          s.removeAllRanges();
          if (backward) s.setBaseAndExtent(ht, 4, p, at);
          else s.setBaseAndExtent(p, at, ht, 4);
        },
        { para, head, backward },
      );
      await expect.poll(() => focused(page)).toBe(para);
      await page.keyboard.press("x");
      await expect.poll(() => mock.ops().length).toBe(1);
      expect(mock.ops()[0].at).toMatchObject({ k: "quote", sp: para });
      expect(await focused(page)).toBe(mock.ops()[0].at.sp);
    });
  }

  test("End with a selection drops the selection, so the next verdict lands on the last stop", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    await focusTo(page, await stopSp(page, "Detail words."));
    await page.evaluate((sp) => {
      const t = document.querySelector(`#content [data-sourcepos="${sp}"]`)!.firstChild as Text;
      const at = t.data.indexOf("upload");
      const r = document.createRange();
      r.setStart(t, at);
      r.setEnd(t, at + "upload".length);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    }, para);
    const last = (await stops(page)).at(-1)!.sp;
    await page.keyboard.press("End");
    await expect.poll(() => focused(page)).toBe(last);
    expect(await page.evaluate(() => getSelection()!.toString())).toBe("");
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].at).toStrictEqual({ k: "block", sp: last, scope: "block" });
  });

  test("cutting the last stop while it has focus keeps a focus, on the stop now last", async ({ page, birta }) => {
    const doc = longDoc();
    const { served } = await open(page, birta, doc);
    await focusTo(page, (await stops(page)).at(-1)!.sp);
    served.write("pr.md", doc.replace("\nFiller paragraph 40.\n", ""));
    const live = (text: string) =>
      page.evaluate((text) => {
        for (const el of document.querySelectorAll("#content p[data-sourcepos]"))
          if ((el.textContent ?? "").trim() === text) return el.getAttribute("data-sourcepos");
        return null;
      }, text);
    await expect.poll(() => live("Filler paragraph 40.")).toBeNull();
    const now = (await live("Filler paragraph 39."))!;
    await expect.poll(() => focused(page)).toBe(now);
  });

  test("a cut above and a cut of the focus in one write: focus goes to the next stop below, not the old index", async ({
    page,
    birta,
  }) => {
    const doc = longDoc();
    const { served } = await open(page, birta, doc);
    await focusTo(page, await stopSp(page, "Filler paragraph 9."));
    // One added line keeps any paragraph from starting on the cut block's old line
    const next = doc
      .replace("\nFiller paragraph 3.\n", "\nFiller paragraph 3.\nmore words.\n")
      .replace("\nFiller paragraph 5.\n", "")
      .replace("\nFiller paragraph 9.\n", "");
    served.write("pr.md", next);
    const live = (text: string) =>
      page.evaluate((text) => {
        for (const el of document.querySelectorAll("#content p[data-sourcepos]"))
          if ((el.textContent ?? "").trim() === text) return el.getAttribute("data-sourcepos");
        return null;
      }, text);
    await expect.poll(() => live("Filler paragraph 9.")).toBeNull();
    await expect.poll(() => focused(page)).toBe(await live("Filler paragraph 10."));
  });
});

// --- Repair: each fails without its fix ------------------------------------------------------

/** A state for `doc` holding only Claude's open question on `sp`, or nothing for null. */
function askState(doc: string, sp: string | null): State {
  const s = fixtureState("flow");
  s.rev = revOf(doc);
  Object.assign(s, { changes: [], deletions: [], lost: [], later: [], rewrite: null });
  const ask = s.items.find((i: State) => i.n === 21);
  s.items = sp ? [{ ...ask, at: { ...ask.at, loc: { sp } } }] : [];
  return s;
}

test.describe("repair", () => {
  test("j onto a stop the window's bottom cuts brings it fully into view, and it stays there", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const n = await stopSp(page, "Filler paragraph 9.");
    const next = await stopSp(page, "Filler paragraph 10.");
    await focusTo(page, n);
    // A scroll from script, so focus stays on n while the next stop crosses the bottom edge
    await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      window.scrollBy(0, el.getBoundingClientRect().top - (innerHeight - 10));
    }, next);
    await page.waitForTimeout(QUIET_MS);
    expect(await focused(page)).toBe(n);
    expect((await blockBox(page, next))!.bottom).toBeGreaterThan(WIDE.height);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(next);
    await page.waitForTimeout(SETTLE_MS);
    expect((await blockBox(page, next))!.bottom).toBeLessThanOrEqual(WIDE.height);
  });

  test("a pushed review_state that grows a note already above keeps the focus's top within 2 px", async ({ page, birta }) => {
    const doc = longDoc();
    const six = spAt(doc, "Filler paragraph 6.");
    const { mock } = await open(page, birta, doc, { state: notesState(doc, [{ sp: six, body: "one line" }]) });
    const sp = await stopSp(page, "Filler paragraph 9.");
    await expect(page.locator('birta-flow[data-kind="note"]', { hasText: "one line" })).toHaveCount(1);
    await focusTo(page, sp);
    await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      window.scrollBy(0, el.getBoundingClientRect().top - innerHeight / 2);
    }, sp);
    await page.waitForTimeout(QUIET_MS);
    const before = (await blockBox(page, sp))!.top;
    const body = Array.from({ length: 8 }, (_, i) => `line ${i + 1} of the same note, amended`).join("\n");
    mock.pushState(notesState(doc, [{ sp: six, body }]));
    await expect(page.locator('birta-flow[data-kind="note"]', { hasText: "line 8 of the same note" })).toHaveCount(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(sp);
    const after = (await blockBox(page, sp))!.top;
    expect(Math.abs(after - before), `top ${before} → ${after}`).toBeLessThanOrEqual(2);
  });

  test("a question the page adds while your scroll goes on never takes the focus", async ({ page, birta }) => {
    const tall = "Tall paragraph " + Array.from({ length: 220 }, (_, i) => `word${i}`).join(" ") + ".";
    const doc = longDoc().replace("\nFiller paragraph 12.\n", `\n${tall}\n`);
    const { mock } = await open(page, birta, doc, { state: askState(doc, null) });
    const straddle = await stopSp(page, "Tall paragraph");
    const x = await stopSp(page, "Filler paragraph 13.");
    await focusTo(page, await stopSp(page, "Filler paragraph 5."));
    await page.waitForTimeout(QUIET_MS);
    // Wheeled so the tall paragraph straddles the header, and the one after it is the first stop in view
    const by = await page.evaluate((sp) => {
      const r = document.querySelector(`#content [data-sourcepos="${sp}"]`)!.getBoundingClientRect();
      return r.top + r.height / 2 - document.querySelector(".file-header")!.getBoundingClientRect().bottom;
    }, straddle);
    // One scroll, in steps under 100 ms apart: it moves focus to x, then the question lands mid-scroll
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, by - 40);
    for (let i = 0; i < 10; i++) {
      await page.waitForTimeout(30);
      if (i === 1) {
        expect(await focused(page)).toBe(x);
        mock.pushState(askState(doc, straddle));
      }
      await page.mouse.wheel(0, 4);
    }
    const blank = page.locator('birta-flow[data-kind="blank"]');
    await expect(blank).toHaveCount(1);
    await page.waitForTimeout(SETTLE_MS);
    // The question shows in view, above x, and still x has the focus
    const q = (await blank.boundingBox())!;
    expect(q.y + q.height).toBeGreaterThan((await view(page)).top);
    expect(await focused(page)).toBe(x);
    expect(await page.evaluate(() => document.activeElement?.getAttribute("data-kind") ?? null)).not.toBe("blank");
  });

  for (const key of ["Meta+ArrowDown", "Alt+ArrowDown"]) {
    test(`${key} scrolls the focus out of view, and the focus follows`, async ({ page, birta }) => {
      await open(page, birta, longDoc());
      const why = await stopSp(page, "Why");
      await focusTo(page, why);
      await page.waitForTimeout(QUIET_MS);
      for (let i = 0; i < 20 && !(await outOfView(page, why)); i++) {
        await page.keyboard.press(key);
        await scrollSettled(page);
      }
      expect(await outOfView(page, why)).toBe(true);
      const want = await firstInView(page);
      expect(want).not.toBeNull();
      await expect.poll(() => focused(page)).toBe(want);
    });
  }
});
