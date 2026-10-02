import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  OPEN_ROOTS,
  STOPS_DOC,
  State,
  booted,
  dispatchKey,
  fixtureDoc,
  fixtureState,
  focused,
  marks,
  mockReview,
  revOf,
  stopSp,
  stops,
  withRev,
} from "./harness";

// The phrase prints local times; UTC lets the spec name them from the epoch alone
test.use({ timezoneId: "UTC" });

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
  links?: Record<string, string>,
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

/** Committed state `name` for `doc`, with nothing placed on its blocks. */
function bare(name: string, doc: string, edit?: (s: State) => void): State {
  const s = withRev(fixtureState(name), revOf(doc));
  Object.assign(s, { items: [], changes: [], deletions: [], lost: [], later: [], rewrite: null });
  edit?.(s);
  return s;
}

/** Presses j from nothing until `sp` has focus. */
async function focusOn(page: Page, sp: string) {
  const all = await stops(page);
  const at = all.findIndex((s) => s.sp === sp);
  if (at < 0) throw new Error(`${sp} is not a stop`);
  while ((await focused(page)) !== null) await page.keyboard.press("Escape");
  // Blanks are stops too, and a whole-doc H1 is not, so walk until the block has focus rather than count
  for (let i = 0; i <= at + 8 && (await focused(page)) !== sp; i++) await page.keyboard.press("j");
  await expect.poll(() => focused(page)).toBe(sp);
}

interface Chrome {
  phrase: string | null;
  turn: string | null;
  pill: string | null;
  menu: string[] | null;
  /** The key sheet's lines, or null while it is closed. */
  sheet: string[] | null;
}

/** What the header and the popovers show, read through the (forced open) shadow roots. */
function chrome(page: Page): Promise<Chrome> {
  return page.evaluate(() => {
    // Each text node is its own word, so keys drawn as chips read apart
    const textOf = (n: Node | null): string => {
      const out: string[] = [];
      const walk = (x: Node) => {
        if (x.nodeType === 3) {
          out.push(x.textContent ?? "");
          return;
        }
        if (x.nodeType === 1 && ["STYLE", "SCRIPT"].includes((x as Element).tagName)) return;
        const root = x.nodeType === 1 ? (x as Element).shadowRoot : null;
        for (const c of Array.from((root ?? x).childNodes)) walk(c);
      };
      if (n) walk(n);
      return out.map((t) => t.trim()).filter(Boolean).join(" ").replace(/\s+/g, " ");
    };
    const shown = (el: Element | null): el is Element =>
      !!el && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
    const open = (el: Element) => {
      try {
        return el.matches(":popover-open") && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
      } catch {
        return false;
      }
    };
    const p = document.querySelector(".file-header birta-phrase");
    const t = document.querySelector(".file-header birta-turn");
    const btn = shown(t) ? (t.shadowRoot ?? t).querySelector("button") : null;
    let pill: string | null = null;
    let menu: string[] | null = null;
    let sheet: string[] | null = null;
    for (const h of Array.from(document.querySelectorAll("birta-pop"))) {
      const root = (h.shadowRoot ?? h) as ParentNode;
      for (const el of Array.from(root.querySelectorAll('[role="status"]'))) if (open(el) && textOf(el)) pill = textOf(el);
      for (const el of Array.from(root.querySelectorAll('[role="menu"]')))
        if (open(el)) menu = Array.from(el.querySelectorAll('[role="menuitem"]')).map((r) => textOf(r));
      for (const el of Array.from(root.querySelectorAll('[role="dialog"]')))
        if (open(el)) sheet = Array.from(el.querySelectorAll(".row")).map((r) => textOf(r));
    }
    return {
      phrase: shown(p) ? textOf(p) : null,
      turn: btn ? textOf(btn) : null,
      pill,
      menu,
      sheet,
    };
  });
}

const pillOf = async (page: Page) => (await chrome(page)).pill;
const sheetOf = async (page: Page) => (await chrome(page)).sheet;
const hash = (page: Page) => page.evaluate(() => location.hash);
const viewing = (page: Page) => decodeURIComponent(new URL(page.url()).pathname);

