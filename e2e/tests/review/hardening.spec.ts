import { Page } from "@playwright/test";
import { spawnSync } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import {
  test,
  testClosed,
  expect,
  Box,
  GATE_ON,
  BIN,
  REPO,
  ensureBuilt,
  Rgb,
  State,
  booted,
  contentBox,
  distance,
  fixtureDoc,
  fixtureState,
  frameBox,
  hex,
  highlighted,
  ink,
  laneState,
  marks,
  misaligned,
  mockReview,
  near,
  over,
  pixelsOf,
  revOf,
  reviewLog,
  shot,
  withRev,
} from "./harness";

// The phrase prints local times; UTC lets the spec name them from the epoch alone
test.use({ timezoneId: "UTC" });

const WIDE = { width: 1280, height: 900 };
const NARROW = { width: 520, height: 900 };
const SETTLE_MS = 400;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

interface OpenOpts {
  state?: State;
  dir?: State;
  args?: string[];
  files?: Record<string, string>;
  viewport?: { width: number; height: number };
}

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, opts: OpenOpts = {}) {
  const served = await birta({ "pr.md": doc, ...(opts.files ?? {}) }, GATE_ON, opts.args);
  await page.setViewportSize(opts.viewport ?? WIDE);
  const mock = await mockReview(page, { state: opts.state, dir: opts.dir });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

/** Text of a host's shadow root (or the host), style sheets left out. */
function textIn(page: Page, sel: string): Promise<string | null> {
  return page.evaluate((sel) => {
    const h = document.querySelector(sel);
    if (!h || !h.getClientRects().length) return null;
    let out = "";
    const walk = (x: Node) => {
      if (x.nodeType === 3) out += x.textContent;
      else if (x.nodeType === 1 && (x as Element).localName !== "style") x.childNodes.forEach(walk);
    };
    (h.shadowRoot ?? h).childNodes.forEach(walk);
    return out.replace(/\s+/g, " ").trim();
  }, sel);
}
const phraseOf = (page: Page) => textIn(page, ".file-header birta-phrase");
const turnOf = (page: Page) => textIn(page, ".file-header birta-turn");

/** Each flow host's text and whether it is painted at all. */
function flowHosts(page: Page): Promise<{ kind: string | null; text: string; shown: boolean; doc: boolean }[]> {
  return page.evaluate(() => {
    const content = document.getElementById("content")!;
    return Array.from(document.querySelectorAll("birta-flow")).map((h) => {
      let out = "";
      const walk = (x: Node) => {
        if (x.nodeType === 3) out += x.textContent;
        else if (x.nodeType === 1 && (x as Element).localName !== "style") x.childNodes.forEach(walk);
      };
      (h.shadowRoot ?? h).childNodes.forEach(walk);
      const r = h.getBoundingClientRect();
      const cs = getComputedStyle(h);
      return {
        kind: h.getAttribute("data-kind"),
        text: out.replace(/\s+/g, " ").trim(),
        shown: r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.display !== "none",
        doc: !content.contains(h),
      };
    });
  });
}

/** `href`s of the doc links whose news dot is painted. */
function paintedDots(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll("#content a[data-br-news]"))
      .filter((a) => {
        const cs = getComputedStyle(a, "::after");
        return cs.content !== "none" && cs.display !== "none" && parseFloat(cs.width) > 0 && a.getClientRects().length > 0;
      })
      .map((a) => a.getAttribute("href")!),
  );
}

/** Whether the lane's marks are painted (both lane hosts, and at least one mark in them). */
function laneShown(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const root = document.querySelector("#lane-frame > birta-over")?.shadowRoot;
    if (!root) return false;
    return Array.from(root.querySelectorAll("[data-mark]")).some((el) => {
      const r = el.getBoundingClientRect();
      return r.width > 0 && r.height > 0;
    });
  });
}

/** The unseen fixture plus a link to a file with news, so every review surface is on the page. */
const LINKED = fixtureDoc("unseen") + "\nSee [b](b.md).\n";
const LINKED_DIR = {
  type: "dir_state",
  root: "",
  truncated: false,
  folder: null,
  files: [{ paths: ["b.md"], summary: null, news: true }],
  totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0 },
};

async function openLinked(page: Page, birta: Start, opts: OpenOpts = {}, doc = LINKED) {
  const out = await open(page, birta, doc, {
    state: withRev(fixtureState("unseen"), revOf(doc)),
    dir: LINKED_DIR,
    files: { "b.md": "# B\n" },
    ...opts,
  });
  await expect.poll(async () => (await flowHosts(page)).filter((h) => h.shown).length).toBeGreaterThan(2);
  await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
  await expect.poll(() => paintedDots(page)).toEqual(["/view/b.md"]);
  return out;
}

// --- Copy ---------------------------------------------------------------------------------

/** Selects from the start of the block holding `from` to the end of the block holding `to`. */
async function selectBlocks(page: Page, from: string, to: string) {
  await page.evaluate(
    ({ from, to }) => {
      // The deepest block holding the text: an item comes after the list that shares its sp
      const blocks = Array.from(document.querySelectorAll("#content [data-sourcepos]"));
      const own = (text: string) => blocks.filter((b) => (b.textContent ?? "").includes(text)).pop()!;
      const a = own(from);
      const b = own(to);
      const first = document.createTreeWalker(a, NodeFilter.SHOW_TEXT).nextNode() as Text;
      const walker = document.createTreeWalker(b, NodeFilter.SHOW_TEXT);
      let last: Text | null = null;
      for (let n = walker.nextNode(); n; n = walker.nextNode()) if ((n.textContent ?? "").trim()) last = n as Text;
      const r = document.createRange();
      r.setStart(first, 0);
      r.setEnd(last!, last!.data.length);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    },
    { from, to },
  );
}

/** Fires a copy event with its own DataTransfer: what the page put there, and whether it took over. */
function syntheticCopy(page: Page): Promise<{ text: string; html: string; prevented: boolean }> {
  return page.evaluate(() => {
    const data = new DataTransfer();
    const e = new ClipboardEvent("copy", { clipboardData: data, bubbles: true, cancelable: true, composed: true });
    const target = getSelection()!.anchorNode?.parentElement ?? document.body;
    target.dispatchEvent(e);
    return { text: data.getData("text/plain"), html: data.getData("text/html"), prevented: e.defaultPrevented };
  });
}

const squash = (s: string) => s.replace(/\s+/g, " ").trim();
/** Words only review hosts hold, never the doc. */
const HOST_WORDS = ["say why", "Yes: CI runs it", "y keep", "not handled", "drop this bullet", "Said why"];

