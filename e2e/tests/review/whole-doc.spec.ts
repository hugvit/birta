import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  Mock,
  OPEN_ROOTS,
  STOPS_DOC,
  State,
  booted,
  cacheCdn,
  fixtureDoc,
  fixtureState,
  focusMark,
  focusTo,
  focused,
  longDoc,
  marks,
  mockReview,
  phraseOf,
  revOf,
  stopSp,
  stops,
  withRev,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
  links?: Record<string, string>,
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

interface OpenOpts {
  state?: State;
  states?: State[];
  dir?: State;
  args?: string[];
  files?: Record<string, string>;
  viewport?: { width: number; height: number };
}

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, opts: OpenOpts = {}) {
  const served = await birta({ "pr.md": doc, ...(opts.files ?? {}) }, GATE_ON, opts.args);
  await page.setViewportSize(opts.viewport ?? WIDE);
  const mock = await mockReview(page, { state: opts.state, states: opts.states, dir: opts.dir });
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

interface DocStop {
  shown: boolean;
  text: string;
  top: number;
  bottom: number;
  /** Whether it sits before `#content` in the document. */
  above: boolean;
}

/** The `whole doc` stop's host, or null while the page has none. */
function docStop(page: Page): Promise<DocStop | null> {
  return page.evaluate(() => {
    const h = document.querySelector('birta-flow[data-kind="doc-stop"]');
    if (!h) return null;
    const content = document.getElementById("content")!;
    const r = h.getBoundingClientRect();
    const cs = getComputedStyle(h);
    let text = "";
    const walk = (x: Node) => {
      if (x.nodeType === 3) text += x.textContent;
      else if (x.nodeType === 1 && (x as Element).localName !== "style") x.childNodes.forEach(walk);
    };
    (h.shadowRoot ?? h).childNodes.forEach(walk);
    return {
      shown: h.getClientRects().length > 0 && r.height > 0 && cs.visibility !== "hidden",
      text: text.replace(/\s+/g, " ").trim(),
      top: r.top,
      bottom: r.bottom,
      above: !!(h.compareDocumentPosition(content) & Node.DOCUMENT_POSITION_FOLLOWING),
    };
  });
}

/** The rows `whole doc` holds: shown flow hosts above `#content`, the stop's own line and a composer left out. */
function docRows(page: Page): Promise<{ kind: string | null; text: string; top: number; bottom: number }[]> {
  return page.evaluate(() => {
    const content = document.getElementById("content")!;
    return Array.from(document.querySelectorAll("birta-flow"))
      .filter((h) => !["doc-stop", "composer"].includes(h.getAttribute("data-kind") ?? ""))
      .filter((h) => !!(h.compareDocumentPosition(content) & Node.DOCUMENT_POSITION_FOLLOWING) && h.getClientRects().length > 0)
      .map((h) => {
        const r = h.getBoundingClientRect();
        let text = "";
        const walk = (x: Node) => {
          if (x.nodeType === 3) text += x.textContent;
          else if (x.nodeType === 1 && (x as Element).localName !== "style") x.childNodes.forEach(walk);
        };
        (h.shadowRoot ?? h).childNodes.forEach(walk);
        return { kind: h.getAttribute("data-kind"), text: text.replace(/\s+/g, " ").trim(), top: r.top, bottom: r.bottom };
      });
  });
}

/** The view a stop must sit in: below the sticky header, above the window's bottom. */
function view(page: Page): Promise<{ top: number; bottom: number; content: number }> {
  return page.evaluate(() => ({
    top: document.querySelector(".file-header")!.getBoundingClientRect().bottom,
    bottom: innerHeight,
    content: document.getElementById("content")!.getBoundingClientRect().top,
  }));
}

/** Whether the header's file chip is marked or outlined as a focus. */
function chipFocused(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const chip = document.querySelector(".file-header-name")!;
    return chip.hasAttribute("data-review-focus") || getComputedStyle(chip).outlineStyle !== "none";
  });
}