/** The turn key as the page names it. */
function turnKey(page: Page): Promise<string> {
  return page.evaluate(() => (/Mac/.test(navigator.platform) ? "⌘⏎" : "Ctrl⏎"));
}

/** The text of every in-flow host, so a folded note reads as absent. */
function flowText(page: Page): Promise<string> {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll("birta-flow"))
      .map((h) => (h.shadowRoot ?? h).textContent ?? "")
      .join("\n"),
  );
}

// --- #15 links ---------------------------------------------------------------------------------

// No link target exists, so following one changes the hash and never scrolls the focus away
const LINK_DOC = [
  "# Links",
  "",
  "See [alpha](#to-alpha) and [beta](#to-beta) here.",
  "",
  "Only [gamma](#to-gamma) here.",
  "",
  "Three: [one](#to-one), [two](#to-two) and [three](#to-three).",
  "",
  "No links in this one.",
  "",
  "## Heading with [delta](#to-delta)",
  "",
  "## Plain heading",
  "",
  "Last words.",
  "",
].join("\n");

/**
 * The picked link: the focused element's text, and whether review rings it: the only link with
 * review's pick attribute, outlined at least 2px in review's own colour, not the theme's focus ring.
 */
function picked(page: Page): Promise<{ text: string; ring: boolean } | null> {
  return page.evaluate(() => {
    const a = document.activeElement;
    if (!a || a.localName !== "a") return null;
    const probe = document.createElement("span");
    probe.style.color = "var(--review-you)";
    document.body.appendChild(probe);
    const colour = getComputedStyle(probe).color;
    probe.remove();
    const cs = getComputedStyle(a);
    const marked = Array.from(document.querySelectorAll("#content [data-br-pick]"));
    const ring =
      marked.length === 1 &&
      marked[0] === a &&
      cs.outlineStyle === "solid" &&
      parseFloat(cs.outlineWidth) >= 2 &&
      cs.outlineColor === colour;
    return { text: a.textContent ?? "", ring };
  });
}

/** Whether nothing is picked: no link has focus or review's pick attribute. */
function noPick(page: Page): Promise<boolean> {
  return page.evaluate(
    () => document.activeElement?.localName !== "a" && document.querySelectorAll("#content [data-br-pick]").length === 0,
  );
}

