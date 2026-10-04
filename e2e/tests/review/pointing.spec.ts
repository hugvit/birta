import * as fs from "fs";
import * as path from "path";
import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  REPO,
  Rgb,
  STOPS_DOC,
  State,
  blockBox,
  booted,
  contentBox,
  fixtureDoc,
  fixtureState,
  focusMark,
  focusTo,
  focusTop,
  focused,
  highlighted,
  longDoc,
  markKeys,
  marks,
  mockReview,
  near,
  revOf,
  reviewLog,
  shot,
  stopSp,
  stops,
  watchSent,
  withRev,
} from "./harness";
import { GESTURES, P_PR, P_RETRY, dragWords, mid, waitDrawn, wordRect } from "../gestures";

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
const ANCHOR_MD = path.join(REPO, "e2e", "fixtures", "anchor.md");
const GESTURES_JSON = path.join(REPO, "tests", "fixtures", "anchor_gestures.json");

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

async function open(
  page: Page,
  birta: Start,
  doc: string,
  opts: { state?: State; viewport?: { width: number; height: number } } = {},
) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(opts.viewport ?? WIDE);
  const mock = await mockReview(page, { state: opts.state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

/** Presses j or k until `sp` has focus. */
async function focusOn(page: Page, sp: string) {
  if (!(await stops(page)).some((s) => s.sp === sp)) throw new Error(`${sp} is not a stop`);
  await focusTo(page, sp);
}

/** The live sp of the paragraph or item whose text is exactly `text`. */
function liveSp(page: Page, text: string): Promise<string | null> {
  return page.evaluate((text) => {
    for (const el of document.querySelectorAll("#content :is(p,li,h1,h2,h3,h4,h5,h6)[data-sourcepos]"))
      if ((el.textContent ?? "").trim() === text) return el.getAttribute("data-sourcepos");
    return null;
  }, text);
}

/** The colour just left of the filename's text, inside the doc stop's 8px wash. */
async function filenameEdge(page: Page): Promise<Rgb> {
  // The text, not the element, since the wash may pad the element
  const text = await page.evaluate(() => {
    const r = document.createRange();
    r.selectNodeContents(document.querySelector(".file-header-name")!);
    const b = r.getBoundingClientRect();
    return { x: b.left, y: b.top, height: b.height };
  });
  const s = await shot(page, { left: text.x - 6, top: text.y, width: 4, height: text.height });
  return s.at(text.x - 4, text.y + text.height / 2);
}

function near1(a: number, b: number) {
  return Math.abs(a - b) <= 1;
}

/** The colour 8px left of a block's box, where the focus outline runs, clear of text and the lane. */
async function washEdge(page: Page, sp: string): Promise<Rgb> {
  const b = (await blockBox(page, sp))!;
  const y = b.top + b.height / 2;
  const s = await shot(page, { left: b.left - 10, top: y - 1, width: 4, height: 2 });
  return s.at(b.left - 7.5, y);
}

/** The colour just outside the filename's box, where its outline runs. */
async function filenameOutline(page: Page): Promise<Rgb> {
  const b = (await page.locator(".file-header-name").boundingBox())!;
  const s = await shot(page, { left: b.x - 3, top: b.y, width: 4, height: b.height });
  return s.at(b.x - 0.5, b.y + b.height / 2);
}

/** The wash a focused heading lays over its section. */
async function scopeMark(page: Page) {
  return (await marks(page)).find((m) => m.mark === "scope") ?? null;
}

/** Every outbox key the page holds, so a spec can wait for acks to land. */
const outbox = (page: Page) =>
  page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith("birta-unacked:") && localStorage.getItem(k)));