async function turnKey(page: Page): Promise<string> {
  return (await page.evaluate(() => /Mac/.test(navigator.platform))) ? "⌘⏎" : "Ctrl⏎";
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

const viewing = (page: Page) => decodeURIComponent(new URL(page.url()).pathname);

/** The fields of an op frame, without the envelope the core adds. */
function opFields(frame: State): State {
  const out = { ...frame };
  for (const k of ["type", "cid", "path", "rev"]) delete out[k];
  return out;
}

/** Every op but seen, which landing on a changed block sends. */
const writes = (mock: Mock) => mock.ops().filter((o) => o.op !== "seen").map(opFields);

/** Polls until the focus mark is on `whole doc`: `data-target="doc"`, with no block's sp. */
async function onWholeDoc(page: Page) {
  await expect.poll(async () => {
    const m = await focusMark(page);
    return m && { target: m.target, sp: m.sp };
  }).toEqual({ target: "doc", sp: null });
}

// --- whole doc, the first stop ------------------------------------------------------------------

test.describe("whole doc, the first stop", () => {
  test("a review tab opens on whole doc: its slim line shows above the doc, in view, and the chip is no focus", async ({
    page,
    birta,
  }) => {
    await open(page, birta, STOPS_DOC);
    await onWholeDoc(page);
    const stop = (await docStop(page))!;
    expect(stop, "a birta-flow[data-kind=doc-stop] host").not.toBeNull();
    expect(stop.shown).toBe(true);
    expect(stop.text).toBe("whole doc");
    expect(stop.above).toBe(true);
    const v = await view(page);
    expect(stop.bottom).toBeLessThanOrEqual(v.content + 1);
    expect(stop.top).toBeGreaterThanOrEqual(v.top - 1);
    // The focus mark outlines the line, and lies between the header and the window's bottom
    const m = (await focusMark(page))!;
    expect(m.top).toBeGreaterThanOrEqual(v.top - 1);
    expect(m.bottom).toBeLessThanOrEqual(v.bottom + 1);
    expect(m.top).toBeLessThanOrEqual(stop.bottom);
    expect(m.bottom).toBeGreaterThanOrEqual(stop.top);
    expect(await chipFocused(page)).toBe(false);
  });

  test("j goes to the title, the empty whole doc line hides, k comes back to it, and k there says top of the doc", async ({
    page,
    birta,
  }) => {
    await open(page, birta, STOPS_DOC);
    const [h1] = (await stops(page)).map((s) => s.sp);
    await onWholeDoc(page);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(h1);
    // Empty, it shows only while focused
    await expect.poll(async () => (await docStop(page))?.shown ?? false).toBe(false);
    expect(await chipFocused(page)).toBe(false);
    await page.keyboard.press("k");
    await onWholeDoc(page);
    await expect.poll(async () => (await docStop(page))?.shown ?? false).toBe(true);
    expect(await pillOf(page)).not.toBe("top of the doc");
    await page.keyboard.press("k");
    await expect.poll(() => pillOf(page)).toBe("top of the doc");
    expect(await focused(page)).toBe("doc");
    expect(await chipFocused(page)).toBe(false);
  });

  test("with no H1 over the doc, whole doc is still the first stop and the chip never takes the focus", async ({
    page,
    birta,
  }) => {
    await open(page, birta, "Intro words.\n\n# Title\n\nBody words.\n");
    const [first] = (await stops(page)).map((s) => s.sp);
    await onWholeDoc(page);
    expect(await chipFocused(page)).toBe(false);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(first);
    await page.keyboard.press("k");
    await onWholeDoc(page);
    expect(await chipFocused(page)).toBe(false);
  });

  test("the chip's name sits on the phrase's line", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: withRev(fixtureState("fresh"), revOf(STOPS_DOC)) });
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    const centers = await page.evaluate(() => {
      const mid = (r: DOMRect) => (r.top + r.bottom) / 2;
      const textBox = (el: Element) => {
        const range = document.createRange();
        range.selectNodeContents(el);
        return range.getBoundingClientRect();
      };
      const chip = document.querySelector(".file-header-name")!;
      const host = document.querySelector(".file-header birta-phrase")!;
      const words = (host.shadowRoot ?? host).querySelector(".phrase") ?? host;
      return { chip: mid(textBox(chip)), phrase: mid(textBox(words)) };
    });
    expect(Math.abs(centers.chip - centers.phrase), JSON.stringify(centers)).toBeLessThanOrEqual(1);
  });

  test("Home from far down puts whole doc and its rows fully below the header", async ({ page, birta }) => {
    const doc = fixtureDoc("drafts") + Array.from({ length: 40 }, (_, i) => `\nFiller paragraph ${i + 1}.\n`).join("");
    await open(page, birta, doc, { state: withRev(fixtureState("drafts"), revOf(doc)) });
    await expect.poll(async () => (await docRows(page)).map((r) => r.text).join("\n")).toContain("is the test ladder still needed?");
    await page.mouse.move(640, 500);
    // Short of the page's end, whose rubber-band bounce on macOS would scroll on after Home's reveal
    await page.mouse.wheel(0, 1200);
    await scrollSettled(page);
    await expect.poll(() => focused(page)).not.toBe("doc");
    await page.keyboard.press("Home");
    await onWholeDoc(page);
    await scrollSettled(page);
    const v = await view(page);
    const rows = await docRows(page);
    expect(rows.length).toBeGreaterThan(0);
    for (const r of rows) expect(r.top, r.text).toBeGreaterThanOrEqual(v.top - 0.5);
    const stop = await docStop(page);
    if (stop?.shown) expect(stop.top).toBeGreaterThanOrEqual(v.top - 0.5);
    const m = (await focusMark(page))!;
    expect(m.top).toBeGreaterThanOrEqual(v.top - 0.5);
    expect(m.bottom).toBeLessThanOrEqual(v.bottom + 1);
  });

  test("p back onto Claude's whole-doc reply lands on whole doc, its rows below the header", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen"), viewport: { width: 1280, height: 320 } });
    for (let i = 0; i < 5; i++) await page.keyboard.press("n");
    await expect.poll(() => pillOf(page)).toBe("5 of 5 · n next · p back");
    await scrollSettled(page);
    expect(await page.evaluate(() => scrollY)).toBeGreaterThan(0);
    for (let i = 0; i < 4; i++) await page.keyboard.press("p");
    await expect.poll(() => pillOf(page)).toBe("1 of 5 · n next · p back");
    await onWholeDoc(page);
    await scrollSettled(page);
    const v = await view(page);
    const rows = await docRows(page);
    expect(rows.map((r) => r.text).join("\n")).toContain("is the test ladder still needed?");
    for (const r of rows) expect(r.top, r.text).toBeGreaterThanOrEqual(v.top - 0.5);
  });
});