test.describe("links in the focused block", () => {
  test("⏎ opens a block's only link", async ({ page, birta }) => {
    await open(page, birta, LINK_DOC);
    await focusOn(page, await stopSp(page, "Only gamma"));
    await page.keyboard.press("Enter");
    await expect.poll(() => hash(page)).toBe("#to-gamma");
  });

  test("⏎ on a block with two links and none picked pills how to pick one, and opens neither", async ({ page, birta }) => {
    await open(page, birta, LINK_DOC);
    await focusOn(page, await stopSp(page, "See alpha"));
    await page.keyboard.press("Enter");
    await expect.poll(() => pillOf(page)).toBe("2 links · Tab to pick");
    expect(await hash(page)).toBe("");
  });

  test("Tab and Shift+Tab cycle the block's links with a ring, and ⏎ opens the picked one", async ({ page, birta }) => {
    await open(page, birta, LINK_DOC);
    const para = await stopSp(page, "See alpha");
    await focusOn(page, para);
    await page.keyboard.press("Tab");
    expect(await picked(page)).toEqual({ text: "alpha", ring: true });
    await page.keyboard.press("Tab");
    expect(await picked(page)).toEqual({ text: "beta", ring: true });
    // Round the end, within the block
    await page.keyboard.press("Tab");
    expect(await picked(page)).toEqual({ text: "alpha", ring: true });
    await page.keyboard.press("Shift+Tab");
    expect(await picked(page)).toEqual({ text: "beta", ring: true });
    expect(await focused(page)).toBe(para);
    await page.keyboard.press("Enter");
    await expect.poll(() => hash(page)).toBe("#to-beta");
  });

  test("Shift+Tab from no pick starts at the last link", async ({ page, birta }) => {
    await open(page, birta, LINK_DOC);
    await focusOn(page, await stopSp(page, "See alpha"));
    await page.keyboard.press("Shift+Tab");
    expect((await picked(page))?.text).toBe("beta");
  });

  test("moving on drops the pick, so ⏎ acts on the new block and never the old link", async ({ page, birta }) => {
    await open(page, birta, LINK_DOC);
    await focusOn(page, await stopSp(page, "See alpha"));
    await page.keyboard.press("Tab");
    expect((await picked(page))?.text).toBe("alpha");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(await stopSp(page, "Only gamma"));
    expect(await noPick(page)).toBe(true);
    await page.keyboard.press("j");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(await hash(page)).toBe("");
  });

  test("on three links Tab and Shift+Tab step each way and wrap, and ⏎ with none picked counts them", async ({
    page,
    birta,
  }) => {
    await open(page, birta, LINK_DOC);
    const three = await stopSp(page, "Three:");
    await focusOn(page, three);
    await page.keyboard.press("Enter");
    await expect.poll(() => pillOf(page)).toBe("3 links · Tab to pick");
    for (const key of ["Tab", "Tab", "Shift+Tab"]) await page.keyboard.press(key);
    expect(await picked(page)).toEqual({ text: "one", ring: true });
    for (const key of ["Tab", "Tab", "Tab"]) await page.keyboard.press(key);
    expect((await picked(page))?.text).toBe("one");
    await page.keyboard.press("Escape");
    await focusOn(page, three);
    await page.keyboard.press("Shift+Tab");
    expect((await picked(page))?.text).toBe("three");
    await page.keyboard.press("Shift+Tab");
    expect((await picked(page))?.text).toBe("two");
  });

  for (const key of ["j", "k", "n", "p", "Escape"]) {
    test(`${key} drops the pick, and ⏎ after it follows no link`, async ({ page, birta }) => {
      await open(page, birta, LINK_DOC);
      await focusOn(page, await stopSp(page, "See alpha"));
      await page.keyboard.press("Tab");
      expect((await picked(page))?.text).toBe("alpha");
      await page.keyboard.press(key);
      expect(await noPick(page)).toBe(true);
      await page.keyboard.press("Enter");
      await page.waitForTimeout(SETTLE_MS);
      expect(await hash(page)).not.toBe("#to-alpha");
    });
  }

  test("scrolling the focus away or clicking elsewhere drops the pick", async ({ page, birta }) => {
    const filler = Array.from({ length: 60 }, (_, i) => `Filler paragraph ${i + 1}.\n`).join("\n");
    await open(page, birta, `${LINK_DOC}\n${filler}`);
    await focusOn(page, await stopSp(page, "Only gamma"));
    await page.keyboard.press("Tab");
    expect((await picked(page))?.text).toBe("gamma");
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await expect.poll(() => focused(page)).toBeNull();
    expect(await noPick(page)).toBe(true);

    await page.evaluate(() => window.scrollTo(0, 0));
    await focusOn(page, await stopSp(page, "Only gamma"));
    await page.keyboard.press("Tab");
    expect((await picked(page))?.text).toBe("gamma");
    await page.locator("#content p", { hasText: "No links in this one" }).click();
    await expect.poll(() => noPick(page)).toBe(true);
  });

  test("Tab from a header control stays the browser's, though a block with links has focus", async ({ page, birta }) => {
    await open(page, birta, LINK_DOC, { state: bare("drafts", LINK_DOC) });
    await focusOn(page, await stopSp(page, "Only gamma"));
    await page.locator(".file-header birta-turn button").focus();
    await page.keyboard.press("Tab");
    expect(await noPick(page)).toBe(true);
    await page.locator(".file-header birta-phrase .phrase").focus();
    await page.keyboard.press("Tab");
    expect(await noPick(page)).toBe(true);
  });

  test("Tab is the browser's on a block with no links, and with no block focused", async ({ page, birta }) => {
    await open(page, birta, LINK_DOC);
    expect(await dispatchKey(page, { key: "Tab", code: "Tab" })).toBe(false);
    await focusOn(page, await stopSp(page, "No links"));
    expect(await dispatchKey(page, { key: "Tab", code: "Tab" })).toBe(false);
    await focusOn(page, await stopSp(page, "Only gamma"));
    expect(await dispatchKey(page, { key: "Tab", code: "Tab" })).toBe(true);
  });

  test("a heading's own anchor icon is not one of its links", async ({ page, birta }) => {
    await open(page, birta, LINK_DOC);
    await focusOn(page, await stopSp(page, "Plain heading"));
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(await hash(page)).toBe("");
    expect(await pillOf(page)).toBeNull();
    expect(await dispatchKey(page, { key: "Tab", code: "Tab" })).toBe(false);
    await focusOn(page, await stopSp(page, "Heading with delta"));
    await page.keyboard.press("Enter");
    await expect.poll(() => hash(page)).toBe("#to-delta");
  });

  test("notes are out of the Tab order", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("flow"), { state: fixtureState("flow") });
    await expect(page.locator('birta-flow[data-kind="note"]').first()).toBeAttached();
    const inOrder = await page.evaluate(() =>
      Array.from(document.querySelectorAll("birta-flow"))
        .filter((h) => !["blank", "composer"].includes(h.getAttribute("data-kind") ?? ""))
        .flatMap((h) => [h, ...Array.from((h.shadowRoot ?? h).querySelectorAll("*"))])
        .filter((el) => (el as HTMLElement).tabIndex >= 0)
        .map((el) => `${el.localName}.${el.className}`),
    );
    expect(inOrder).toEqual([]);
  });
});