// Closed roots, as shipped: how a selection over a host serialises can differ from an open root
testClosed.describe("copy", () => {
  testClosed.use({ timezoneId: "UTC" });

  testClosed("a selection over notes copies the doc's text alone, with its paragraphs on their own lines", async ({
    page,
    birta,
  }) => {
    const doc = fixtureDoc("unseen");
    const { mock } = await open(page, birta, doc);
    await selectBlocks(page, "When the upload", "retries on 503");
    const plain = await page.evaluate(() => getSelection()!.toString());
    expect(plain).toContain("When the upload");
    mock.pushState(fixtureState("unseen"));
    await expect.poll(() => page.locator("birta-flow").count()).toBeGreaterThan(2);
    await selectBlocks(page, "When the upload", "retries on 503");
    const got = await syntheticCopy(page);
    expect(got.prevented).toBe(true);
    for (const w of HOST_WORDS) expect(got.text).not.toContain(w);
    expect(squash(got.text)).toBe(squash(plain));
    // The paragraph, the heading and the item each start a line
    expect(got.text).toMatch(/fixes most of these\.\s*\n\s*What changes\s*\n/);
  });

  testClosed("a selection that touches no note is left to the browser", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => page.locator("birta-flow").count()).toBeGreaterThan(2);
    await selectBlocks(page, "What changes", "What changes");
    const got = await syntheticCopy(page);
    expect(got.prevented).toBe(false);
    expect(got.text).toBe("");
  });

  testClosed("a selection inside the composer is left to the composer", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await page.keyboard.press("c");
    await page.keyboard.type("draft words");
    await page.keyboard.press("ControlOrMeta+a");
    const prevented = await page.evaluate(() => {
      const e = new ClipboardEvent("copy", { clipboardData: new DataTransfer(), bubbles: true, cancelable: true, composed: true });
      (document.activeElement ?? document.body).dispatchEvent(e);
      return e.defaultPrevented;
    });
    expect(prevented).toBe(false);
  });

  testClosed("the real clipboard gets the doc's text alone", async ({ page, birta, browserName, context }) => {
    testClosed.skip(browserName !== "chromium", "only Chromium lets a test read the system clipboard");
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    const { mock } = await open(page, birta, fixtureDoc("unseen"));
    await selectBlocks(page, "When the upload", "retries on 503");
    const plain = await page.evaluate(() => getSelection()!.toString());
    mock.pushState(fixtureState("unseen"));
    await expect.poll(() => page.locator("birta-flow").count()).toBeGreaterThan(2);
    await selectBlocks(page, "When the upload", "retries on 503");
    await page.keyboard.press("ControlOrMeta+c");
    const text = await page.evaluate(() => navigator.clipboard.readText());
    for (const w of HOST_WORDS) expect(text).not.toContain(w);
    expect(squash(text)).toBe(squash(plain));
  });

  // K5a
  testClosed("a selection from mid-sentence to mid-item copies exactly the words selected", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"));
    const select = () =>
      page.evaluate(() => {
        const find = (needle: string): [Text, number] => {
          const w = document.createTreeWalker(document.getElementById("content")!, NodeFilter.SHOW_TEXT);
          for (let n = w.nextNode(); n; n = w.nextNode()) {
            const i = (n as Text).data.indexOf(needle);
            if (i >= 0) return [n as Text, i];
          }
          throw new Error(needle);
        };
        const [a, i] = find("503 we just");
        const [b, j] = find("on 503");
        const r = document.createRange();
        r.setStart(a, i + 4);
        r.setEnd(b, j + 2);
        getSelection()!.removeAllRanges();
        getSelection()!.addRange(r);
      });
    await select();
    const plain = await page.evaluate(() => getSelection()!.toString());
    mock.pushState(fixtureState("unseen"));
    await expect.poll(() => page.locator("birta-flow").count()).toBeGreaterThan(2);
    await select();
    const got = await syntheticCopy(page);
    expect(got.prevented).toBe(true);
    expect(got.text.startsWith("we just fail")).toBe(true);
    expect(got.text.trimEnd().endsWith("retries on")).toBe(true);
    expect(squash(got.text)).toBe(squash(plain));
  });

  // K5b
  testClosed("hidden text stays out, and a code block keeps its lines and indent", async ({ page, birta }) => {
    // Appended after the fixture's blocks, so its state's sourcepos still holds
    const doc =
      fixtureDoc("unseen") + "\n<details><summary>More</summary>\n\nHIDDEN-WORDS\n\n</details>\n\n```\nfirst  line\n  second line\n```\n";
    const { mock } = await open(page, birta, doc);
    await selectBlocks(page, "When the upload", "second line");
    const plain = await page.evaluate(() => getSelection()!.toString());
    mock.pushState(withRev(fixtureState("unseen"), revOf(doc)));
    await expect.poll(() => page.locator("birta-flow").count()).toBeGreaterThan(2);
    await selectBlocks(page, "When the upload", "second line");
    const got = await syntheticCopy(page);
    expect(got.prevented).toBe(true);
    // Chromium's own toString sometimes includes a closed details' body, so that is no oracle here
    expect(got.text).not.toContain("HIDDEN-WORDS");
    expect(squash(got.text)).toContain(squash(plain.split("More")[0]));
    expect(got.text).toContain("first  line\n  second line");
    for (const w of HOST_WORDS) expect(got.text).not.toContain(w);
  });

  // K5c: the real clipboard, since a handler that stays out leaves a synthetic one empty
  testClosed("a selection inside a note copies the note's words", async ({ page, birta, browserName, context }) => {
    testClosed.skip(browserName !== "chromium", "only Chromium lets a test read the system clipboard");
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    const note = page.locator('#content birta-flow[data-kind="note"]').first();
    await expect(note).toBeVisible();
    await note.click({ clickCount: 3 });
    await page.keyboard.press("ControlOrMeta+c");
    const text = await page.evaluate(() => navigator.clipboard.readText());
    expect(HOST_WORDS.some((w) => text.includes(w)), text).toBe(true);
  });

  // K5d
  testClosed("a copy with nothing selected throws nothing and is left to the browser", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => page.locator("birta-flow").count()).toBeGreaterThan(2);
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.evaluate(() => getSelection()!.removeAllRanges());
    expect((await syntheticCopy(page)).prevented).toBe(false);
    expect(errors).toEqual([]);
  });

  // K5e
  testClosed("a selection from the header into the doc leaves the folder row off the clipboard", async ({
    page,
    birta,
    browserName,
    context,
  }) => {
    testClosed.skip(browserName !== "chromium", "only Chromium lets a test read the system clipboard");
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    const doc = "# PR\n\nText.\n";
    await open(page, birta, doc, {
      state: withRev(fixtureState("fresh"), revOf(doc)),
      dir: folderListing(2),
      files: FOLDER_FILES,
      args: ["."],
    });
    await expect.poll(() => page.locator("birta-flow").count()).toBeGreaterThan(0);
    await page.evaluate(() => {
      const r = document.createRange();
      r.setStart(document.querySelector(".file-header-name")!.firstChild!, 0);
      const last = document.querySelector("#content p")!.firstChild as Text;
      r.setEnd(last, last.data.length);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    });
    await page.keyboard.press("ControlOrMeta+c");
    const text = await page.evaluate(() => navigator.clipboard.readText());
    expect(text).toContain("Text.");
    expect(text).not.toContain("on this folder");
  });
});

// --- m and reading mode -------------------------------------------------------------------