test.describe("focus", () => {
  test("j walks every stop in order, nested items included, and k walks back past the H1 to whole doc", async ({
    page,
    birta,
  }) => {
    await open(page, birta, STOPS_DOC);
    const all = await stops(page);
    // The walk covers nested items and table rows, and never the raw HTML block
    expect(all.map((s) => s.kind)).toEqual([
      "heading",
      "heading",
      "paragraph",
      "heading",
      "list_item",
      "list_item",
      "list_item",
      "list_item",
      "list_item",
      "table_row",
      "table_row",
      "code_block",
      "heading",
      "paragraph",
      "heading",
      "paragraph",
    ]);
    expect(await focused(page)).toBe("doc");
    const down: (string | null)[] = [];
    for (let i = 0; i < all.length; i++) {
      await page.keyboard.press("j");
      await expect.poll(() => focused(page)).toBe(all[i].sp);
      down.push(await focused(page));
    }
    expect(down).toEqual(all.map((s) => s.sp));
    await page.keyboard.press("j");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(all[all.length - 1].sp);
    for (let i = all.length - 2; i >= 0; i--) {
      await page.keyboard.press("k");
      await expect.poll(() => focused(page)).toBe(all[i].sp);
    }
    // The H1 spans the doc, and whole doc still lies above it, with nothing above that
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("doc");
    await page.keyboard.press("k");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe("doc");
  });

  test("the filename is never outlined or washed: whole doc is the doc stop, whatever review has done in the tab", async ({
    page,
    birta,
    context,
  }) => {
    // With no H1 spanning the doc, the file chip once stood for the doc
    const { served } = await open(page, birta, "Intro words.\n\n" + STOPS_DOC);
    const bg = await filenameEdge(page);
    await expect.poll(() => focused(page)).toBe("doc");
    await page.waitForTimeout(SETTLE_MS);
    expect(near(await filenameOutline(page), bg)).toBe(true);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:12");
    expect(near(await filenameOutline(page), bg)).toBe(true);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("doc");
    await page.waitForTimeout(SETTLE_MS);
    expect(near(await filenameOutline(page), bg)).toBe(true);
    expect(near(await filenameEdge(page), bg)).toBe(true);
    await page.reload();
    await booted(page);
    await expect.poll(() => focused(page)).toBe("doc");
    await page.waitForTimeout(SETTLE_MS);
    expect(near(await filenameOutline(page), bg)).toBe(true);
    const other = await context.newPage();
    await other.setViewportSize(WIDE);
    await other.goto(`${served.url}/view/pr.md`);
    await booted(other);
    await other.waitForTimeout(SETTLE_MS);
    expect(near(await filenameOutline(other), bg)).toBe(true);
  });

  test("a heading's wash covers its section, down to the next heading of its level or higher", async ({
    page,
    birta,
  }) => {
    await open(page, birta, STOPS_DOC);
    const content = await contentBox(page);
    const detailWords = (await blockBox(page, await stopSp(page, "Detail words.")))!;
    const nothingYet = (await blockBox(page, await stopSp(page, "Nothing yet.")))!;
    for (const [heading, last] of [
      ["What changes", detailWords],
      ["Limits", detailWords],
    ] as const) {
      const sp = await stopSp(page, heading);
      const h = (await blockBox(page, sp))!;
      await focusOn(page, sp);
      const m = (await scopeMark(page))!;
      expect(near1(m.top, h.top) && near1(m.bottom, last.bottom), `${heading}: ${m.top}-${m.bottom}`).toBe(true);
      expect(near1(m.left, content.left - 8) && near1(m.right, content.right + 8), `${heading}: ${m.left}-${m.right}`).toBe(
        true,
      );
    }
    // The title is an ordinary block: its wash stays on its own line, though its section runs to "Nothing yet."
    const title = await stopSp(page, "Upload retries");
    const t = (await blockBox(page, title))!;
    await focusOn(page, title);
    const tm = await scopeMark(page);
    expect(tm === null || tm.bottom <= t.bottom + 4, `title wash ${tm?.top}-${tm?.bottom}`).toBe(true);
    expect(nothingYet.bottom).toBeGreaterThan(t.bottom);
  });

  test("a list item's outline covers the whole item, its nested list and its marker", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    for (const text of ["backoff of 1, 2 and 4 seconds", "capped at 8 seconds"]) {
      const sp = await stopSp(page, text);
      const li = (await blockBox(page, sp))!;
      await focusOn(page, sp);
      const m = (await focusMark(page))!;
      // The outline keeps 2px off the item's lines
      expect(near1(m.top, li.top - 2) && near1(m.bottom, li.bottom + 2), `${text}: ${m.top}-${m.bottom}`).toBe(true);
      expect(near1(m.left, li.list!.left - 8) && near1(m.right, li.right + 8), `${text}: ${m.left}-${m.right}`).toBe(true);
    }
  });

  test("a paragraph, a table row and a code block outline their own box", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const content = await contentBox(page);
    for (const text of ["When the upload", "503", "retry(upload, 3);"]) {
      const sp = await stopSp(page, text);
      const b = (await blockBox(page, sp))!;
      await focusOn(page, sp);
      const m = (await focusMark(page))!;
      expect(near1(m.top, b.top - 2) && near1(m.bottom, b.bottom + 2), `${text}: ${m.top}-${m.bottom}`).toBe(true);
      const left = Math.max(b.left, content.left) - 8;
      const right = Math.min(b.right, content.right) + 8;
      expect(near1(m.left, left) && near1(m.right, right), `${text}: ${m.left}-${m.right}`).toBe(true);
    }
  });

  test("your scroll from whole doc moves the focus to the first stop below the headers, and j goes on from it", async ({
    page,
    birta,
  }) => {
    await open(page, birta, longDoc());
    const target = await stopSp(page, "Filler paragraph 20.");
    const next = await stopSp(page, "Filler paragraph 21.");
    await expect.poll(() => focused(page)).toBe("doc");
    const by = await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      return el.getBoundingClientRect().top - document.querySelector(".file-header")!.getBoundingClientRect().bottom - 10;
    }, target);
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, by);
    await expect.poll(() => focused(page)).toBe(target);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(next);
  });

  test("j brings the stop it focuses into view", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const all = await stops(page);
    const height = await page.evaluate(() => innerHeight);
    const firstBelow = await page.evaluate(
      ({ sps, height }) =>
        sps.findIndex((sp) => document.querySelector(`#content [data-sourcepos="${sp}"]`)!.getBoundingClientRect().top > height),
      { sps: all.map((s) => s.sp), height },
    );
    expect(firstBelow).toBeGreaterThan(0);
    for (let i = 0; i <= firstBelow; i++) await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(all[firstBelow].sp);
    const header = (await page.locator(".file-header").boundingBox())!;
    const b = (await blockBox(page, all[firstBelow].sp))!;
    expect(b.top).toBeGreaterThanOrEqual(header.y + header.height - 1);
    expect(b.bottom).toBeLessThanOrEqual(height + 1);
  });

  test("focus your scroll takes out of view moves to a stop in view, and a scroll from script leaves it there", async ({
    page,
    birta,
  }) => {
    await open(page, birta, longDoc());
    const all = await stops(page);
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(all[1].sp);
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, 100_000);
    await expect.poll(async () => ![null, all[1].sp].includes(await focused(page))).toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    const moved = (await focused(page))!;
    const b = (await blockBox(page, moved))!;
    const header = (await page.locator(".file-header").boundingBox())!;
    expect(b.top).toBeGreaterThanOrEqual(header.y + header.height - 1);
    expect(b.top).toBeLessThan(WIDE.height);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(moved);
  });

  test("focus follows its block through an edit above it, and moves to the next stop below when the block goes", async ({
    page,
    birta,
  }) => {
    const { served } = await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusOn(page, sp);
    served.write("pr.md", STOPS_DOC.replace("## Why\n", "## Why\n\nA new first paragraph.\n"));
    await expect.poll(() => liveSp(page, "Detail words.")).not.toBe(sp);
    const moved = (await liveSp(page, "Detail words."))!;
    await expect.poll(() => focused(page)).toBe(moved);
    const b = (await blockBox(page, moved))!;
    await expect.poll(async () => near1((await focusMark(page))!.top, b.top - 2)).toBe(true);

    served.write("pr.md", STOPS_DOC.replace("Detail words.\n", ""));
    await expect.poll(() => liveSp(page, "Detail words.")).toBeNull();
    await expect.poll(() => focused(page)).toBe(await liveSp(page, "Stacked on"));
    expect(await focused(page)).not.toBeNull();
  });

  test("the focus outline is painted; r hides it and keeps focus, and m leaves it", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    const bg = await washEdge(page, sp);
    await focusOn(page, sp);
    await expect.poll(async () => near(await washEdge(page, sp), bg)).toBe(false);
    await page.keyboard.press("r");
    await expect.poll(async () => near(await washEdge(page, sp), bg)).toBe(true);
    expect(await focused(page)).toBe(sp);
    await page.keyboard.press("r");
    await expect.poll(async () => near(await washEdge(page, sp), bg)).toBe(false);
    await page.keyboard.press("m");
    await page.waitForTimeout(SETTLE_MS);
    expect(near(await washEdge(page, sp), bg)).toBe(false);
    expect(await focused(page)).toBe(sp);
    await page.keyboard.press("m");
  });

  test("the focus outline is re-measured when the page reflows", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "When the upload");
    const onBlock = async () => {
      const b = (await blockBox(page, sp))!;
      const m = await focusMark(page);
      return !!m && near1(m.top, b.top - 2) && near1(m.bottom, b.bottom + 2) && near1(m.left, b.left - 8);
    };
    // Focused by a click in reading mode, which a review key would leave; leaving it pushes the doc down
    await page.keyboard.press("j");
    await page.keyboard.press("r");
    await expect.poll(() => page.evaluate(() => document.body.classList.contains("reading-mode"))).toBe(true);
    await page.locator(`#content [data-sourcepos="${sp}"]`).click();
    await expect.poll(() => focused(page)).toBe(sp);
    await page.keyboard.press("r");
    await expect.poll(onBlock).toBe(true);
    await page.setViewportSize({ width: 800, height: 900 });
    await expect.poll(onBlock).toBe(true);
  });

  test("after an edit, j walks the new stops and a heading's wash grows with its section", async ({ page, birta }) => {
    const { served } = await open(page, birta, STOPS_DOC);
    const limits = await stopSp(page, "Limits");
    await focusOn(page, limits);
    served.write(
      "pr.md",
      STOPS_DOC.replace("## Why\n", "## Why\n\nA new first paragraph.\n").replace("Detail words.\n", "Detail words.\n\nMore detail.\n"),
    );
    await expect.poll(() => liveSp(page, "More detail.")).not.toBeNull();
    const last = (await blockBox(page, (await liveSp(page, "More detail."))!))!;
    await expect.poll(async () => near1((await scopeMark(page))!.bottom, last.bottom)).toBe(true);
    // Up to whole doc, then down past the title and ## Why
    await focusTop(page);
    expect(await focused(page)).toBe("doc");
    for (const _ of [0, 1, 2]) await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(await liveSp(page, "A new first paragraph."));
  });

  test("focus survives the scroll j makes, and a small scroll", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const all = await stops(page);
    const height = await page.evaluate(() => innerHeight);
    const firstBelow = await page.evaluate(
      ({ sps, height }) =>
        sps.findIndex((sp) => document.querySelector(`#content [data-sourcepos="${sp}"]`)!.getBoundingClientRect().top > height),
      { sps: all.map((s) => s.sp), height },
    );
    for (let i = 0; i <= firstBelow; i++) await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(all[firstBelow].sp);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(all[firstBelow].sp);
    await page.mouse.wheel(0, 40);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(all[firstBelow].sp);
  });

  test("j and k pass over a closed details on its summary, and walk inside once it opens", async ({ page, birta }) => {
    // "More" also opens a paragraph above, so a search for the summary's words would land there
    const doc = `${STOPS_DOC}\nMore on this.\n\n<details><summary>More</summary>\n\nInside details.\n\n- inner item\n\n</details>\n\nAfter details.\n`;
    const { mock } = await open(page, birta, doc);
    const before = await stopSp(page, "More on this.");
    const inside = await stopSp(page, "Inside details.");
    const after = await stopSp(page, "After details.");
    await focusOn(page, before);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("summary");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(after);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("summary");
    // The summary has no stop, so a verdict on it takes the first stop its details hides
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].at).toStrictEqual({ k: "block", sp: inside, scope: "block" });
    await page.locator("#content summary").click();
    await expect(page.locator("#content details")).toHaveAttribute("open", "");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(inside);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(await stopSp(page, "inner item"));
  });

  test("a verdict on a closed details lands on its own block, though its summary's words recur", async ({
    page,
    birta,
  }) => {
    const doc = [
      "# Notes",
      "",
      "More on this.",
      "",
      "- item with details",
      "",
      "  <details><summary>More</summary>",
      "",
      "  Nested inside.",
      "",
      "  </details>",
      "",
      "- next item",
      "",
      "<details><summary>More</summary>",
      "",
      "Inside details.",
      "",
      "</details>",
      "",
    ].join("\n");
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    // A summary inside a stop is reached as that stop; the top-level one is its own target
    await focusOn(page, await stopSp(page, "next item"));
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("summary");
    await page.keyboard.press("x");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    const line = doc.split("\n").indexOf("Inside details.") + 1;
    expect(reviewLog(served.root)![3].at).toMatchObject({ scope: "block", lines: [line, line] });
  });

  test("m, z, Esc, Enter and r do not start review in the tab", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const bg = await filenameEdge(page);
    for (const key of ["m", "m", "z", "Escape", "Enter", "r", "r"]) await page.keyboard.press(key);
    await page.reload();
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(near(await filenameEdge(page), bg)).toBe(true);
  });
});