// --- ⏎ order: a dot comes before the block's only link, a picked link before the dot ----------------

// The flow fixture with a link in the item note 6 sits on; same length, so every sp holds
const DOT_DOC = fixtureDoc("flow").replace(
  "- backoff of 1, 2 and 4 seconds, capped at 8",
  "- backoff of [1, 2 and 4](#b) seconds, cap 8",
);
const DOT_SP = "11:1-11:44";

/** The flow state on DOT_DOC, handed back since, with note 6 seen: it folds to a dot. */
function dotState(doc = DOT_DOC): State {
  const s = withRev(fixtureState("flow"), revOf(doc));
  s.items.find((i: State) => i.n === 6).seen = true;
  const round = { ...JSON.parse(JSON.stringify(s.rounds.at(-1))), submit: 26, ts: s.server_now - 60_000, items: [] };
  for (const k of ["claim", "claimed_by", "yield"]) delete round[k];
  s.rounds.push(round);
  return s;
}

/** DOT_DOC with two links on that line, still the same length. */
const DOT2_DOC = fixtureDoc("flow").replace(
  "- backoff of 1, 2 and 4 seconds, capped at 8",
  "- [backoff](#b) of [1, 2](#c) and 4 s, cap 8",
);

const dotCount = async (page: Page) => (await marks(page)).filter((m) => m.mark === "dot").length;

test.describe("⏎ order on a focused block", () => {
  test("a dot opens before the block's only link", async ({ page, birta }) => {
    expect(DOT_DOC).not.toBe(fixtureDoc("flow"));
    await open(page, birta, DOT_DOC, { state: dotState() });
    await expect.poll(() => dotCount(page)).toBe(1);
    expect(await flowText(page)).not.toContain("cap it");
    await focusOn(page, DOT_SP);
    await page.keyboard.press("Enter");
    await expect.poll(() => flowText(page)).toContain("cap it");
    expect(await hash(page)).toBe("");
  });

  test("a dot opens before the pill for a block's two links", async ({ page, birta }) => {
    expect(DOT2_DOC).not.toBe(fixtureDoc("flow"));
    // The first dot ever says what it is once; that pill is not the one this checks for
    await page.addInitScript(() => localStorage.setItem("birta-dot-hint", "1"));
    await open(page, birta, DOT2_DOC, { state: dotState(DOT2_DOC) });
    await expect.poll(() => dotCount(page)).toBe(1);
    await focusOn(page, DOT_SP);
    await page.keyboard.press("Enter");
    await expect.poll(() => flowText(page)).toContain("cap it");
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).toBeNull();
    expect(await hash(page)).toBe("");
  });

  test("a picked link opens before the dot", async ({ page, birta }) => {
    await open(page, birta, DOT_DOC, { state: dotState() });
    await expect.poll(() => dotCount(page)).toBe(1);
    await focusOn(page, DOT_SP);
    await page.keyboard.press("Tab");
    expect((await picked(page))?.text).toBe("1, 2 and 4");
    await page.keyboard.press("Enter");
    await expect.poll(() => hash(page)).toBe("#b");
    expect(await flowText(page)).not.toContain("cap it");
  });
});