// --- Keys on whole doc --------------------------------------------------------------------------

test.describe("keys on whole doc", () => {
  test("l, ., x and y say what to do instead and write nothing", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("fresh"), { state: fixtureState("fresh") });
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await onWholeDoc(page);
    const said: Record<string, string | null> = {};
    for (const key of ["l", ".", "x", "y"]) {
      await page.evaluate(() => (window as unknown as { BR: { chrome: { pill(t: string): void } } }).BR.chrome.pill(""));
      await page.keyboard.press(key);
      await expect.poll(() => pillOf(page), { message: key }).not.toBeNull();
      said[key] = await pillOf(page);
    }
    expect(said).toEqual({
      l: "later needs a block · j",
      ".": "your call needs a block · j",
      x: "whole doc: c to say what's wrong",
      y: `${await turnKey(page)} approves`,
    });
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
    expect(await focused(page)).toBe("doc");
  });

  test("u says nothing here to revert, writes nothing and shows no source", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("fresh"), { state: fixtureState("fresh") });
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await onWholeDoc(page);
    await page.keyboard.press("u");
    await expect.poll(() => pillOf(page)).toBe("nothing here to revert");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBe("doc");
    await expect(page.locator("#source-view")).toBeHidden();
  });

  test("⏎ opens and closes the lost-notes row, and focus stays on whole doc", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("lost"), { state: fixtureState("lost") });
    await expect.poll(async () => (await docRows(page)).map((r) => r.text).join("\n")).toContain("lost its place");
    await onWholeDoc(page);
    const lostRow = async () => (await docRows(page)).find((r) => r.kind === "lost")?.text ?? "";
    expect(await lostRow()).not.toContain("was on:");
    await page.keyboard.press("Enter");
    await expect.poll(lostRow).toContain("was on:");
    expect(await focused(page)).toBe("doc");
    await page.keyboard.press("Enter");
    await expect.poll(lostRow).not.toContain("was on:");
    expect(await focused(page)).toBe("doc");
  });

  test("⏎ with only a folder row opens the menu on its folder row", async ({ page, birta }) => {
    const doc = "# PR\n\nText.\n";
    const folder = { summary: { ...summaryOf("drafts"), notes: 2 } };
    await open(page, birta, doc, {
      state: withRev(fixtureState("fresh"), revOf(doc)),
      dir: { ...dirState([listed("b.md")]), folder },
      files: { "b.md": "# B\n" },
      args: ["."],
    });
    await expect.poll(async () => (await docRows(page)).map((r) => r.text)).toContain("2 notes on this folder ▸");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await onWholeDoc(page);
    await page.keyboard.press("Enter");
    await expect.poll(() => menuRows(page)).not.toBeNull();
    const current = (await menuRows(page))!.filter((r) => r.current).map((r) => r.text);
    expect(current).toHaveLength(1);
    expect(current[0].startsWith("note on this folder")).toBe(true);
  });
});