test.describe("verdict keys", () => {
  test("a verdict acts on the selection, then the focus, and on the doc only says what to do", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    await focusOn(page, para);
    await page.evaluate((sp) => {
      const p = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      const t = p.firstChild as Text;
      const at = t.data.indexOf("upload");
      const r = document.createRange();
      r.setStart(t, at);
      r.setEnd(t, at + "upload".length);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    }, para);
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.keyboard.press("Escape");
    await page.keyboard.press("l");
    await expect.poll(() => mock.ops().length).toBe(2);
    // Whole doc, above the title, which Esc never drops focus to
    await focusTop(page);
    expect(await focused(page)).toBe("doc");
    // A whole-doc verdict tells the agent nothing it can act on
    await page.keyboard.press(".");
    await expect(page.getByText("your call needs a block · j")).toBeVisible();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(2);
    const [quote, block] = mock.ops();
    expect(quote).toMatchObject({ op: "comment", verdict: "yes", at: { k: "quote", sp: para, q: "upload", off: 9 } });
    expect(block).toMatchObject({ op: "comment", verdict: "later", at: { k: "block", sp: para, scope: "block" } });
    for (const op of mock.ops()) expect(op.body).toBeUndefined();
  });

  test("j and k clear a selection, so the next verdict goes to the focused block", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    await focusOn(page, para);
    await page.evaluate((sp) => {
      const t = document.querySelector(`#content [data-sourcepos="${sp}"]`)!.firstChild as Text;
      const r = document.createRange();
      r.setStart(t, 9);
      r.setEnd(t, 15);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    }, para);
    await expect(page.getByText("c note · q ask · x cut")).toBeVisible();
    for (const [key, text] of [["j", "What changes"], ["k", "When the upload"]] as const) {
      await page.keyboard.press(key);
      const sp = await stopSp(page, text);
      await expect.poll(() => focused(page)).toBe(sp);
      expect(await page.evaluate(() => getSelection()!.toString())).toBe("");
      await expect(page.getByText("c note · q ask · x cut")).toHaveCount(0);
      // Reselect inside the focused block, since a selection elsewhere would move the focus to it
      if (key === "j") {
        await page.evaluate((sp) => {
          const h = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
          const t = Array.from(h.childNodes).find((n) => n.nodeType === 3 && n.textContent!.trim()) as Text;
          const r = document.createRange();
          r.setStart(t, 0);
          r.setEnd(t, 4);
          getSelection()!.removeAllRanges();
          getSelection()!.addRange(r);
        }, sp);
        await expect(page.getByText("c note · q ask · x cut")).toBeVisible();
      }
    }
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].at).toStrictEqual({ k: "block", sp: para, scope: "block" });
  });

  test("a verdict on a heading covers its section, and one on an item or row covers that stop", async ({
    page,
    birta,
  }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const want: [string, string, string][] = [
      ["What changes", "yes", "section"],
      ["backoff of 1, 2 and 4 seconds", "no", "block"],
      ["503", "later", "block"],
    ];
    for (const [text, verdict, scope] of want) {
      const sp = await stopSp(page, text);
      await focusOn(page, sp);
      await page.keyboard.press({ yes: "y", no: "x", later: "l" }[verdict]!);
      await expect.poll(() => mock.ops().at(-1)?.at?.sp).toBe(sp);
      expect(mock.ops().at(-1)).toMatchObject({ op: "comment", verdict, at: { k: "block", sp, scope } });
    }
  });

  test("z retracts the verdict it just sent", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await focusOn(page, await stopSp(page, "Detail words."));
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    // The mock acks with n = 101
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1]).toMatchObject({ op: "retract", to: 101 });
  });

  test("after an edit, a verdict carries the new rev and the moved sp, and the server keeps it", async ({
    page,
    birta,
  }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    const sent = watchSent(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const sp = await stopSp(page, "Detail words.");
    await focusOn(page, sp);
    const next = STOPS_DOC.replace("## Why\n", "## Why\n\nA new first paragraph.\n");
    served.write("pr.md", next);
    await expect.poll(() => liveSp(page, "Detail words.")).not.toBe(sp);
    const moved = (await liveSp(page, "Detail words."))!;
    await expect.poll(() => focused(page)).toBe(moved);
    await page.keyboard.press("x");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    const op = sent.find((f) => f.type === "op")!;
    expect(op.rev).toBe(revOf(next));
    expect(op.at).toStrictEqual({ k: "block", sp: moved, scope: "block" });
  });

  test("z retracts the latest verdict by its own ack, whatever order acks arrive in, then the one before", async ({
    page,
    birta,
  }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await page.keyboard.press("x");
    await page.keyboard.press("j");
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(2);
    const [first, second] = mock.ops();
    expect(first.cid).not.toBe(second.cid);
    mock.ack(second.cid); // n 101
    mock.ack(first.cid); // n 102
    await expect.poll(() => outbox(page)).toEqual([]);
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(3);
    expect(mock.ops()[2]).toMatchObject({ op: "retract", to: 101 });
    // z is a stack: the next press takes back the verdict before
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(4);
    expect(mock.ops()[3]).toMatchObject({ op: "retract", to: 102 });
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().filter((o) => o.op === "retract")).toHaveLength(2);
  });

  test("z before an ack names no n, and after a nack takes nothing back", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    mock.nack(mock.ops()[0].cid, "read_only");
    await expect.poll(() => outbox(page)).toEqual([]);
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });

  test("a selection in another block beats the focus, and a caret is no selection", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    const why = await stopSp(page, "Why");
    const select = (sp: string, word: string | null) =>
      page.evaluate(
        ({ sp, word }) => {
          const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
          const t = Array.from(el.childNodes).find((n) => n.nodeType === 3 && n.textContent!.trim()) as Text;
          const r = document.createRange();
          const at = word ? t.data.indexOf(word) : 1;
          r.setStart(t, at);
          r.setEnd(t, word ? at + word.length : at);
          getSelection()!.removeAllRanges();
          getSelection()!.addRange(r);
        },
        { sp, word },
      );
    await focusOn(page, await stopSp(page, "Detail words."));
    await select(para, "upload");
    // The selection's block becomes the focus
    await expect.poll(() => focused(page)).toBe(para);
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].at).toMatchObject({ k: "quote", sp: para, q: "upload", off: 9 });
    await focusOn(page, why);
    await select(why, null);
    await expect(page.getByText("c note · q ask · x cut")).toHaveCount(0);
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1].at).toStrictEqual({ k: "block", sp: why, scope: "section" });
    // Esc skips the empty selection, and on a stop does nothing
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(why);
  });
});