// --- The folder: Shift+J/K, link dots, no drafter ----------------------------------------------------

/** A doc's `dir_state` summary as `review::summarize` builds it, with its live-note count. */
function summary(name: string, edit?: (s: State) => void): State {
  const s = fixtureState(name);
  edit?.(s);
  const notes = s.items.filter((i: State) => i.by.type === "human" && !["retracted", "withdrawn"].includes(i.status)).length;
  return { stage: s.phrase.stage, phrase: s.phrase, counts: s.counts, notes };
}

function listed(paths: string | string[], sum: State = null, news = false): State {
  return { paths: ([] as string[]).concat(paths), summary: sum, news };
}

function dirState(files: State[], totals: State = {}): State {
  return {
    type: "dir_state",
    root: "",
    truncated: false,
    folder: null,
    files,
    totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0, ...totals },
  };
}

const INDEX_DOC = [
  "# Index",
  "",
  "See [news](news.md), [drafts](drafts.md), [sent](sent.md), [approved](approved.md), [quiet](quiet.md), [fresh](fresh.md) and [alias](alias.md).",
  "",
].join("\n");

const FILES: Record<string, string> = {
  "pr.md": INDEX_DOC,
  "news.md": "# News\n",
  "drafts.md": "# Drafts\n",
  "sent.md": "# Sent\n",
  "approved.md": "# Approved\n",
  "quiet.md": "# Quiet\n",
  "fresh.md": "# Fresh\n",
};
/** alias.md is a symlink to sent.md, so the listing names sent.md by both paths. */
const LINKS = { "alias.md": "sent.md" };

/** Every kind of file: news alone, drafts, sent notes, approved notes, and nothing. */
const BUSY = () =>
  dirState(
    [
      listed("news.md", null, true),
      listed("drafts.md", summary("drafts")),
      listed("pr.md", summary("drafts")),
      listed("fresh.md", summary("fresh")),
      listed(["sent.md", "alias.md"], summary("sent")),
      listed("approved.md", summary("approved")),
      listed("quiet.md"),
    ],
    { drafts: 6 },
  );

/** Serves FILES as a folder with review on and the socket mocked; the page is pr.md. */
async function openFolder(page: Page, birta: Start, dir: State, state: State = bare("drafts", INDEX_DOC)) {
  const served = await birta(FILES, GATE_ON, ["."], LINKS);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state, dir });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  await expect.poll(async () => (await chrome(page)).phrase).not.toBeNull();
  return { served, mock };
}

test.describe("Shift+J and Shift+K", () => {
  test("go to the next and previous file with news, drafts or open notes, past a fresh one", async ({
    page,
    birta,
  }) => {
    const { served } = await openFolder(page, birta, BUSY());
    await page.keyboard.press("Shift+J");
    await page.waitForURL(`${served.url}/view/sent.md`);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(async () => (await chrome(page)).phrase).not.toBeNull();
    await page.keyboard.press("Shift+K");
    await page.waitForURL(`${served.url}/view/drafts.md`);
  });

  test("with nowhere to go they pill why and stay", async ({ page, birta }) => {
    const dir = dirState([listed("pr.md", summary("drafts"), true), listed("approved.md", summary("approved")), listed("quiet.md")]);
    const { mock } = await openFolder(page, birta, dir);
    await page.keyboard.press("Shift+J");
    await expect.poll(() => pillOf(page)).toBe("no other file has notes or news");
    await page.keyboard.press("Shift+K");
    await page.waitForTimeout(SETTLE_MS);
    expect(viewing(page)).toBe("/view/pr.md");
    expect(await pillOf(page)).toBe("no other file has notes or news");
    expect(mock.ops()).toEqual([]);
  });

  test("serving one file they pill the same", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    await page.keyboard.press("Shift+K");
    await expect.poll(() => pillOf(page)).toBe("no other file has notes or news");
  });
});