/** Rows of the open phrase menu, and which is current. */
function menuRows(page: Page): Promise<{ text: string; current: boolean }[] | null> {
  return page.evaluate(() => {
    const root = document.querySelector("birta-pop")?.shadowRoot;
    const menu = root?.querySelector('[role="menu"]');
    if (!menu || !menu.matches(":popover-open")) return null;
    return Array.from(menu.querySelectorAll('[role="menuitem"]')).map((r) => ({
      text: (r.textContent ?? "").replace(/\s+/g, " ").trim(),
      current: r.classList.contains("current"),
    }));
  });
}

// --- A title is an ordinary block ---------------------------------------------------------------

test.describe("a title", () => {
  test("x, l and . on an H1 that spans the doc are on the title line only, and say so", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const [h1] = (await stops(page)).map((s) => s.sp);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(h1);
    const said: string[] = [];
    for (const key of ["x", "l", "."]) {
      const n = writes(mock).length;
      await page.keyboard.press(key);
      await expect.poll(() => writes(mock).length, { message: key }).toBe(n + 1);
      await expect.poll(() => pillOf(page), { message: key }).toMatch(/\(title\)/);
      said.push((await pillOf(page))!);
    }
    expect(writes(mock).map((o) => ({ at: o.at, verdict: o.verdict }))).toEqual([
      { at: { k: "block", sp: h1, scope: "block" }, verdict: "no" },
      { at: { k: "block", sp: h1, scope: "block" }, verdict: "later" },
      { at: { k: "block", sp: h1, scope: "block" }, verdict: "your_call" },
    ]);
    expect(said).toEqual([
      "no · # Upload retries (title) · z to undo",
      "later · # Upload retries (title) · z to undo",
      "your call · # Upload retries (title) · z to undo",
    ]);
  });

  test("a long title's message names it as a title", async ({ page, birta }) => {
    const doc = "# Add retries with backoff to the upload client\n\n## Why\n\nBody words.\n";
    const { mock } = await open(page, birta, doc);
    const [h1] = (await stops(page)).map((s) => s.sp);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(h1);
    await page.keyboard.press("x");
    await expect.poll(() => writes(mock)).toEqual([{ op: "comment", at: { k: "block", sp: h1, scope: "block" }, verdict: "no" }]);
    await expect.poll(() => pillOf(page)).toMatch(/^no · # Add retries\b.* \(title\) · z to undo$/);
  });
});