test.describe("m and reading mode", () => {
  // m hides marks only; the notes stay in the flow (pinned in flow.spec)
  test("m hides the link dots and the quote underline as well as the lane, and keeps the chrome", async ({
    page,
    birta,
  }) => {
    await openLinked(page, birta);
    const phrase = await phraseOf(page);
    const turn = await turnOf(page);
    const notes = (await flowHosts(page)).filter((h) => h.shown).map((h) => h.text);
    await page.keyboard.press("m");
    await expect.poll(() => laneShown(page)).toBe(false);
    await expect.poll(() => highlighted(page)).toEqual([]);
    await expect.poll(() => paintedDots(page)).toEqual([]);
    expect(await phraseOf(page)).toBe(phrase);
    expect(await turnOf(page)).toBe(turn);
    expect((await flowHosts(page)).filter((h) => h.shown).map((h) => h.text)).toEqual(notes);

    await page.keyboard.press("m");
    await expect.poll(() => laneShown(page)).toBe(true);
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
    await expect.poll(() => paintedDots(page)).toEqual(["/view/b.md"]);
    await expect.poll(() => misaligned(page)).toEqual([]);
  });

  // Reading mode hides every real note and mark, so anything note-like left is the doc's own
  test("in reading mode a doc's note-like HTML stays while the real notes hide", async ({ page, birta }) => {
    const doc = LINKED + '\n<div class="lookalike">↳ Claude · done · 7 words → 10 · y keep · u revert · c note</div>\n';
    await openLinked(page, birta, { state: withRev(fixtureState("unseen"), revOf(doc)) }, doc);
    const lookalike = page.locator("#content .lookalike");
    await expect(lookalike).toBeVisible();
    await page.keyboard.press("r");
    await expect.poll(async () => (await flowHosts(page)).filter((h) => h.shown).length).toBe(0);
    await expect.poll(() => laneShown(page)).toBe(false);
    await expect(lookalike).toBeVisible();
  });

  test("reading mode hides the notes, the quote underline, the link dots and the lane, and leaving it brings them back", async ({
    page,
    birta,
  }) => {
    await openLinked(page, birta);
    await page.keyboard.press("r");
    await expect.poll(() => page.evaluate(() => document.body.classList.contains("reading-mode"))).toBe(true);
    await expect.poll(() => laneShown(page)).toBe(false);
    await expect.poll(async () => (await flowHosts(page)).filter((h) => h.shown).map((h) => h.text)).toEqual([]);
    await expect.poll(() => highlighted(page)).toEqual([]);
    await expect.poll(() => paintedDots(page)).toEqual([]);
    expect(await phraseOf(page)).toBeNull();

    await page.keyboard.press("r");
    await expect.poll(() => laneShown(page)).toBe(true);
    await expect.poll(async () => (await flowHosts(page)).filter((h) => h.shown).length).toBeGreaterThan(2);
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
    await expect.poll(() => paintedDots(page)).toEqual(["/view/b.md"]);
    expect(await phraseOf(page)).not.toBeNull();
    await expect.poll(() => misaligned(page)).toEqual([]);
  });

  for (const how of ["m", "r"]) {
    test(`with the marks hidden by ${how}, c still opens a composer you can see and keeps the note`, async ({ page, birta }) => {
      const { mock } = await openLinked(page, birta);
      await page.keyboard.press(how);
      await expect.poll(() => laneShown(page)).toBe(false);
      await page.keyboard.press("c");
      await expect(page.locator('birta-flow[data-kind="composer"] textarea')).toBeFocused();
      await expect(page.locator('birta-flow[data-kind="composer"] textarea')).toBeVisible();
      await page.keyboard.type("read on");
      await page.keyboard.press("Enter");
      await expect.poll(() => mock.ops().filter((o) => o.op === "comment").map((o) => o.body)).toEqual(["read on"]);
    });
  }

  test("in reading mode the review keys still act", async ({ page, birta }) => {
    const { mock } = await openLinked(page, birta);
    await page.keyboard.press("r");
    await expect.poll(() => page.evaluate(() => document.body.classList.contains("reading-mode"))).toBe(true);
    await page.keyboard.press("j");
    await page.keyboard.press("c");
    await page.keyboard.type("read on");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().filter((o) => o.op === "comment").map((o) => o.body)).toEqual(["read on"]);
  });

  /** Whether the under layer paints anything: washes, underlays. */
  const underShown = (page: Page) =>
    page.evaluate(() => {
      const root = document.querySelector("#lane-frame > birta-under")?.shadowRoot;
      return (
        !!root &&
        Array.from(root.querySelectorAll("[data-mark]")).some((el) => {
          const r = el.getBoundingClientRect();
          return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== "hidden";
        })
      );
    });

  // K6a
  for (const how of ["m", "r"]) {
    test(`${how} stays on through a new state and a new listing`, async ({ page, birta }) => {
      const { mock } = await openLinked(page, birta);
      await page.keyboard.press(how);
      await expect.poll(() => laneShown(page)).toBe(false);
      const next = withRev(fixtureState("unseen"), revOf(LINKED));
      next.server_now = (next.server_now ?? 0) + 1_000;
      mock.pushState(next);
      mock.push({ ...LINKED_DIR, totals: { ...LINKED_DIR.totals, items: 1 } });
      await page.waitForTimeout(SETTLE_MS);
      expect(await laneShown(page)).toBe(false);
      expect(await underShown(page)).toBe(false);
      expect(await highlighted(page)).toEqual([]);
      expect(await paintedDots(page)).toEqual([]);
      if (how === "r") expect((await flowHosts(page)).filter((h) => h.shown)).toEqual([]);
    });
  }

  // K6b: m and r are independent toggles
  test("leaving reading mode with m on keeps the marks hidden and brings the notes back", async ({ page, birta }) => {
    await openLinked(page, birta);
    await page.keyboard.press("m");
    await page.keyboard.press("r");
    await page.keyboard.press("r");
    await expect.poll(async () => (await flowHosts(page)).filter((h) => h.shown).length).toBeGreaterThan(2);
    expect(await laneShown(page)).toBe(false);
    expect(await highlighted(page)).toEqual([]);
    expect(await paintedDots(page)).toEqual([]);
    await page.keyboard.press("m");
    await expect.poll(() => laneShown(page)).toBe(true);
    await expect.poll(() => highlighted(page)).toEqual(["we just fail"]);
    await expect.poll(() => paintedDots(page)).toEqual(["/view/b.md"]);
  });

  test("m on, then reading mode on and off, then m off, and every mark is back", async ({ page, birta }) => {
    await openLinked(page, birta);
    await page.keyboard.press("r");
    await page.keyboard.press("m");
    await page.keyboard.press("r");
    expect(await laneShown(page)).toBe(false);
    await expect.poll(async () => (await flowHosts(page)).filter((h) => h.shown).length).toBeGreaterThan(2);
    await page.keyboard.press("m");
    await expect.poll(() => laneShown(page)).toBe(true);
    await expect.poll(() => paintedDots(page)).toEqual(["/view/b.md"]);
  });

  // K6c
  test("reading mode hides every real note and leaves a look-alike in the doc on show", async ({ page, birta }) => {
    const doc = fixtureDoc("unseen") + "\n<div><p>↳ Claude · done · LOOKALIKE said why</p></div>\n";
    await open(page, birta, doc, { state: withRev(fixtureState("unseen"), revOf(doc)) });
    await expect.poll(async () => (await flowHosts(page)).filter((h) => h.shown).length).toBeGreaterThan(2);
    await page.keyboard.press("r");
    await expect.poll(async () => (await flowHosts(page)).filter((h) => h.shown)).toEqual([]);
    await expect(page.locator("#content p", { hasText: "LOOKALIKE said why" })).toBeVisible();
  });

  test("a composer already open stays, with its words, when reading mode comes on", async ({ page, birta }) => {
    await openLinked(page, birta);
    await page.keyboard.press("c");
    await page.keyboard.type("half a thought");
    await page.locator("#content h1").click();
    await page.keyboard.press("r");
    await expect.poll(() => page.evaluate(() => document.body.classList.contains("reading-mode"))).toBe(true);
    const area = page.locator('birta-flow[data-kind="composer"] textarea');
    await expect(area).toBeVisible();
    await expect(area).toHaveValue("half a thought");
  });
});

// --- Reduced motion -----------------------------------------------------------------------