/** `href`s of the doc's links that carry a dot for `kind`. */
function dotted(page: Page, kind: "news" | "notes"): Promise<string[]> {
  return page.evaluate(
    (attr) => Array.from(document.querySelectorAll(`#content a[${attr}]`)).map((a) => a.getAttribute("href")!),
    `data-br-${kind}`,
  );
}

function afterOf(page: Page, href: string) {
  return page.evaluate((href) => {
    const cs = getComputedStyle(document.querySelector(`#content a[href="${href}"]`)!, "::after");
    return { content: cs.content, width: cs.width, height: cs.height, radius: cs.borderRadius, display: cs.display, bg: cs.backgroundColor };
  }, href);
}

test.describe("link dots for notes", () => {
  test("a link to a file with drafts or open notes carries the news dot's dot; approved and quiet ones don't", async ({
    page,
    birta,
  }) => {
    await openFolder(page, birta, BUSY());
    await expect.poll(() => dotted(page, "notes")).toEqual(["/view/drafts.md", "/view/sent.md", "/view/alias.md"]);
    expect(await dotted(page, "news")).toEqual(["/view/news.md"]);
    const news = await afterOf(page, "/view/news.md");
    expect(news.display).toBe("inline-block");
    expect(await afterOf(page, "/view/drafts.md")).toEqual(news);
    for (const href of ["/view/approved.md", "/view/quiet.md", "/view/fresh.md"])
      expect((await afterOf(page, href)).display, href).not.toBe("inline-block");
  });

  test("m hides the notes dot with the news dot, and a listing without the notes takes it away", async ({ page, birta }) => {
    const { mock } = await openFolder(page, birta, BUSY());
    await expect.poll(() => dotted(page, "notes")).toEqual(["/view/drafts.md", "/view/sent.md", "/view/alias.md"]);
    await page.keyboard.press("m");
    await expect.poll(async () => (await afterOf(page, "/view/drafts.md")).display).toBe("none");
    await page.keyboard.press("m");
    await expect.poll(async () => (await afterOf(page, "/view/drafts.md")).display).toBe("inline-block");
    mock.push(dirState([listed("pr.md", summary("drafts")), listed("sent.md", summary("sent"))]));
    await expect.poll(() => dotted(page, "notes")).toEqual(["/view/sent.md"]);
  });

  test("notes dots come back after the doc is patched", async ({ page, birta }) => {
    const { served } = await openFolder(page, birta, BUSY());
    await expect.poll(async () => (await dotted(page, "notes")).length).toBe(3);
    served.write("pr.md", INDEX_DOC.replace("See [news]", "Now see [news]"));
    await expect(page.locator("#content")).toContainText("Now see news", { timeout: 10_000 });
    await expect.poll(async () => (await dotted(page, "notes")).length).toBe(3);
  });

  test("notes dots come back after a theme change re-renders the doc", async ({ page, birta }) => {
    const { served, mock } = await openFolder(page, birta, BUSY());
    await expect.poll(async () => (await dotted(page, "notes")).length).toBe(3);
    mock.holdContent = true;
    const edited = INDEX_DOC.replace("See [news]", "Themed: see [news]");
    served.write("pr.md", edited);
    const html = () => fetch(`${served.url}/render/pr.md`).then((r) => r.text());
    await expect.poll(html).toContain("Themed: see");
    const css = await page.evaluate(() => document.getElementById("theme-vars")!.textContent);
    mock.push({
      type: "theme_update",
      path: "pr.md",
      rev: revOf(edited),
      html: await html(),
      theme_name: "github",
      active_variant: "dark",
      css_vars: css,
      variants: ["light", "dark"],
      has_toggle: true,
      theme_attr: null,
    });
    await expect(page.locator("#content")).toContainText("Themed: see news", { timeout: 10_000 });
    await expect.poll(async () => (await dotted(page, "notes")).length).toBe(3);
  });
});

// --- N11: a folder with no drafter --------------------------------------------------------------