// --- Reloads, new tabs and other files ----------------------------------------------------------

test.describe("a reload", () => {
  test("opens on the stop the tab had, brought into view; on whole doc, on whole doc", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const sp = await stopSp(page, "Filler paragraph 30.");
    await focusTo(page, sp);
    await page.reload();
    await booted(page);
    await expect.poll(() => focused(page)).toBe(sp);
    const m = (await focusMark(page))!;
    const v = await view(page);
    expect(m.top).toBeGreaterThanOrEqual(v.top - 1);
    expect(m.bottom).toBeLessThanOrEqual(v.bottom + 1);
    await page.keyboard.press("Home");
    await onWholeDoc(page);
    await page.reload();
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    await onWholeDoc(page);
  });

  test("a new tab, or another file in the same tab, opens on whole doc", async ({ page, context, birta }) => {
    const { served } = await open(page, birta, STOPS_DOC, { files: { "b.md": STOPS_DOC.replace("Upload retries", "Other doc") }, args: ["."] });
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    const other = await context.newPage();
    await other.addInitScript(OPEN_ROOTS);
    await cacheCdn(other);
    await other.setViewportSize(WIDE);
    await other.goto(`${served.url}/view/pr.md`);
    await booted(other);
    await page.waitForTimeout(SETTLE_MS);
    await onWholeDoc(other);
    await other.close();
    // The same tab, another path: the stop it kept is pr.md's alone
    await page.goto(`${served.url}/view/b.md`);
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    await onWholeDoc(page);
  });
});

// --- Arriving in a file -------------------------------------------------------------------------

/** A doc's `dir_state` summary, as `review::summarize` builds it from the fold. */
function summaryOf(name: string): State {
  const s = fixtureState(name);
  return { stage: s.phrase.stage, phrase: s.phrase, counts: s.counts };
}

function listed(paths: string | string[], sum: State = null, news = false): State {
  return { paths: ([] as string[]).concat(paths), summary: sum, news };
}

function dirState(files: State[]): State {
  return { type: "dir_state", root: "", truncated: false, folder: null, files, totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0 } };
}

// news.md is unseen.md with a line more and no whole-doc note, so its first walk stop is a block
const NEWS_DOC = fixtureDoc("unseen") + "\nTail words.\n";
const NEWS_FIRST = "5:1-6:68";
const ARRIVE: Record<string, string> = {
  "a.md": "# A\n\nAlpha words.\n",
  "pr.md": "# PR\n\nSee [news](news.md) and [c](sub/c.md).\n\nClosing words.\n",
  "fresh.md": "# Fresh\n\nNothing here.\n",
  "news.md": NEWS_DOC,
  "sub/c.md": "# C\n\nGamma words.\n",
};

function newsState(): State {
  const s = withRev(fixtureState("unseen"), revOf(NEWS_DOC));
  s.items = s.items.filter((i: State) => i.n !== 6);
  return s;
}

/** Every page's state: news.md's, and an empty one for the others. */
function arriveStates(): State[] {
  const empty = (doc: string) => withRev(fixtureState("fresh"), revOf(doc));
  return [newsState(), ...["a.md", "pr.md", "fresh.md", "sub/c.md"].map((p) => empty(ARRIVE[p]))];
}