test.describe("reduced motion", () => {
  test("with reduced motion nothing review draws animates: lane, notes, header and popovers", async ({ page, birta }) => {
    await openLinked(page, birta);
    await page.locator(".file-header birta-phrase").click();
    const durations = () =>
      page.evaluate(() => {
        const out: { where: string; t: number; a: number }[] = [];
        const sum = (s: string) => Math.max(...s.split(",").map((d) => parseFloat(d) || 0));
        const look = (where: string, el: Element, pseudo?: string) => {
          const cs = getComputedStyle(el, pseudo);
          out.push({ where, t: sum(cs.transitionDuration), a: sum(cs.animationDuration) });
        };
        for (const tag of ["birta-under", "birta-over", "birta-flow", "birta-phrase", "birta-turn", "birta-pop"]) {
          for (const h of Array.from(document.querySelectorAll(tag))) {
            look(tag, h);
            h.shadowRoot?.querySelectorAll("*").forEach((el) => look(`${tag} ${el.localName}.${el.className}`, el));
          }
        }
        document.querySelectorAll("#content a[data-br-news]").forEach((a) => {
          look("link dot", a, "::after");
        });
        const name = document.querySelector(".file-header-name");
        if (name) look("file name", name);
        return out;
      });
    expect(Math.max(...(await durations()).map((d) => d.t))).toBeGreaterThan(0);
    await page.emulateMedia({ reducedMotion: "reduce" });
    await expect.poll(async () => (await durations()).filter((d) => d.t > 0 || d.a > 0).map((d) => d.where)).toEqual([]);
  });

  // K11
  test("with reduced motion from the start, a walk, a kept note, the pill and the menu start no animation", async ({
    page,
    birta,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await openLinked(page, birta);
    const running: string[] = [];
    const sample = async (when: string) =>
      running.push(
        ...(
          await page.evaluate(() => {
            const roots: (Document | ShadowRoot)[] = [document];
            document
              .querySelectorAll("birta-under, birta-over, birta-flow, birta-phrase, birta-turn, birta-pop")
              .forEach((h) => h.shadowRoot && roots.push(h.shadowRoot));
            return roots
              .flatMap((r) => r.getAnimations())
              .filter((a) => a.playState === "running")
              .map((a) =>
                String(
                  (a as CSSAnimation).animationName ?? (a as CSSTransition).transitionProperty ?? a.id,
                ),
              );
          })
        ).map((a) => `${when}: ${a}`),
      );
    await page.keyboard.press("n");
    await sample("n");
    await page.keyboard.press("c");
    await page.keyboard.type("x");
    await page.keyboard.press("Enter");
    await sample("keep");
    await page.keyboard.press("ControlOrMeta+Enter");
    await sample("pill");
    await page.locator(".file-header birta-phrase").click();
    await sample("menu");
    expect(running).toEqual([]);
  });
});

// --- Colour roles across the shipped themes -----------------------------------------------

type Variant = "light" | "dark";
type Palette = Record<string, string>;

/**
 * The themes the build bundles, read from `BUNDLED_THEMES` in `src/theme.rs`. The binary may be
 * stale when the test list is built, so the K7a test checks it agrees once it is rebuilt.
 */
function bundled(): string[] {
  const src = fs.readFileSync(path.join(REPO, "src", "theme.rs"), "utf-8");
  const table = src.slice(src.indexOf("static BUNDLED_THEMES"), src.indexOf("];", src.indexOf("static BUNDLED_THEMES")));
  return [...table.matchAll(/^\s*name: "([^"]+)",$/gm)].map((m) => m[1]).sort();
}

const BUNDLED = bundled();

/** What `--list-themes` says the rebuilt binary bundles, with no user theme in reach. */
function listedThemes(): string[] {
  ensureBuilt();
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-themes-"));
  try {
    const r = spawnSync(BIN, ["--list-themes"], {
      env: { PATH: process.env.PATH ?? "", HOME: home, XDG_CONFIG_HOME: path.join(home, ".config") },
      encoding: "utf-8",
    });
    if (r.status !== 0) throw new Error(`--list-themes exited ${r.status}: ${r.stderr}`);
    const names = (r.stdout ?? "")
      .split("\n")
      .map((l) => l.match(/^\s*(\S+)\s+\(bundled\)\s*$/)?.[1])
      .filter((n): n is string => !!n);
    if (!names.length) throw new Error(`--list-themes listed no bundled theme: ${r.stdout}`);
    return names.sort();
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
}

/** Each bundled theme's variants, read from its TOML (flat `key = "#hex"` lines). */
function themes(): { name: string; variant: Variant; p: Palette }[] {
  const dir = path.join(REPO, "assets", "themes");
  const out: { name: string; variant: Variant; p: Palette }[] = [];
  for (const file of BUNDLED.map((n) => `${n}.toml`).filter((f) => fs.existsSync(path.join(dir, f)))) {
    let name = "";
    let cur: Palette | null = null;
    for (const line of fs.readFileSync(path.join(dir, file), "utf-8").split("\n")) {
      const t = line.trim();
      const head = t.match(/^\[theme\.(light|dark)\]$/);
      if (head) {
        cur = {};
        out.push({ name, variant: head[1] as Variant, p: cur });
        continue;
      }
      if (t.startsWith("[")) {
        cur = null;
        continue;
      }
      const kv = t.match(/^(\w+)\s*=\s*"([^"]*)"/);
      if (!kv) continue;
      if (kv[1] === "name" && !cur) name = kv[2];
      else if (cur) cur[kv[1]] = kv[2];
    }
  }
  return out;
}

const THEMES = themes();

/** The roles a theme should resolve to; github keeps the page's own constants. */
function roles(name: string, variant: Variant, p: Palette) {
  const github = name === "github";
  return {
    bg: p.bg_primary,
    agent: p.fg_muted,
    you: github ? (variant === "dark" ? "#4493f8" : "#0969da") : p.alert_note,
    new: github ? (variant === "dark" ? "#bb800926" : "#fff8c5") : p.bg_attention_muted,
    border: p.border_birta,
  };
}

type Rgba = [number, number, number, number];

/** WCAG contrast of two colours. */
function contrast(a: Rgb, b: Rgb): number {
  const lum = (c: Rgb) => {
    const [r, g, bl] = c.map((v) => {
      const x = v / 255;
      return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi, lo] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (hi + 0.05) / (lo + 0.05);
}

function rgba(css: string): Rgba {
  const n = (css.match(/[\d.]+/g) ?? []).map(Number);
  // color(srgb r g b / a) carries 0–1 channels
  if (css.startsWith("color(")) return [n[0] * 255, n[1] * 255, n[2] * 255, n[3] ?? 1];
  return [n[0], n[1], n[2], n[3] ?? 1];
}

function hexRgba(h: string): Rgba {
  const [r, g, b] = hex(h);
  return [r, g, b, h.length === 9 ? parseInt(h.slice(7), 16) / 255 : 1];
}

function sameColour(css: string, want: string): boolean {
  const a = rgba(css);
  const b = hexRgba(want);
  return a.slice(0, 3).every((v, i) => Math.abs(v - b[i]) <= 2) && Math.abs(a[3] - b[3]) <= 0.02;
}

/** The colour `--name` resolves to inside each place review paints, by a probe element there. */
function roleSites(page: Page, name: string): Promise<Record<string, string>> {
  return page.evaluate((name) => {
    const probe = (parent: Element | ShadowRoot) => {
      const s = document.createElement("span");
      s.style.color = `var(${name})`;
      parent.appendChild(s);
      const c = getComputedStyle(s).color;
      s.remove();
      return c;
    };
    const flow = document.querySelector("#content birta-flow");
    return {
      lane: probe(document.querySelector("#lane-frame > birta-over")!.shadowRoot!),
      flow: flow ? probe(flow.shadowRoot!) : "missing",
      doc: probe(document.querySelector("#content p")!),
      header: probe(document.querySelector(".file-header")!),
      popover: probe(document.querySelector("birta-pop")!.shadowRoot!),
    };
  }, name);
}

const THEME_DOC = fixtureDoc("unseen") + "\nSee [b](b.md).\n";

for (const { name, variant, p } of THEMES) {
  test.describe(`theme ${name} ${variant}`, () => {
    test("every review colour role is the theme's, wherever review paints it", async ({ page, birta }) => {
      const want = roles(name, variant, p);
      const doc = THEME_DOC;
      await page.emulateMedia({ colorScheme: variant });
      // A seen bar keeps the agent's colour and an unseen one lays the wash; an ask is the accent, a sent no a disc
      const state = laneState(doc, {
        bars: [{ sp: "5:1-6:68" }, { sp: "11:1-11:44", seen: true }],
        glyphs: [
          { sp: "10:1-10:16", verdict: "ask" },
          { sp: "14:1-14:13", verdict: "no", status: "sent" },
        ],
      });
      await open(page, birta, doc, {
        state,
        dir: LINKED_DIR,
        files: { "b.md": "# B\n" },
        args: ["pr.md", "--theme", name, `--${variant}`],
      });
      await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(2);
      await expect.poll(() => page.locator("#content birta-flow").count()).toBeGreaterThan(0);
      await expect.poll(() => paintedDots(page)).toEqual(["/view/b.md"]);
      expect(await page.evaluate(() => document.documentElement.getAttribute("data-theme"))).toBe(variant);

      // The same role is one colour in the lane, the notes, the doc, the header and the popovers
      for (const [role, value] of [
        ["--review-you", want.you],
        ["--review-agent", want.agent],
        ["--review-new", want.new],
      ] as const) {
        const sites = await roleSites(page, role);
        for (const [site, css] of Object.entries(sites)) expect(sameColour(css, value), `${role} in ${site}: ${css} vs ${value}`).toBe(true);
      }

      // Painted marks: the agent's bar, your glyph, the wash past the block's right edge
      const ms = await marks(page);
      const frame = await frameBox(page);
      const s = await shot(page, frame);
      const bg = hex(want.bg);
      expect(near(s.at(frame.left + 2, frame.top + 2), bg), "frame background").toBe(true);
      const bar = ms.find((m) => m.mark === "bar" && m.sp === "11:1-11:44")!;
      expect(near(s.at((bar.left + bar.right) / 2, (bar.top + bar.bottom) / 2), hex(want.agent), 8), "bar").toBe(true);
      const glyph = ms.find((m) => m.mark === "glyph" && m.sp === "10:1-10:16")!;
      const glyphInk = ink(pixelsOf(s, glyph), bg);
      expect(glyphInk && distance(glyphInk, hex(want.you)) <= 16, `glyph ink ${glyphInk} vs ${want.you}`).toBe(true);
      const wash = ms.find((m) => m.mark === "wash")!;
      expect(near(s.at(wash.right - 3, (wash.top + wash.bottom) / 2), over(want.new, bg), 8), "wash").toBe(true);

      // Chrome: the phrase, the turn button's frame, the link dot
      const chrome = await page.evaluate(() => {
        const phrase = document.querySelector(".file-header birta-phrase")!.shadowRoot!.querySelector(".phrase")!;
        const btn = document.querySelector(".file-header birta-turn")!.shadowRoot!.querySelector("button")!;
        const dot = getComputedStyle(document.querySelector("#content a[data-br-news]")!, "::after");
        return { phrase: getComputedStyle(phrase).color, border: getComputedStyle(btn).borderTopColor, dot: dot.backgroundColor };
      });
      expect(sameColour(chrome.phrase, want.agent), `phrase ${chrome.phrase}`).toBe(true);
      expect(sameColour(chrome.border, want.border), `button border ${chrome.border}`).toBe(true);
      expect(sameColour(chrome.dot, want.agent), `link dot ${chrome.dot}`).toBe(true);

      // Every role stands apart from the page and from each other
      expect(distance(hex(want.agent), bg)).toBeGreaterThanOrEqual(32);
      expect(distance(hex(want.you), bg)).toBeGreaterThanOrEqual(32);
      expect(distance(hex(want.you), hex(want.agent))).toBeGreaterThanOrEqual(32);
      expect(distance(over(want.new, bg), bg)).toBeGreaterThanOrEqual(16);

      // The fresh bar, a sent no and a card's words read in every theme
      const fresh = ms.find((m) => m.mark === "bar" && m.sp === "5:1-6:68")!;
      const freshPx = s.at((fresh.left + fresh.right) / 2, (fresh.top + fresh.bottom) / 2);
      expect(contrast(freshPx, bg), `fresh bar ${freshPx}`).toBeGreaterThanOrEqual(3);
      const painted = await page.evaluate(() => {
        const g = document.querySelector("#lane-frame > birta-over")!.shadowRoot!.querySelector('[data-mark="glyph"][data-verdict="no"]')!;
        const host = Array.from(document.querySelectorAll("#content birta-flow")).find((h) => h.shadowRoot!.querySelector(".lead"))!;
        const root = host.shadowRoot!;
        const cs = (el: Element) => getComputedStyle(el);
        return {
          no: cs(g).backgroundColor,
          lead: cs(root.querySelector(".lead")!).color,
          text: cs(root.querySelector(".text")!).color,
          card: cs(root.querySelector(".flow")!).backgroundColor,
        };
      });
      const rgb = (css: string) => rgba(css).slice(0, 3) as Rgb;
      expect(rgba(painted.no)[3], `no fill ${painted.no}`).toBe(1);
      expect(contrast(rgb(painted.no), bg), `no fill ${painted.no}`).toBeGreaterThanOrEqual(3);
      expect(rgba(painted.card)[3], `card ${painted.card}`).toBe(1);
      // The lead is the theme's own muted ink, which some themes keep under 4.5:1 even on the page; a card costs it little
      const onPage = contrast(rgb(painted.lead), bg);
      expect(contrast(rgb(painted.lead), rgb(painted.card)), `lead ${painted.lead} on ${painted.card}, ${onPage} on the page`).toBeGreaterThanOrEqual(
        Math.min(4.5, onPage * 0.85),
      );
      expect(contrast(rgb(painted.text), rgb(painted.card)), `text ${painted.text} on ${painted.card}`).toBeGreaterThanOrEqual(4.5);
    });
  });
}

// K7a
test("the loop runs every bundled theme in every variant it ships, dark ones included", () => {
  expect(listedThemes()).toEqual(BUNDLED);
  expect(BUNDLED).toHaveLength(11);
  expect(new Set(THEMES.map((t) => t.name))).toEqual(new Set(BUNDLED));
  // Counted from the TOML text itself, so a parser that drops a table can't agree with itself
  for (const name of BUNDLED) {
    const text = fs.readFileSync(path.join(REPO, "assets", "themes", `${name}.toml`), "utf-8");
    for (const v of ["light", "dark"] as const)
      expect(THEMES.filter((t) => t.name === name && t.variant === v).length, `${name} ${v}`).toBe(
        (text.match(new RegExp(`^\\[theme\\.${v}\\]$`, "gm")) ?? []).length,
      );
  }
  expect(THEMES.filter((t) => t.variant === "dark")).toHaveLength(11);
  expect(THEMES).toHaveLength(15);
});

// K7b: the variant flag wins over the OS
for (const [name, variant, scheme] of [
  ["github", "light", "dark"],
  ["github", "dark", "light"],
  ["catppuccin", "light", "dark"],
  ["catppuccin", "dark", "light"],
] as const) {
  test(`theme ${name} --${variant} under an OS in ${scheme} mode keeps the ${variant} roles`, async ({ page, birta }) => {
    const want = roles(name, variant, THEMES.find((t) => t.name === name && t.variant === variant)!.p);
    await page.emulateMedia({ colorScheme: scheme });
    const state = laneState(THEME_DOC, { bars: [{ sp: "5:1-6:68" }], glyphs: [{ sp: "10:1-10:16", verdict: "no" }] });
    await open(page, birta, THEME_DOC, {
      state,
      dir: LINKED_DIR,
      files: { "b.md": "# B\n" },
      args: ["pr.md", "--theme", name, `--${variant}`],
    });
    await expect.poll(() => page.locator("#content birta-flow").count()).toBeGreaterThan(0);
    for (const [role, value] of [
      ["--review-you", want.you],
      ["--review-agent", want.agent],
      ["--review-new", want.new],
    ] as const)
      for (const [site, css] of Object.entries(await roleSites(page, role)))
        expect(sameColour(css, value), `${role} in ${site}: ${css} vs ${value}`).toBe(true);
  });
}

// --- The narrow lane ----------------------------------------------------------------------

test.describe("the narrow lane", () => {
  /** Each lane mark's box; a glyph's knockout ring is the page's own colour, so it is left out. */
  async function painted(page: Page): Promise<(Box & { what: string })[]> {
    return (await marks(page))
      .filter((m) => ["bar", "glyph", "tick", "rule"].includes(m.mark))
      .map((m) => ({ ...m, what: `${m.mark} ${m.sp ?? m.after ?? m.style ?? ""}` }));
  }

  /** Clear of the frame's edge, and a gap from the doc's text that reads as one. */
  const EDGE = 2;
  const TEXT_GAP = 4;

  test("at 520px every mark keeps clear of the frame's edge and of the doc's text", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen"), viewport: NARROW });
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(2);
    const frame = await frameBox(page);
    const content = await contentBox(page);
    const out: string[] = [];
    for (const m of await painted(page)) {
      if (m.left < frame.left + EDGE) out.push(`${m.what}: left ${m.left}, frame edge ${frame.left}`);
      if (m.right > content.left - TEXT_GAP) out.push(`${m.what}: right ${m.right}, text at ${content.left}`);
    }
    expect(out).toEqual([]);
  });

  test("at 520px no mark's ink lands by the frame's edge or by the doc's text", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen"), viewport: NARROW });
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(2);
    await page.waitForTimeout(SETTLE_MS);
    const frame = await frameBox(page);
    const content = await contentBox(page);
    const lane = { left: frame.left, top: frame.top, right: content.left, bottom: Math.min(frame.bottom, NARROW.height) };
    const s = await shot(page, { ...lane, width: lane.right - lane.left, height: lane.bottom - lane.top });
    const bg: Rgb = [255, 255, 255];
    // The new-text wash reaches into the lane by design; it is a background, not a mark
    const wash = over("#fff8c5", bg);
    const columns = new Set<number>();
    for (let y = Math.ceil(lane.top); y < Math.floor(lane.bottom); y++)
      for (let x = Math.ceil(lane.left); x < Math.floor(lane.right); x++) {
        const px = s.at(x, y);
        if (distance(px, bg) > 24 && distance(px, wash) > 24) columns.add(x);
      }
    const xs = [...columns].sort((a, b) => a - b);
    expect(xs.length).toBeGreaterThan(0);
    expect(xs[0], "ink by the frame's edge").toBeGreaterThanOrEqual(frame.left + EDGE);
    expect(xs[xs.length - 1], "ink by the doc's text").toBeLessThan(content.left - TEXT_GAP);
  });

  for (const viewport of [NARROW, WIDE]) {
    test(`at ${viewport.width}px a deletion tick crosses no rule, bar or glyph`, async ({ page, birta }) => {
      await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen"), viewport });
      await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "tick").length).toBe(1);
      const all = await painted(page);
      const ticks = all.filter((m) => m.what.startsWith("tick"));
      const others = all.filter((m) => !m.what.startsWith("tick"));
      const hits = ticks.flatMap((t) =>
        others
          .filter((o) => t.left < o.right && o.left < t.right && t.top < o.bottom && o.top < t.bottom)
          .map((o) => `${t.what} crosses ${o.what}`),
      );
      expect(hits).toEqual([]);
    });
  }
});