test.describe("selection", () => {
  const committed = JSON.parse(fs.readFileSync(GESTURES_JSON, "utf-8"));

  async function openAnchor(page: Page, birta: Start) {
    const served = await birta({ "anchor.md": fs.readFileSync(ANCHOR_MD, "utf-8") }, GATE_ON);
    // The viewport the gestures were recorded at
    await page.setViewportSize({ width: 1100, height: 1000 });
    const mock = await mockReview(page);
    await page.goto(`${served.url}/view/anchor.md`);
    await booted(page);
    await waitDrawn(page);
    return mock;
  }

  // 19 and 20 hand a range to the extractor directly; no user can select them
  for (const g of GESTURES.filter((g) => Number(g.id.slice(0, 2)) <= 18)) {
    test(`x on gesture ${g.id} sends the committed quote`, async ({ page, birta }, testInfo) => {
      const want = committed.engines[testInfo.project.name]?.[g.id];
      expect(want, `no committed ${testInfo.project.name} recording of ${g.id}`).toBeTruthy();
      const mock = await openAnchor(page, birta);
      await g.act(page);
      await page.keyboard.press("x");
      await expect.poll(() => mock.ops().length).toBe(1);
      const at = mock.ops()[0].at;
      const expected: State = { k: "quote", q: want.quote, pre: want.pre, suf: want.suf };
      if (want.sp !== null) Object.assign(expected, { sp: want.sp, off: want.off, tl: want.tl });
      expect(at).toStrictEqual(expected);
      expect(mock.ops()[0].verdict).toBe("no");
    });
  }

  test("a selection shows what the keys will do, and the hint goes with the selection", async ({ page, birta }) => {
    await openAnchor(page, birta);
    await GESTURES.find((g) => g.id.startsWith("01"))!.act(page);
    const hint = page.getByText("c note · q ask · x cut");
    await expect(hint).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(hint).toBeHidden();
  });

  test("a drag into the next paragraph is clamped to the first, and says so", async ({ page, birta }) => {
    await openAnchor(page, birta);
    await GESTURES.find((g) => g.id.startsWith("08"))!.act(page);
    await expect(page.getByText("selection clamped to this paragraph")).toBeVisible();
  });

  test("a triple-click selects its block without a clamp pill, since nothing was dropped", async ({ page, birta }) => {
    await openAnchor(page, birta);
    for (const id of ["02", "03", "04", "06"]) {
      await GESTURES.find((g) => g.id.startsWith(id))!.act(page);
      await expect(page.getByText("c note · q ask · x cut")).toBeVisible();
      await expect(page.getByText("selection clamped to this paragraph")).toHaveCount(0);
    }
  });

  test("a drag across code lines keeps the newline and the tab", async ({ page, birta }) => {
    const mock = await openAnchor(page, birta);
    const pre = 'pre[data-sourcepos="43:1-47:3"]';
    await dragWords(page, [pre, "retry"], [pre, "5"]);
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].at).toMatchObject({ k: "quote", sp: "43:1-47:3", q: "retry() {\n\tlet n = 5" });
  });

  for (const [id, from, to] of [
    // Dragging from a link drags the link, so the in-block case starts on plain text
    ["09", [P_RETRY, "three"], [P_RETRY, "end"]],
    ["08", [P_PR, "adds"], [P_RETRY, "hard"]],
  ] as [string, [string, string], [string, string]][]) {
    test(`gesture ${id} dragged backwards sends the same quote as forwards`, async ({ page, birta }) => {
      const mock = await openAnchor(page, birta);
      await GESTURES.find((g) => g.id.startsWith(id))!.act(page);
      await page.keyboard.press("x");
      await expect.poll(() => mock.ops().length).toBe(1);
      await page.keyboard.press("Escape");
      await wordRect(page, ...from);
      const b = await wordRect(page, ...to);
      const a = await wordRect(page, ...from);
      // Past the double-click interval, since the forward drag ended on this spot
      await page.waitForTimeout(600);
      // Clear of the footnote link that follows "end", which a press there would drag
      await page.mouse.move(b.right - 3, mid(b).y);
      await page.mouse.down();
      await page.mouse.move(a.left + 1, mid(a).y, { steps: 8 });
      await page.mouse.up();
      if (id === "08") await expect(page.getByText("selection clamped to this paragraph")).toBeVisible();
      await page.keyboard.press("y");
      await expect.poll(() => mock.ops().length).toBe(2);
      expect(mock.ops()[1].at).toStrictEqual(mock.ops()[0].at);
    });
  }

  test("a selection outside the doc is not a target, so the verdict falls to the doc, which decides nothing", async ({
    page,
    birta,
  }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.locator(".file-header-name").dblclick();
    expect(await page.evaluate(() => getSelection()!.toString())).not.toBe("");
    await page.keyboard.press("x");
    await expect(page.getByText("whole doc: c to say what's wrong")).toBeVisible();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });
});