/** Files with notes: a, pr, news and sub/c; fresh.md has none, so ⇧J passes it. */
const ARRIVE_DIR = () =>
  dirState([
    listed("a.md", summaryOf("drafts")),
    listed("pr.md", summaryOf("drafts")),
    listed("fresh.md", summaryOf("fresh")),
    listed("news.md", null, true),
    listed("sub/c.md", summaryOf("drafts")),
  ]);

async function openArrive(page: Page, birta: Start, at = "pr.md") {
  const served = await birta(ARRIVE, GATE_ON, ["."]);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { states: arriveStates(), dir: ARRIVE_DIR() });
  await page.goto(`${served.url}/view/${at}`);
  await booted(page);
  await expect.poll(() => phraseOf(page)).not.toBeNull();
  return { served, mock };
}

/** Waits for the page at `path` to boot with its state. */
async function arrived(page: Page, url: string, path: string) {
  await page.waitForURL(`${url}/view/${path}`);
  await booted(page);
  await expect.poll(() => phraseOf(page)).not.toBeNull();
}

test.describe("arriving in a file", () => {
  test("a followed link lands on the file's first walk stop", async ({ page, birta }) => {
    const { served } = await openArrive(page, birta);
    await focusTo(page, await stopSp(page, "See news"));
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    await arrived(page, served.url, "news.md");
    await expect.poll(() => focused(page)).toBe(NEWS_FIRST);
  });

  test("a followed link to a file with nothing to walk lands on whole doc", async ({ page, birta }) => {
    const { served } = await openArrive(page, birta);
    await focusTo(page, await stopSp(page, "See news"));
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    await arrived(page, served.url, "sub/c.md");
    await onWholeDoc(page);
  });

  test("⇧J and ⇧K name the file and its place, land by the arrival rule, and never wrap", async ({ page, birta }) => {
    const { served } = await openArrive(page, birta);
    await page.keyboard.press("Shift+J");
    await arrived(page, served.url, "news.md");
    await expect.poll(() => pillOf(page)).toBe("news.md · 3 of 4 files with notes");
    await expect.poll(() => focused(page)).toBe(NEWS_FIRST);

    await page.keyboard.press("Shift+J");
    await arrived(page, served.url, "sub/c.md");
    await expect.poll(() => pillOf(page)).toBe("sub/c.md · 4 of 4 files with notes");
    await onWholeDoc(page);

    // The last file with notes: no wrap to a.md
    await page.keyboard.press("Shift+J");
    await page.waitForTimeout(SETTLE_MS);
    expect(viewing(page), "no wrap").toBe("/view/sub/c.md");
    await expect.poll(() => pillOf(page)).toBe("no more files with notes · ⇧K back");

    await page.keyboard.press("Shift+K");
    await arrived(page, served.url, "news.md");
    await expect.poll(() => pillOf(page)).toBe("news.md · 3 of 4 files with notes");
    await expect.poll(() => focused(page)).toBe(NEWS_FIRST);
  });

  test("⇧K from the first file with notes says so and stays: no wrap to the last", async ({ page, birta }) => {
    await openArrive(page, birta, "a.md");
    await page.keyboard.press("Shift+K");
    await page.waitForTimeout(SETTLE_MS);
    expect(viewing(page), "no wrap").toBe("/view/a.md");
    await expect.poll(() => pillOf(page)).toBe("no more files with notes · ⇧J back");
  });

  test("arriving again lands by the arrival rule, not on the stop the file had", async ({ page, birta }) => {
    const { served } = await openArrive(page, birta);
    await page.keyboard.press("Shift+J");
    await arrived(page, served.url, "news.md");
    await expect.poll(() => focused(page)).toBe(NEWS_FIRST);
    await focusTo(page, "11:1-11:44");
    await page.keyboard.press("Shift+K");
    await arrived(page, served.url, "pr.md");
    await page.keyboard.press("Shift+J");
    await arrived(page, served.url, "news.md");
    await expect.poll(() => focused(page)).toBe(NEWS_FIRST);
    // A reload, though, keeps the stop it had
    await focusTo(page, "11:1-11:44");
    await page.reload();
    await booted(page);
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
  });
});