// --- The header at a narrow width ---------------------------------------------------------

test.describe("the header at a narrow width", () => {
  test("at 520px the phrase gives way first and the file name stays whole", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen"), viewport: NARROW });
    await expect.poll(() => turnOf(page)).not.toBeNull();
    const h = await page.evaluate(() => {
      const box = (el: Element) => el.getBoundingClientRect();
      const name = document.querySelector(".file-header-name")!;
      const phraseHost = document.querySelector(".file-header birta-phrase")!;
      const phrase = phraseHost.shadowRoot!.querySelector(".phrase")!;
      const turn = document.querySelector(".file-header birta-turn")!;
      return {
        name: { text: name.textContent, scroll: name.scrollWidth, client: name.clientWidth, right: box(name).right },
        phrase: {
          scroll: phrase.scrollWidth,
          client: phrase.clientWidth,
          left: box(phraseHost).left,
          right: box(phraseHost).right,
          title: phraseHost.getAttribute("title"),
          ellipsis: getComputedStyle(phrase).textOverflow,
        },
        turn: { left: box(turn).left, width: box(turn).width },
      };
    });
    expect(h.name.text).toBe("pr.md");
    expect(h.name.scroll, "the file name is cut").toBeLessThanOrEqual(h.name.client);
    expect(h.phrase.scroll, "the phrase is not cut").toBeGreaterThan(h.phrase.client);
    expect(h.phrase.ellipsis).toBe("ellipsis");
    expect(h.phrase.title).toBe("Claude replied 14:13 · 4 unseen · 1 of 3 blocks decided · 1 done · 1 answered · 2 without a note · 1 not handled");
    expect(h.phrase.left).toBeGreaterThanOrEqual(h.name.right);
    expect(h.phrase.right).toBeLessThanOrEqual(h.turn.left);
    expect(h.turn.width).toBeGreaterThanOrEqual(112);
  });

  test("at 520px a long file name still shows its whole name before any of the phrase", async ({ page, birta }) => {
    const doc = fixtureDoc("unseen");
    const served = await birta({ "retry-plan.md": doc }, GATE_ON);
    await page.setViewportSize(NARROW);
    await mockReview(page, { state: fixtureState("unseen") });
    await page.goto(`${served.url}/view/retry-plan.md`);
    await booted(page);
    await expect.poll(() => turnOf(page)).not.toBeNull();
    const name = await page.evaluate(() => {
      const el = document.querySelector(".file-header-name")!;
      return { scroll: el.scrollWidth, client: el.clientWidth };
    });
    expect(name.scroll).toBeLessThanOrEqual(name.client);
  });

  test("at 520px a name too long for the header is cut only once the phrase has given all its room", async ({
    page,
    birta,
  }) => {
    const name = "a-very-long-file-name-for-the-retry-and-backoff-plan-review.md";
    const served = await birta({ [name]: fixtureDoc("unseen") }, GATE_ON);
    await page.setViewportSize(NARROW);
    await mockReview(page, { state: fixtureState("unseen") });
    await page.goto(`${served.url}/view/${name}`);
    await booted(page);
    await expect.poll(() => turnOf(page)).not.toBeNull();
    const h = await page.evaluate(() => {
      const el = document.querySelector(".file-header-name")!;
      const header = document.querySelector(".file-header")!.getBoundingClientRect();
      const phrase = document.querySelector(".file-header birta-phrase")!.getBoundingClientRect();
      const turn = document.querySelector(".file-header birta-turn")!.getBoundingClientRect();
      return { cut: el.scrollWidth > el.clientWidth, phrase: phrase.width, turn: { left: turn.left, right: turn.right, width: turn.width }, header: header.right };
    });
    expect(h.turn.width).toBeGreaterThanOrEqual(112);
    expect(h.turn.right).toBeLessThanOrEqual(h.header);
    if (h.cut) expect(h.phrase, "the name is cut while the phrase keeps room").toBeLessThan(2);
    // Nor may a whole name run under the phrase or the button
    const edges = await page.evaluate(() => ({
      name: document.querySelector(".file-header-name")!.getBoundingClientRect().right,
      phrase: document.querySelector(".file-header birta-phrase")!.getBoundingClientRect().left,
      turn: document.querySelector(".file-header birta-turn")!.getBoundingClientRect().left,
    }));
    expect(edges.name).toBeLessThanOrEqual(edges.phrase + 1);
    expect(edges.name).toBeLessThanOrEqual(edges.turn);
  });
});