test.describe("underline", () => {
  test("a quote note's words are underlined from the server's offsets, and nothing else is", async ({
    page,
    birta,
  }) => {
    await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    // Item 4 carries off [27, 39]; item 5 is a block note and the doc note has no place
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
  });

  test("m hides the underline and m brings it back", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
    const rect = await page.evaluate(() => {
      for (const [, hl] of (CSS as any).highlights) for (const r of hl) return (r as Range).getBoundingClientRect().toJSON();
      return null;
    });
    const box = { left: rect.left - 2, top: rect.top, width: rect.width + 4, height: rect.height + 4 };
    const pixels = async () => {
      const s = await shot(page, box);
      const out: number[] = [];
      for (let y = Math.ceil(box.top); y < box.top + box.height - 1; y++)
        for (let x = Math.ceil(box.left); x < box.left + box.width - 1; x++) out.push(...s.at(x, y));
      return out;
    };
    const shown = await pixels();
    await page.keyboard.press("m");
    await expect.poll(async () => (await pixels()).join()).not.toBe(shown.join());
    await page.keyboard.press("m");
    await expect.poll(async () => (await pixels()).join()).toBe(shown.join());
  });

  test("a later state without the note clears its underline", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
    const next = fixtureState("drafts");
    next.items = next.items.filter((i: State) => i.n !== 4);
    mock.pushState(next);
    await expect.poll(() => highlighted(page)).toEqual([]);
  });
  test("the underline paints at.loc.off, not a search for the quote", async ({ page, birta }) => {
    const state = fixtureState("drafts");
    state.items.find((i: State) => i.n === 4).at.loc.off = [27, 34];
    await open(page, birta, fixtureDoc("drafts"), { state });
    await expect.poll(() => highlighted(page)).toEqual(["we just"]);
  });

  test("a patch to the quoted paragraph moves the underline onto live text", async ({ page, birta }) => {
    const doc = fixtureDoc("drafts");
    const next = doc.replace("start again.", "start over again.");
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.setViewportSize(WIDE);
    await mockReview(page, { states: [fixtureState("drafts"), withRev(fixtureState("drafts"), revOf(next))] });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
    served.write("pr.md", next);
    await expect(page.locator("#content")).toContainText("start over again.");
    await expect
      .poll(() =>
        page.evaluate(() => {
          const out: string[] = [];
          for (const [, hl] of (CSS as any).highlights) for (const r of hl) out.push(`${r}:${r.startContainer.isConnected}`);
          return out;
        }),
      )
      .toEqual(["we just fail:true"]);
  });

  test("reading mode hides the underline", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
    await page.keyboard.press("r");
    await expect.poll(() => highlighted(page)).toEqual([]);
    await page.keyboard.press("r");
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
  });

  test("without the Highlight API the page boots, draws its marks and throws nothing", async ({ page, birta }) => {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.addInitScript(() => Object.defineProperty(CSS, "highlights", { value: undefined, configurable: true }));
    await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await expect.poll(async () => (await markKeys(page)).some((k) => k.includes("5:1-6:54"))).toBe(true);
    expect(errors).toEqual([]);
  });
});