// --- Adversary ----------------------------------------------------------------------------------

/** The top and bottom of the doc block at `sp`. */
function blockEdges(page: Page, sp: string): Promise<{ top: number; bottom: number }> {
  return page.evaluate((sp) => {
    const r = document.querySelector(`#content [data-sourcepos="${sp}"]`)!.getBoundingClientRect();
    return { top: r.top, bottom: r.bottom };
  }, sp);
}

test.describe("adversary: whole doc", () => {
  test("the title's focus washes the title line only, while ## Why's still washes its section", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const [h1] = (await stops(page)).map((s) => s.sp);
    const why = await stopSp(page, "Why");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(h1);
    // Its verdicts are on the title line, so the scope it shows must be no wider
    const title = await blockEdges(page, h1);
    const titleScope = (await marks(page)).filter((m) => m.mark === "scope");
    for (const s of titleScope) expect(s.bottom, "title scope reaches past the title").toBeLessThanOrEqual(title.bottom + 4);
    await focusTo(page, why);
    const heading = await blockEdges(page, why);
    const whyScope = (await marks(page)).find((m) => m.mark === "scope");
    expect(whyScope, "## Why still washes its section").toBeTruthy();
    expect(whyScope!.bottom).toBeGreaterThan(heading.bottom + 20);
  });

  test("Home from far down on an empty whole doc shows its line fully between the header and the window's bottom", async ({
    page,
    birta,
  }) => {
    await open(page, birta, longDoc());
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, 4000);
    await scrollSettled(page);
    await expect.poll(() => focused(page)).not.toBe("doc");
    expect((await docStop(page))?.shown ?? false, "empty and unfocused: hidden").toBe(false);
    await page.keyboard.press("Home");
    await onWholeDoc(page);
    await scrollSettled(page);
    const v = await view(page);
    const stop = (await docStop(page))!;
    expect(stop.shown).toBe(true);
    expect(stop.text).toBe("whole doc");
    expect(stop.top).toBeGreaterThanOrEqual(v.top - 0.5);
    expect(stop.bottom).toBeLessThanOrEqual(v.bottom);
    expect(await chipFocused(page)).toBe(false);
  });

  test("a reload restored onto a block leaves the empty whole doc line hidden", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const sp = await stopSp(page, "Filler paragraph 30.");
    await focusTo(page, sp);
    await page.reload();
    await booted(page);
    await expect.poll(() => focused(page)).toBe(sp);
    await page.waitForTimeout(SETTLE_MS);
    expect((await docStop(page))?.shown ?? false).toBe(false);
  });

  test("a reload restores this tab's stop, not the one another tab on the same path moved to later", async ({
    page,
    context,
    birta,
  }) => {
    const { served } = await open(page, birta, longDoc());
    const mine = await stopSp(page, "Filler paragraph 30.");
    const theirs = await stopSp(page, "Filler paragraph 10.");
    await focusTo(page, mine);
    const other = await context.newPage();
    await other.addInitScript(OPEN_ROOTS);
    await cacheCdn(other);
    await other.setViewportSize(WIDE);
    await mockReview(other, {});
    await other.goto(`${served.url}/view/pr.md`);
    await booted(other);
    await onWholeDoc(other);
    await focusTo(other, theirs);
    await page.reload();
    await booted(page);
    await expect.poll(() => focused(page)).toBe(mine);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(mine);
    await other.reload();
    await booted(other);
    await expect.poll(() => focused(other)).toBe(theirs);
    await other.waitForTimeout(SETTLE_MS);
    expect(await focused(other)).toBe(theirs);
    await other.close();
  });

  test("⏎ on whole doc with nothing folded opens nothing, follows no link, and writes nothing", async ({ page, birta }) => {
    const doc = "See [more](#more) below.\n\n## More\n\nWords.\n";
    const { mock } = await open(page, birta, doc);
    await onWholeDoc(page);
    const url = page.url();
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(page.url()).toBe(url);
    expect(await menuRows(page)).toBeNull();
    expect(await page.locator('birta-flow[data-kind="composer"]').count()).toBe(0);
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBe("doc");
  });
});