// --- Kept folder notes on the page --------------------------------------------------------

/** A folder summary as `review::summarize` gives it, from a committed state. */
function summary(name: string, edit?: (s: State) => void): State {
  const s = fixtureState(name);
  edit?.(s);
  return { stage: s.phrase.stage, phrase: s.phrase, counts: s.counts };
}

/**
 * A folder listing whose folder has `notes` live notes, `drafts` of them unsent. `notes` is
 * the summary's count of every live note: drafts, sent and handled, not retracted.
 */
function folderListing(notes: number, drafts = notes): State {
  return {
    type: "dir_state",
    root: "",
    truncated: false,
    folder: { summary: { ...summary("drafts", (s) => Object.assign(s.counts, { drafts, decided: 0 })), notes } },
    files: [{ paths: ["b.md"], summary: null, news: false }],
    totals: { drafts, unseen_files: 0, decided: 0, items: 0 },
  };
}

/** The doc-slot hosts' texts, above the doc's first block. */
async function docSlot(page: Page): Promise<string[]> {
  return (await flowHosts(page)).filter((h) => h.shown && h.doc).map((h) => h.text);
}

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

const FOLDER_FILES = { "b.md": "# B\n" };

test.describe("kept folder notes", () => {
  for (const [notes, drafts, row] of [
    [2, 2, "2 notes on this folder ▸"],
    [1, 1, "1 note on this folder ▸"],
    [3, 0, "3 notes on this folder ▸"],
    [3, 1, "3 notes on this folder ▸"],
  ] as const) {
    test(`a folder with ${notes} live notes, ${drafts} unsent, shows "${row}" in the doc slot`, async ({ page, birta }) => {
      const doc = "# PR\n\nText.\n";
      await open(page, birta, doc, {
        state: withRev(fixtureState("fresh"), revOf(doc)),
        dir: folderListing(notes, drafts),
        files: FOLDER_FILES,
        args: ["."],
      });
      await expect.poll(() => docSlot(page)).toContain(row);
    });
  }

  test("a folder with no notes of its own shows no folder row", async ({ page, birta }) => {
    const doc = "# PR\n\nText.\n";
    const listing = folderListing(0);
    listing.folder = null;
    await open(page, birta, doc, { state: withRev(fixtureState("fresh"), revOf(doc)), dir: listing, files: FOLDER_FILES, args: ["."] });
    await page.waitForTimeout(SETTLE_MS);
    expect((await docSlot(page)).filter((t) => t.includes("on this folder"))).toEqual([]);
  });

  test("serving one file there is no folder row, whatever the listing says", async ({ page, birta }) => {
    const doc = "# PR\n\nText.\n";
    await open(page, birta, doc, { state: withRev(fixtureState("fresh"), revOf(doc)), dir: folderListing(2), files: FOLDER_FILES });
    await page.waitForTimeout(SETTLE_MS);
    expect((await docSlot(page)).filter((t) => t.includes("on this folder"))).toEqual([]);
  });

  test("▸ on the folder row opens the menu on its folder row", async ({ page, birta }) => {
    const doc = "# PR\n\nText.\n";
    await open(page, birta, doc, {
      state: withRev(fixtureState("fresh"), revOf(doc)),
      dir: folderListing(2),
      files: FOLDER_FILES,
      args: ["."],
    });
    await expect.poll(() => docSlot(page)).toContain("2 notes on this folder ▸");
    // The menu's rows come from the doc's state, so wait until the chrome shows it
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.locator("birta-flow", { hasText: "on this folder" }).getByText("▸").click();
    await expect.poll(() => menuRows(page)).not.toBeNull();
    const current = (await menuRows(page))!.filter((r) => r.current).map((r) => r.text);
    expect(current).toHaveLength(1);
    expect(current[0].startsWith("note on this folder")).toBe(true);
  });

  // K9a
  test("the row counts the folder's own live notes, not the drafts across the folder", async ({ page, birta }) => {
    const doc = "# PR\n\nText.\n";
    const listing = folderListing(2, 0);
    listing.totals.drafts = 5;
    await open(page, birta, doc, { state: withRev(fixtureState("fresh"), revOf(doc)), dir: listing, files: FOLDER_FILES, args: ["."] });
    await expect.poll(() => docSlot(page)).toContain("2 notes on this folder ▸");
  });

  test("a folder whose notes were all taken back shows no row, whatever its drafts say", async ({ page, birta }) => {
    const doc = "# PR\n\nText.\n";
    await open(page, birta, doc, {
      state: withRev(fixtureState("fresh"), revOf(doc)),
      dir: folderListing(0, 2),
      files: FOLDER_FILES,
      args: ["."],
    });
    await page.waitForTimeout(SETTLE_MS);
    expect((await docSlot(page)).filter((t) => t.includes("on this folder"))).toEqual([]);
  });

  // K9b
  test("on a real server the folder row stays after a hand-back, a reload, and on the folder's other page", async ({
    page,
    birta,
  }) => {
    const served = await birta(
      { "pr.md": "# PR\n\nText.\n", "b.md": "# B\n" },
      { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "300" },
      ["."],
    );
    const folderLog = () => {
      const f = path.join(served.root, ".birta.jsonl");
      return fs.existsSync(f) ? fs.readFileSync(f, "utf-8").trim().split("\n").map((l) => JSON.parse(l)) : [];
    };
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    // A doc with no review yet shows the phrase from the first review key
    await page.keyboard.press("j");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.locator(".file-header birta-phrase").click();
    await page.locator("birta-pop [role=menuitem]", { hasText: "note on this folder" }).click();
    await page.locator('birta-flow[data-kind="composer"] textarea').fill("FOLDER-NOTE");
    await page.locator('birta-flow[data-kind="composer"] textarea').press("Enter");
    await expect.poll(() => docSlot(page), { timeout: 5_000 }).toContain("1 note on this folder ▸");
    await page.keyboard.press("Escape");
    // The folder's draft makes this a hand-back, never an approve
    await expect.poll(() => turnOf(page), { timeout: 5_000 }).toMatch(/^Hand back/);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => folderLog().some((l) => l.op === "submit")).toBe(true);
    // Past the 300 ms hold, so the note is sent; this page's own phrase has nothing to send
    await page.waitForTimeout(600);
    expect(await docSlot(page)).toContain("1 note on this folder ▸");
    await page.reload();
    await booted(page);
    await expect.poll(() => docSlot(page), { timeout: 5_000 }).toContain("1 note on this folder ▸");
    await page.goto(`${served.url}/view/b.md`);
    await booted(page);
    await expect.poll(() => docSlot(page), { timeout: 5_000 }).toContain("1 note on this folder ▸");
  });
});