test.describe("a long doc", () => {
  const paragraphs = (n: number) =>
    "# Long\n\n" + Array.from({ length: n }, (_, i) => `Paragraph number ${i} with *some* words.\n`).join("\n");

  /** Serves `n` paragraphs and focuses the second stop, so the next j is a step between blocks. */
  async function openLong(page: Page, birta: Start, n: number) {
    const served = await birta({ "pr.md": paragraphs(n) }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    expect(await stops(page)).toHaveLength(n + 1);
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await expect.poll(async () => focused(page)).toBe((await stops(page))[1].sp);
  }

  /** One j, dispatched as the keyboard would: the queries it makes and how long it takes. */
  function oneJ(page: Page): Promise<{ queries: number; ms: number }> {
    return page.evaluate(() => {
      let queries = 0;
      const el = Element.prototype.querySelectorAll;
      const doc = Document.prototype.querySelectorAll;
      Element.prototype.querySelectorAll = function (this: Element, sel: string) {
        queries++;
        return el.call(this, sel);
      } as typeof el;
      Document.prototype.querySelectorAll = function (this: Document, sel: string) {
        queries++;
        return doc.call(this, sel);
      } as typeof doc;
      const t0 = performance.now();
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "j", code: "KeyJ", bubbles: true, cancelable: true }));
      const ms = performance.now() - t0;
      Element.prototype.querySelectorAll = el;
      Document.prototype.querySelectorAll = doc;
      return { queries, ms };
    });
  }

  test("one j queries the doc no more on 3000 paragraphs than on 300", async ({ page, birta }) => {
    const counts: number[] = [];
    for (const n of [300, 3000]) {
      await openLong(page, birta, n);
      counts.push((await oneJ(page)).queries);
    }
    expect(counts[1]).toBe(counts[0]);
    expect(counts[1]).toBeLessThanOrEqual(5);
  });

  test("one j on 3000 paragraphs takes under 30 ms", async ({ page, birta, browserName }) => {
    test.skip(browserName !== "chromium", "the budget is chromium's");
    await openLong(page, birta, 3000);
    const times: number[] = [];
    for (let i = 0; i < 5; i++) times.push((await oneJ(page)).ms);
    times.sort((a, b) => a - b);
    expect(times[2]).toBeLessThan(30);
  });
});