test.describe("a folder with no drafter", () => {
  test("shows no Approve button until a note makes it a hand-back", async ({ page, birta }) => {
    const served = await birta({ "pr.md": "# PR\n\nText.\n", "b.md": "# B\n" }, GATE_ON, ["."]);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("j");
    await expect.poll(async () => (await chrome(page)).phrase).not.toBeNull();
    await page.waitForTimeout(SETTLE_MS);
    expect((await chrome(page)).turn).toBeNull();
    await page.keyboard.press("c");
    await page.locator('birta-flow[data-kind="composer"] textarea').fill("a note");
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await chrome(page)).turn).toBe(`Hand back · 1 ${await turnKey(page)}`);
  });

  test("adds the hand-off line to the phrase once a hand-back has sat unclaimed for 10 s", async ({ page, birta }) => {
    const state = bare("sent", INDEX_DOC, (s) => {
      s.owner.drafted_by = null;
      s.phrase.sent.ts = s.server_now - 8_000;
    });
    const at = new Date(state.phrase.sent.ts).toISOString().slice(11, 16);
    await openFolder(page, birta, dirState([listed("pr.md", summary("sent"))]), state);
    await expect.poll(async () => (await chrome(page)).phrase).toBe(`sent ${at} · not picked up yet`);
    await expect
      .poll(async () => (await chrome(page)).phrase, { timeout: 5_000 })
      .toBe(`sent ${at} · not picked up yet · copy hand-off line`);
  });
});

// --- #2 the ? sheet, the menu row, and the unbound-letter pill ---------------------------------------

/** The sheet's lines with the default keys, the turn key as this page names it. */
async function sheetLines(page: Page): Promise<string[]> {
  return [
    "decide y x l .",
    "write c q",
    "move j k n p ⇧J ⇧K Tab",
    "take back z",
    "Claude's changes u revert Claude's change (open row)",
    `hand back / approve ${await turnKey(page)}`,
    "view r d t m",
    "u source view",
  ];
}

test.describe("the ? sheet", () => {
  test("? opens the key sheet, grouped; Esc closes it and leaves the focus", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    const sp = await stopSp(page, "When the upload hits a 503");
    await focusOn(page, sp);
    // As a US keyboard sends it: Shift held
    await dispatchKey(page, { key: "?", code: "Slash", shiftKey: true });
    await expect.poll(() => sheetOf(page)).toEqual(await sheetLines(page));
    await page.keyboard.press("Escape");
    await expect.poll(() => sheetOf(page)).toBeNull();
    expect(await focused(page)).toBe(sp);
  });

  test("? again closes it, with Shift or without", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    await dispatchKey(page, { key: "?", code: "Slash", shiftKey: true });
    await expect.poll(() => sheetOf(page)).not.toBeNull();
    await dispatchKey(page, { key: "?", code: "Slash", shiftKey: true });
    await expect.poll(() => sheetOf(page)).toBeNull();
    // A layout that types ? without Shift
    await page.keyboard.press("?");
    await expect.poll(() => sheetOf(page)).not.toBeNull();
    await page.keyboard.press("?");
    await expect.poll(() => sheetOf(page)).toBeNull();
  });

  test("any other key closes it and acts, and so does a click on the doc", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    await dispatchKey(page, { key: "?", code: "Slash", shiftKey: true });
    await expect.poll(() => sheetOf(page)).not.toBeNull();
    await page.keyboard.press("j");
    await expect.poll(() => sheetOf(page)).toBeNull();
    await expect.poll(() => focused(page)).not.toBeNull();
    await dispatchKey(page, { key: "?", code: "Slash", shiftKey: true });
    await expect.poll(() => sheetOf(page)).not.toBeNull();
    await page.locator("#content p").first().click();
    await expect.poll(() => sheetOf(page)).toBeNull();
  });

  test("the menu's keys row opens it by keyboard too", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    await page.locator(".file-header birta-phrase").click();
    await expect.poll(async () => (await chrome(page)).menu?.at(-1)).toBe("keys ?");
    for (let i = 0; i < 6; i++) await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect.poll(() => sheetOf(page)).toEqual(await sheetLines(page));
    expect((await chrome(page)).menu).toBeNull();
  });

  test("the phrase menu ends with a keys row that opens it", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    await page.locator(".file-header birta-phrase").click();
    await expect.poll(async () => (await chrome(page)).menu?.at(-1)).toBe("keys ?");
    await page.locator('birta-pop [role="menuitem"]', { hasText: "keys" }).click();
    await expect.poll(() => sheetOf(page)).toEqual(await sheetLines(page));
    expect((await chrome(page)).menu).toBeNull();
  });

  test("after approve, the sheet opens and closes over the clean view, which keeps its keys", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("approved", STOPS_DOC) });
    await expect.poll(async () => (await chrome(page)).phrase).toMatch(/c to reopen$/);
    await dispatchKey(page, { key: "?", code: "Slash", shiftKey: true });
    await expect.poll(() => sheetOf(page)).toEqual(await sheetLines(page));
    await page.keyboard.press("Escape");
    await expect.poll(() => sheetOf(page)).toBeNull();
    await page.keyboard.press("y");
    await expect.poll(() => pillOf(page)).toBe("approved · c to reopen");
    // A clean-view key closes an open sheet and still answers as the clean view does
    await dispatchKey(page, { key: "?", code: "Slash", shiftKey: true });
    await expect.poll(() => sheetOf(page)).not.toBeNull();
    await page.keyboard.press("x");
    await expect.poll(() => sheetOf(page)).toBeNull();
    expect(await pillOf(page)).toBe("approved · c to reopen");
  });

  test("? typed in a note is text", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    await page.keyboard.press("c");
    const area = page.locator('birta-flow[data-kind="composer"] textarea');
    await expect(area).toBeFocused();
    await page.keyboard.press("?");
    await expect(area).toHaveValue("?");
    expect(await sheetOf(page)).toBeNull();
  });
});