// --- IME composition (dispatched; Safari's real order is in the manual pass) ---------------

interface Ime {
  key?: string;
  code?: string;
  isComposing?: boolean;
  keyCode?: number;
  metaKey?: boolean;
  ctrlKey?: boolean;
}

/**
 * Fires composition and key events at the textarea inside the flow host `sel`'s root, which
 * the harness's `dispatchKey` can't reach. Returns whether the keydown was default-prevented.
 */
function imeEvent(page: Page, sel: string, kind: "compositionstart" | "compositionend" | "keydown", init: Ime = {}) {
  return page.evaluate(
    ({ sel, kind, init }) => {
      const area = Array.from(document.querySelectorAll(sel))
        .map((h) => h.shadowRoot?.querySelector("textarea"))
        .find((a) => a);
      if (!area) throw new Error(`no textarea in ${sel}`);
      if (kind !== "keydown") {
        area.dispatchEvent(new CompositionEvent(kind, { bubbles: true, composed: true, data: kind === "compositionend" ? "日本" : "" }));
        return false;
      }
      const { keyCode, ...rest } = init;
      const e = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, composed: true, ...rest });
      if (keyCode !== undefined) Object.defineProperty(e, "keyCode", { value: keyCode });
      area.dispatchEvent(e);
      return e.defaultPrevented;
    },
    { sel, kind, init },
  );
}