test.describe("adversary: arriving in a file", () => {
  test("a link followed by a mouse click lands on the file's first walk stop", async ({ page, birta }) => {
    const { served } = await openArrive(page, birta);
    await page.locator('#content a[href$="news.md"]').click();
    await arrived(page, served.url, "news.md");
    await expect.poll(() => focused(page)).toBe(NEWS_FIRST);
    expect(await chipFocused(page)).toBe(false);
  });

  test("after arriving by a link, a reload keeps the stop the tab had", async ({ page, birta }) => {
    const { served } = await openArrive(page, birta);
    await focusTo(page, await stopSp(page, "See news"));
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    await arrived(page, served.url, "news.md");
    await expect.poll(() => focused(page)).toBe(NEWS_FIRST);
    await focusTo(page, "11:1-11:44");
    await page.reload();
    await booted(page);
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    // A late arrival rule must not take the restored stop back
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe("11:1-11:44");
  });

  test("opening a file with news straight from its address is no arrival: it opens on whole doc", async ({ page, birta }) => {
    await openArrive(page, birta, "news.md");
    await onWholeDoc(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe("doc");
    expect(await chipFocused(page)).toBe(false);
  });
});

// Guards: these pass today, and fail if "title" loosens to "an H1 whose section reaches the end"
test.describe("adversary guards: what is not a title", () => {
  test("an H1 after an intro is no title: x there is no on its section", async ({ page, birta }) => {
    const { mock } = await open(page, birta, "Intro words.\n\n# Title\n\nBody words.\n");
    const h1 = await stopSp(page, "Title");
    await focusTo(page, h1);
    await page.keyboard.press("x");
    await expect.poll(() => writes(mock)).toEqual([{ op: "comment", at: { k: "block", sp: h1, scope: "section" }, verdict: "no" }]);
    await expect.poll(() => pillOf(page)).toBe("no · # Title (section) · z to undo");
  });
});

// --- Repair: review findings ----------------------------------------------------------------

test.describe("repair", () => {
  test("a reload after a write above the focus opens on the same block, at its new lines", async ({ page, birta }) => {
    const doc = longDoc();
    const { served } = await open(page, birta, doc);
    const was = await stopSp(page, "Filler paragraph 30.");
    await focusTo(page, was);
    served.write("pr.md", doc.replace("## Why\n", "## Why\n\nA new first paragraph.\n"));
    const live = () =>
      page.evaluate(() => {
        const p = Array.from(document.querySelectorAll("#content p")).find((e) => e.textContent === "Filler paragraph 30.");
        return p ? p.getAttribute("data-sourcepos") : null;
      });
    await expect.poll(live).not.toBe(was);
    const now = (await live())!;
    await expect.poll(() => focused(page)).toBe(now);
    await page.waitForTimeout(SETTLE_MS);
    await page.reload();
    await booted(page);
    await expect.poll(() => focused(page)).toBe(now);
    const text = await page.evaluate((sp) => document.querySelector(`#content [data-sourcepos="${sp}"]`)?.textContent ?? null, now);
    expect(text).toBe("Filler paragraph 30.");
  });

  test("your scroll back to the top of an empty whole doc puts the focus on whole doc", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    await focusTo(page, await stopSp(page, "Filler paragraph 20."));
    expect((await docStop(page))?.shown ?? false).toBe(false);
    await page.mouse.move(640, 500);
    await page.mouse.wheel(0, -100_000);
    await scrollSettled(page);
    expect(await page.evaluate(() => scrollY)).toBe(0);
    await onWholeDoc(page);
    await expect.poll(async () => (await docStop(page))?.shown ?? false).toBe(true);
  });
});