test.describe("an unbound letter", () => {
  test("pills `? for keys` once per key per session", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    await page.keyboard.press("h");
    await expect.poll(() => pillOf(page)).toBe("? for keys");
    // Another pill in between shows whether h pills again
    await page.keyboard.press("Shift+J");
    await expect.poll(() => pillOf(page)).toBe("no other file has notes or news");
    await page.keyboard.press("h");
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).not.toBe("? for keys");
    await page.keyboard.press("g");
    await expect.poll(() => pillOf(page)).toBe("? for keys");

    await page.reload();
    await booted(page);
    await expect.poll(async () => (await chrome(page)).phrase).not.toBeNull();
    await page.keyboard.press("h");
    await page.keyboard.press("g");
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).toBeNull();
    await page.keyboard.press("f");
    await expect.poll(() => pillOf(page)).toBe("? for keys");
  });

  test("a new tab is a new session, so it pills again", async ({ page, context, birta }) => {
    const { served } = await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    await page.keyboard.press("h");
    await expect.poll(() => pillOf(page)).toBe("? for keys");
    const other = await context.newPage();
    await other.addInitScript(OPEN_ROOTS);
    await other.setViewportSize(WIDE);
    await mockReview(other, { state: bare("drafts", STOPS_DOC) });
    await other.goto(`${served.url}/view/pr.md`);
    await booted(other);
    await expect.poll(async () => (await chrome(other)).phrase).not.toBeNull();
    await other.keyboard.press("h");
    await expect.poll(() => pillOf(other)).toBe("? for keys");
  });

  test("the bound letters are the page's own bindings, rebound or not", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    // d is the viewer's own dark-mode key
    await page.keyboard.press("d");
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).toBeNull();
    await page.evaluate(() => {
      (window as unknown as { KEYBINDINGS: Record<string, string> }).KEYBINDINGS.yes = "g";
    });
    await page.keyboard.press("g");
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).not.toBe("? for keys");
    await page.keyboard.press("y");
    await expect.poll(() => pillOf(page)).toBe("? for keys");
  });

  test("bound letters, Shift and Ctrl letters, and a plain reader's letters pill nothing of the kind", async ({
    page,
    birta,
  }) => {
    await open(page, birta, STOPS_DOC, { state: bare("drafts", STOPS_DOC) });
    for (const key of ["j", "m", "m", "Shift+H", "Control+h", "Alt+h", "t"]) await page.keyboard.press(key);
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).toBeNull();
  });

  test("a plain reader, with no review shown, gets no pill", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    await page.keyboard.press("h");
    await page.waitForTimeout(SETTLE_MS);
    expect(await chrome(page)).toMatchObject({ phrase: null, pill: null });
  });
});