const COMPOSER = 'birta-flow[data-kind="composer"]';
const composing = (key: string, more: Ime = {}): Ime => ({ key, code: key === "Enter" ? "Enter" : `Key${key.toUpperCase()}`, isComposing: true, keyCode: 229, ...more });

test.describe("IME composition", () => {
  test("in the composer, ⏎ and Esc mid-word belong to the IME, then ⏎ keeps the note", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await page.keyboard.press("c");
    await expect(page.locator(`${COMPOSER} textarea`)).toBeFocused();
    await imeEvent(page, COMPOSER, "compositionstart");
    await page.keyboard.insertText("にほん");
    expect(await imeEvent(page, COMPOSER, "keydown", composing("Enter"))).toBe(false);
    expect(await imeEvent(page, COMPOSER, "keydown", composing("Escape", { code: "Escape" }))).toBe(false);
    for (const k of ["y", "x", "l", "q", "c"]) await imeEvent(page, COMPOSER, "keydown", composing(k));
    // Safari ends the composition before the ⏎ that commits it, which still reads 229
    await imeEvent(page, COMPOSER, "compositionend");
    expect(await imeEvent(page, COMPOSER, "keydown", { key: "Enter", code: "Enter", isComposing: false, keyCode: 229 })).toBe(false);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await expect(page.locator(`${COMPOSER} textarea`)).toBeFocused();
    await page.locator(`${COMPOSER} textarea`).fill("日本語");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().map((o) => [o.op, o.body])).toEqual([["comment", "日本語"]]);
  });

  test("⌘⏎ mid-word in the composer hands nothing back and keeps nothing", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await page.keyboard.press("c");
    await expect(page.locator(`${COMPOSER} textarea`)).toBeFocused();
    await imeEvent(page, COMPOSER, "compositionstart");
    await page.keyboard.insertText("にほん");
    const mac = await page.evaluate(() => /Mac/.test(navigator.platform));
    await imeEvent(page, COMPOSER, "keydown", composing("Enter", mac ? { metaKey: true } : { ctrlKey: true }));
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await expect(page.locator(`${COMPOSER} textarea`)).toBeFocused();
  });

  // K1: the same dispatched keys act when nothing is composing, or the checks above prove nothing
  test("a dispatched ⏎ that isn't composing keeps the note", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await page.keyboard.press("c");
    await expect(page.locator(`${COMPOSER} textarea`)).toBeFocused();
    await page.keyboard.insertText("日本語");
    expect(await imeEvent(page, COMPOSER, "keydown", { key: "Enter", code: "Enter" })).toBe(true);
    await expect.poll(() => mock.ops().map((o) => [o.op, o.body])).toEqual([["comment", "日本語"]]);
  });

  test("a dispatched ⌘⏎ that isn't composing acts", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await page.keyboard.press("c");
    await expect(page.locator(`${COMPOSER} textarea`)).toBeFocused();
    await page.keyboard.insertText("にほん");
    const mac = await page.evaluate(() => /Mac/.test(navigator.platform));
    await imeEvent(page, COMPOSER, "keydown", { key: "Enter", code: "Enter", ...(mac ? { metaKey: true } : { ctrlKey: true }) });
    await expect.poll(() => mock.ops().length).toBeGreaterThan(0);
  });

  test("in a blank's written answer, a dispatched ⏎ that isn't composing sends it", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("flow"), { state: fixtureState("flow") });
    const blank = 'birta-flow[data-kind="blank"]';
    await page.locator(blank, { hasText: "is 503 the only code?" }).click();
    await page.keyboard.press("a");
    await page.keyboard.insertText("にほん");
    await imeEvent(page, blank, "keydown", { key: "Enter", code: "Enter" });
    await expect.poll(() => mock.ops().map((o) => [o.op, o.body])).toEqual([["reply", "aにほん"]]);
  });

  test("in a blank's written answer, ⏎ mid-word belongs to the IME", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("flow"), { state: fixtureState("flow") });
    const blank = 'birta-flow[data-kind="blank"]';
    await page.locator(blank, { hasText: "is 503 the only code?" }).click();
    await page.keyboard.press("a");
    await expect(page.locator(`${blank} textarea`)).toBeFocused();
    await imeEvent(page, blank, "compositionstart");
    await page.keyboard.insertText("にほん");
    await imeEvent(page, blank, "keydown", composing("Enter"));
    await imeEvent(page, blank, "compositionend");
    await imeEvent(page, blank, "keydown", { key: "Enter", code: "Enter", isComposing: false, keyCode: 229 });
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().map((o) => [o.op, o.body])).toEqual([["reply", "aにほん"]]);
  });
});

// --- The page's own beacon (K4) -------------------------------------------------------------

test.describe("the page's own beacon", () => {
  /** Proxies the page's socket to the real server, swallowing the page's ops so none is acked. */
  async function dropOps(page: Page) {
    await page.routeWebSocket(/\/ws$/, (ws) => {
      const server = ws.connectToServer();
      ws.onMessage((raw) => {
        if (JSON.parse(raw.toString()).type !== "op") server.send(raw);
      });
    });
  }

  test("a note the socket never acked reaches the review file when the page goes away", async ({ page, birta }) => {
    const served = await birta({ "pr.md": fixtureDoc("unseen") }, GATE_ON);
    await page.setViewportSize(WIDE);
    await dropOps(page);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("c");
    await page.keyboard.type("only by beacon");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)).toBeNull();
    // The page's own pagehide path, on a live page: a real unload can drop the beacon in a
    // headless browser, and a reload's outbox replay would hide a missing one
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    await expect
      .poll(() => (reviewLog(served.root) ?? []).filter((l) => l.op === "comment").map((l) => l.body))
      .toEqual(["only by beacon"]);
  });

  test("a hand-back the socket never acked is not sent by the beacon", async ({ page, birta }) => {
    const served = await birta({ "pr.md": fixtureDoc("unseen") }, GATE_ON);
    await page.setViewportSize(WIDE);
    await dropOps(page);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("c");
    await page.keyboard.type("kept");
    await page.keyboard.press("Enter");
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.waitForTimeout(SETTLE_MS);
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    await expect.poll(() => (reviewLog(served.root) ?? []).some((l) => l.op === "comment")).toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    expect((reviewLog(served.root) ?? []).filter((l) => l.op === "submit" || l.op === "approve")).toEqual([]);
  });
});
