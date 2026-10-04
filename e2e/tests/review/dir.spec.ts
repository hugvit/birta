import { Page } from "@playwright/test";
import { spawnSync } from "child_process";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  BIN,
  GATE_ON,
  Mock,
  REPO,
  State,
  booted,
  dispatchKey,
  fixtureDoc,
  fixtureState,
  focused,
  mockReview,
  revOf,
  withRev,
  stripMessage,
  focusMark,
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

// --- States ------------------------------------------------------------------------------------

/** A doc's `dir_state` summary, exactly as `review::summarize` builds it from the fold. */
function summary(name: string, edit?: (s: State) => void): State {
  const s = fixtureState(name);
  edit?.(s);
  return { stage: s.phrase.stage, phrase: s.phrase, counts: s.counts };
}

function listed(paths: string | string[], sum: State = null, news = false): State {
  return { paths: ([] as string[]).concat(paths), summary: sum, news };
}

function dirState(files: State[], totals: State = {}, folder: State = null): State {
  return {
    type: "dir_state",
    root: "",
    truncated: false,
    folder,
    files,
    totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0, ...totals },
  };
}

/** Turns the mock's own listing into `next` and pushes it, so a resend after `content` agrees. */
function swapDir(mock: Mock, dir: State, next: State) {
  for (const k of Object.keys(dir)) delete dir[k];
  Object.assign(dir, JSON.parse(JSON.stringify(next)));
  mock.push(dir);
}

/** The fields of a committed op frame, without the envelope the core adds. */
function fixtureOp(name: string): State {
  const file = path.join(REPO, "e2e", "fixtures", "review", "ops", `${name}.json`);
  const out = { ...JSON.parse(fs.readFileSync(file, "utf-8")).frame };
  for (const k of ["type", "cid", "path", "rev"]) delete out[k];
  return out;
}

const opsNamed = (mock: Mock, name: string) => mock.ops().filter((o) => o.op === name);

// --- The folder ---------------------------------------------------------------------------------

/** The page: links to listed files, one by its other spelling, one encoded, one with a fragment. */
const LINKS_DOC = [
  "# Folder index",
  "",
  "Read [b](b.md), [c](sub/c.md), [plan](plan.md), [other](other.md), [spaced](my%20notes.md) and [why](b.md#why).",
  "",
  "Closing paragraph.",
  "",
].join("\n");

const FOLDER: Record<string, string> = {
  "pr.md": LINKS_DOC,
  "b.md": "# B\n\n## Why\n\nBecause.\n",
  "sub/c.md": "# C\n",
  "notes/plan.md": "# Plan\n",
  "other.md": "# Other\n",
  "my notes.md": "# Spaced\n",
};
const FOLDER_LINKS = { "plan.md": "notes/plan.md" };

/** Committed state `name` for LINKS_DOC, with nothing placed on its blocks. */
function pageState(name: string, edit?: (s: State) => void): State {
  const s = withRev(fixtureState(name), revOf(LINKS_DOC));
  Object.assign(s, { items: [], changes: [], deletions: [], lost: [], later: [], rewrite: null });
  edit?.(s);
  return s;
}

/** Four other files with activity: three with news, one alias pair without. */
function folderDir(totals: State = { drafts: 6 }): State {
  return dirState(
    [
      listed("b.md", summary("replied"), true),
      listed("sub/c.md", null, true),
      listed(["notes/plan.md", "plan.md"], summary("drafts", (s) => (s.counts.decided = 0)), false),
      listed("pr.md", summary("drafts"), false),
      listed("my notes.md", null, true),
    ],
    totals,
  );
}

interface OpenOpts {
  state?: State;
  dir?: State;
  acks?: "auto" | "manual";
  /** Push the listing once after boot, as the server does, rather than after every `content`. */
  dirOnce?: boolean;
}

/** Serves FOLDER as a folder with review on and the socket mocked; the page is pr.md. */
async function openFolder(page: Page, birta: Start, opts: OpenOpts = {}) {
  const served = await birta(FOLDER, GATE_ON, ["."], FOLDER_LINKS);
  await page.setViewportSize(WIDE);
  const state = opts.state ?? pageState("drafts");
  const dir = opts.dir ?? folderDir();
  const mock = await mockReview(page, { state, dir: opts.dirOnce ? undefined : dir, acks: opts.acks });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  if (opts.dirOnce) mock.push(dir);
  return { served, mock, state, dir };
}

// --- Probes -------------------------------------------------------------------------------------

interface Part {
  text: string;
  fontSize: string;
  fontFamily: string;
  color: string;
  left: number;
  right: number;
}

interface Row {
  text: string;
  current: boolean;
  left: number;
  right: number;
  /** The row's text-bearing elements, in order. */
  parts: Part[];
}

interface Chrome {
  phrase: string | null;
  title: string | null;
  turn: { text: string; kbd: string | null; disabled: boolean } | null;
  message: string | null;
  menu: Row[] | null;
}

/** What the header and the popovers show, read through the (forced open) shadow roots. */
function chrome(page: Page): Promise<Chrome> {
  return page.evaluate(() => {
    const textOf = (n: Node | null): string => {
      let out = "";
      const walk = (x: Node) => {
        if (x.nodeType === 3) {
          out += x.textContent;
          return;
        }
        if (x.nodeType === 1 && ["STYLE", "SCRIPT"].includes((x as Element).tagName)) return;
        const root = x.nodeType === 1 ? (x as Element).shadowRoot : null;
        for (const c of Array.from((root ?? x).childNodes)) walk(c);
      };
      if (n) walk(n);
      return out.replace(/\s+/g, " ").trim();
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
    let turn = null;
    if (btn) {
      const all = textOf(btn);
      const kbd = btn.querySelector("kbd") ? textOf(btn.querySelector("kbd")) : null;
      const text = kbd && all.endsWith(kbd) ? all.slice(0, -kbd.length).trim() : all;
      turn = { text, kbd, disabled: btn.disabled || btn.getAttribute("aria-disabled") === "true" };
    }
    // A message shows in the strip's keys part while the host carries data-message
    const stripHost = document.querySelector("birta-strip");
    const keysPart = stripHost && stripHost.getClientRects().length && stripHost.hasAttribute("data-message")
      ? (stripHost.shadowRoot?.querySelector('[data-part="keys"]') ?? stripHost.querySelector('[data-part="keys"]'))
      : null;
    const message: string | null = keysPart ? textOf(keysPart) || null : null;
    let menu: Row[] | null = null;
    for (const h of Array.from(document.querySelectorAll("birta-pop"))) {
      const root = (h.shadowRoot ?? h) as ParentNode;
      for (const el of Array.from(root.querySelectorAll('[role="menu"]'))) {
        if (!open(el)) continue;
        menu = Array.from(el.querySelectorAll('[role="menuitem"]')).map((r) => {
          // Each element's own text, so a wrapper never stands in for its parts
          const parts = [r, ...Array.from(r.querySelectorAll("*"))]
            .map((e) => ({ e, own: Array.from(e.childNodes).filter((c) => c.nodeType === 3 && (c.textContent ?? "").trim()) }))
            .filter(({ own }) => own.length > 0)
            .map(({ e, own }) => {
              const cs = getComputedStyle(e);
              const range = document.createRange();
              range.setStartBefore(own[0]);
              range.setEndAfter(own[own.length - 1]);
              const box = range.getBoundingClientRect();
              return {
                text: own.map((c) => c.textContent).join("").replace(/\s+/g, " ").trim(),
                fontSize: cs.fontSize,
                fontFamily: cs.fontFamily,
                color: cs.color,
                left: box.left,
                right: box.right,
              };
            });
          const box = r.getBoundingClientRect();
          return {
            text: parts.map((x) => x.text).join(" "),
            current: r.classList.contains("current"),
            left: box.left,
            right: box.right,
            parts,
          };
        });
      }
    }
    return {
      phrase: shown(p) ? textOf(p) : null,
      title: shown(p) ? p.getAttribute("title") : null,
      turn,
      message,
      menu,
    };
  });
}

const phraseOf = async (page: Page) => (await chrome(page)).phrase;
const turnOf = async (page: Page) => (await chrome(page)).turn;
const rowTexts = async (page: Page) => (await chrome(page)).menu?.map((r) => r.text) ?? null;

function hint(page: Page): Promise<string> {
  return page.evaluate(() => (/Mac/.test(navigator.platform) ? "⌘⏎" : "Ctrl⏎"));
}

async function openMenu(page: Page) {
  await page.locator(".file-header birta-phrase").click();
  await expect.poll(() => rowTexts(page)).not.toBeNull();
}

/** `href` of every link in the doc that carries a news dot. */
function dotted(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll("#content a[data-br-news]")).map((a) => a.getAttribute("href")!),
  );
}

/** The composer's textarea, inside its (forced open) root. */
const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"] textarea');

/** The path a page is on, decoded. */
const viewing = (page: Page) => decodeURIComponent(new URL(page.url()).pathname);

// --- The phrase and the button -----------------------------------------------------------------

test.describe("the phrase in a folder", () => {
  test("names the other files with activity, and follows the listing", async ({ page, birta }) => {
    const { mock, dir } = await openFolder(page, birta);
    await expect.poll(() => phraseOf(page)).toBe("your turn · 3 notes · 1 of 3 blocks decided · 4 other files ▾");
    expect((await chrome(page)).title).toBe("your turn · 3 notes · 1 of 3 blocks decided · 4 other files ▾");

    swapDir(mock, dir, dirState([listed("pr.md", summary("drafts")), listed("b.md", null, true)], { drafts: 3 }));
    await expect.poll(() => phraseOf(page)).toBe("your turn · 3 notes · 1 of 3 blocks decided · 1 other file ▾");

    swapDir(mock, dir, dirState([listed("pr.md", summary("drafts"))], { drafts: 3 }));
    await expect.poll(() => phraseOf(page)).toBe("your turn · 3 notes · 1 of 3 blocks decided");
  });

  test("a page under a linked doc's other spelling is not one of the other files", async ({ page, birta }) => {
    const served = await birta(FOLDER, GATE_ON, ["."], FOLDER_LINKS);
    await page.setViewportSize(WIDE);
    const plan = withRev(fixtureState("drafts"), revOf(FOLDER["notes/plan.md"]));
    Object.assign(plan, { items: [], changes: [], deletions: [], lost: [], later: [], rewrite: null });
    await mockReview(page, { state: plan, dir: folderDir() });
    await page.goto(`${served.url}/view/plan.md`);
    await booted(page);
    await expect.poll(() => phraseOf(page)).toBe("your turn · 3 notes · 1 of 3 blocks decided · 4 other files ▾");
  });

});

/** A doc of yours with no review file, which alone shows no chrome (design call 4). */
const noFile = () =>
  pageState("fresh", (s) => {
    s.log.lines = 0;
    s.owner.drafted_by = null;
  });

// Extends design call 4: in a folder, a review file anywhere shows the chrome on every page
test.describe("when the chrome shows in a folder", () => {
  test("a review file on another listed doc shows it on a doc with none", async ({ page, birta }) => {
    await openFolder(page, birta, { state: noFile() });
    await expect.poll(() => phraseOf(page)).toBe("your turn · 4 other files ▾");
  });

  test("the folder's own review file shows it too", async ({ page, birta }) => {
    const dir = dirState([listed("b.md", null, true)], { drafts: 3 }, { summary: summary("drafts") });
    await openFolder(page, birta, { state: noFile(), dir });
    await expect.poll(() => phraseOf(page)).toBe("your turn · 1 other file ▾");
  });

  test("files listed only for news show nothing to a plain reader", async ({ page, birta }) => {
    const dir = dirState([listed("b.md", null, true), listed("sub/c.md", null, true)]);
    await openFolder(page, birta, { state: noFile(), dir });
    // The listing has arrived: b.md's two links and c.md's carry dots
    await expect.poll(async () => (await dotted(page)).length).toBe(3);
    await page.waitForTimeout(SETTLE_MS);
    expect(await chrome(page)).toMatchObject({ phrase: null, turn: null });
  });

  test("serving one file, other docs' review files show nothing", async ({ page, birta }) => {
    const served = await birta(FOLDER, GATE_ON, ["pr.md"], FOLDER_LINKS);
    await page.setViewportSize(WIDE);
    await mockReview(page, { state: noFile(), dir: folderDir() });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await chrome(page)).toMatchObject({ phrase: null, turn: null });
  });
});

test.describe("the button in a folder", () => {
  test("with unseen files elsewhere it reads the carry-forward label, with no key hint", async ({ page, birta }) => {
    const state = pageState("drafts", (s) => (s.counts.drafts = 1));
    await openFolder(page, birta, { state, dir: folderDir({ drafts: 3, unseen_files: 2 }) });
    await expect.poll(() => turnOf(page)).toEqual({ text: "Hand back · 3 · 2 files unseen", kbd: null, disabled: false });
  });

  test("counts every file's drafts, with the key hint", async ({ page, birta }) => {
    await openFolder(page, birta);
    await expect.poll(() => turnOf(page)).toEqual({ text: "Hand back · 6", kbd: await hint(page), disabled: false });
  });

  test("⌘⏎, a click and the composer's ⌘⏎ each hand back the whole folder", async ({ page, birta }) => {
    const { mock } = await openFolder(page, birta);
    const want = fixtureOp("hand_back-all");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 6");

    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => opsNamed(mock, "hand_back").length).toBe(1);
    await page.locator(".file-header birta-turn button").click();
    await expect.poll(() => opsNamed(mock, "hand_back").length).toBe(2);
    // The click leaves focus in the button's host, where review keys don't act
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.keyboard.press("c");
    await composer(page).fill("one more");
    await composer(page).press("ControlOrMeta+Enter");
    await expect.poll(() => opsNamed(mock, "hand_back").length).toBe(3);

    for (const op of opsNamed(mock, "hand_back")) {
      const { type, cid, path: p, rev, ...fields } = op;
      expect(fields).toEqual(want);
    }
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("with no drafts here but some elsewhere, ⌘⏎ hands those back and never approves", async ({ page, birta }) => {
    const { mock } = await openFolder(page, birta, { state: pageState("replied"), dir: folderDir({ drafts: 2 }) });
    await expect.poll(() => turnOf(page)).toEqual({ text: "Hand back · 2", kbd: await hint(page), disabled: false });
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => opsNamed(mock, "hand_back").length).toBe(1);
    expect(opsNamed(mock, "hand_back")[0].scope).toBe("all");
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
  });
});

// --- The menu ----------------------------------------------------------------------------------

test.describe("the menu in a folder", () => {
  test("lists a folder note and every other file after the doc's own rows", async ({ page, birta }) => {
    await openFolder(page, birta);
    await openMenu(page);
    const rows = (await chrome(page)).menu!;
    expect(rows.map((r) => r.text)).toEqual([
      "copy hand-off line",
      "drafted by Claude · mark as yours",
      "note on this folder",
      "b.md Claude replied 14:13 · 1 unseen · 1 of 3 blocks decided · 1 done",
      "sub/c.md changed",
      "notes/plan.md your turn · 3 notes",
      "my notes.md changed",
      "keys ?",
    ]);
  });

  test("a file row is its path in 14px sans and its note in 12px mono, muted, at the right", async ({ page, birta }) => {
    await openFolder(page, birta);
    await openMenu(page);
    const row = (await chrome(page)).menu!.find((r) => r.text.startsWith("b.md"))!;
    const pathPart = row.parts.find((p) => p.text === "b.md")!;
    const note = row.parts.find((p) => p.text === "Claude replied 14:13 · 1 unseen · 1 of 3 blocks decided · 1 done")!;
    expect(pathPart, JSON.stringify(row.parts)).toBeTruthy();
    expect(note, JSON.stringify(row.parts)).toBeTruthy();
    expect(pathPart.fontSize).toBe("14px");
    expect(pathPart.fontFamily).not.toMatch(/mono/i);
    expect(note.fontSize).toBe("12px");
    expect(note.fontFamily).toMatch(/mono/i);
    expect(note.color).not.toBe(pathPart.color);
    // Path on the left padding, note flush with the right padding
    expect(Math.abs(pathPart.left - (row.left + 16))).toBeLessThanOrEqual(2);
    expect(Math.abs(note.right - (row.right - 16))).toBeLessThanOrEqual(2);
    expect(note.left).toBeGreaterThan(pathPart.right);
  });

  test("⏎ on a file row opens that file, and so does a click", async ({ page, birta }) => {
    const { served } = await openFolder(page, birta);
    await openMenu(page);
    for (let i = 0; i < 3; i++) await page.keyboard.press("j");
    expect((await chrome(page)).menu!.find((r) => r.current)!.text).toMatch(/^b\.md /);
    await page.keyboard.press("Enter");
    await page.waitForURL(`${served.url}/view/b.md`);

    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await openMenu(page);
    await page.locator("birta-pop [role=menuitem]", { hasText: "my notes.md" }).click();
    await page.waitForURL(`${served.url}/view/my%20notes.md`);
  });

  test("the folder row opens a note on the folder, and shows the folder's summary", async ({ page, birta }) => {
    const folder = { summary: summary("drafts", (s) => (s.counts.decided = 0)) };
    const dir = dirState(folderDir().files, { drafts: 6 }, folder);
    const { mock } = await openFolder(page, birta, { dir });
    await openMenu(page);
    expect((await chrome(page)).menu!.map((r) => r.text)).toContain("note on this folder your turn · 3 notes");
    await page.locator("birta-pop [role=menuitem]", { hasText: "note on this folder" }).click();
    await expect(composer(page)).toBeFocused();
    // The folder note sits in the doc slot, above the first block
    const above = await page.evaluate(() => {
      const host = document.querySelector('birta-flow[data-kind="composer"]')!;
      const h1 = document.querySelector("#content h1")!;
      return host.getBoundingClientRect().bottom <= h1.getBoundingClientRect().top + 1;
    });
    expect(above).toBe(true);
    await composer(page).fill("Make these consistent.");
    await composer(page).press("Enter");
    await expect.poll(() => opsNamed(mock, "comment").length).toBe(1);
    const { type, cid, path: p, rev, ...fields } = opsNamed(mock, "comment")[0];
    expect(fields).toEqual(fixtureOp("comment-dir"));
  });
});

test.describe("the menu, more", () => {
  test("⏎ after the listing changes under an open menu opens the row shown as current", async ({ page, birta }) => {
    const { mock, dir } = await openFolder(page, birta);
    await openMenu(page);
    for (let i = 0; i < 5; i++) await page.keyboard.press("j");
    expect((await chrome(page)).menu!.find((r) => r.current)!.text).toMatch(/^notes\/plan\.md /);
    const next = folderDir();
    next.files.splice(0, 1); // b.md goes, so the plan row's index now holds my notes.md
    swapDir(mock, dir, next);
    await expect.poll(async () => (await rowTexts(page))?.some((t) => t.startsWith("b.md")) ?? false).toBe(false);
    const current = (await chrome(page)).menu?.find((r) => r.current)?.parts[0].text ?? null;
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    // A closed menu opens nothing; an open one opens what it showed
    expect(viewing(page)).toBe(current ? `/view/${current}` : "/view/pr.md");
    expect(viewing(page)).not.toBe("/view/my notes.md");
  });

  test("a file row shows a path with markup in it as text", async ({ page, birta }) => {
    await openFolder(page, birta, { dir: dirState([listed("<b>x</b> & y.md", null, true)], { drafts: 3 }) });
    await openMenu(page);
    expect(await rowTexts(page)).toContain("<b>x</b> & y.md changed");
    const tags = await page.evaluate(() =>
      Array.from(document.querySelectorAll("birta-pop")).some((h) => !!(h.shadowRoot ?? h).querySelector('[role="menu"] b')),
    );
    expect(tags).toBe(false);
  });

  test("Esc saves a folder note on the folder, typed under its own half-typed key", async ({ page, birta }) => {
    const { mock } = await openFolder(page, birta);
    await page.keyboard.press("c");
    await composer(page).fill("doc words");
    await page.waitForTimeout(SETTLE_MS);
    await composer(page).press("Escape");
    await openMenu(page);
    await page.locator("birta-pop [role=menuitem]", { hasText: "note on this folder" }).click();
    await composer(page).fill("folder words");
    await page.waitForTimeout(SETTLE_MS);
    await composer(page).press("Escape");
    await expect.poll(() => opsNamed(mock, "comment").map((o) => [o.at, o.body])).toEqual([
      [{ k: "doc" }, "doc words"],
      [{ k: "dir" }, "folder words"],
    ]);
    const keys = mock.frames.filter((f) => f.msg.type === "compose" && f.msg.text).map((f) => [f.msg.text, f.msg.key]);
    const keyOf = (t: string) => keys.find(([text]) => text === t)![1];
    expect(keyOf("folder words")).not.toBe(keyOf("doc words"));
  });

  test("serving one file, the menu lists the other files but offers no folder note", async ({ page, birta }) => {
    const served = await birta(FOLDER, GATE_ON, ["pr.md"], FOLDER_LINKS);
    await page.setViewportSize(WIDE);
    await mockReview(page, { state: pageState("drafts"), dir: folderDir() });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await openMenu(page);
    const rows = (await rowTexts(page))!;
    expect(rows).not.toContain("note on this folder");
    expect(rows.some((r) => r.startsWith("note on this folder"))).toBe(false);
    expect(rows).toContain("sub/c.md changed");
  });
});

// --- Link dots ---------------------------------------------------------------------------------

test.describe("link dots", () => {
  test("mark links to files with news, by their decoded path, and nothing else", async ({ page, birta }) => {
    await openFolder(page, birta);
    await expect
      .poll(() => dotted(page))
      .toEqual(["/view/b.md", "/view/sub/c.md", "/view/my%20notes.md", "/view/b.md#why"]);
    const dot = await page.evaluate(() => {
      const cs = getComputedStyle(document.querySelector("#content a[data-br-news]")!, "::after");
      return { content: cs.content, width: cs.width, height: cs.height, radius: cs.borderRadius, display: cs.display };
    });
    expect(dot).toEqual({ content: '""', width: "6px", height: "6px", radius: "50%", display: "inline-block" });
  });

  test("a doc with news marks links to either of its paths", async ({ page, birta }) => {
    const { mock, dir } = await openFolder(page, birta);
    await expect.poll(() => dotted(page)).toContain("/view/b.md");
    swapDir(mock, dir, dirState([listed(["notes/plan.md", "plan.md"], null, true)]));
    await expect.poll(() => dotted(page)).toEqual(["/view/plan.md"]);
  });

  test("a dot goes when its file's news does", async ({ page, birta }) => {
    const { mock, dir } = await openFolder(page, birta);
    await expect.poll(() => dotted(page)).toContain("/view/b.md");
    const next = folderDir();
    next.files[0].news = false;
    swapDir(mock, dir, next);
    await expect.poll(() => dotted(page)).toEqual(["/view/sub/c.md", "/view/my%20notes.md"]);
  });

  // The edit is in the links' own paragraph, so the patch replaces the links themselves
  test("dots come back after the doc is patched", async ({ page, birta }) => {
    const { served } = await openFolder(page, birta, { dirOnce: true });
    await expect.poll(async () => (await dotted(page)).length).toBe(4);
    served.write("pr.md", LINKS_DOC.replace("Read [b]", "Now read [b]"));
    await expect(page.locator("#content")).toContainText("Now read b", { timeout: 10_000 });
    await expect.poll(async () => (await dotted(page)).length).toBe(4);
  });

  test("dots come back after a state ahead of the page makes it fetch the doc", async ({ page, birta }) => {
    const { served, mock, state } = await openFolder(page, birta, { dirOnce: true });
    await expect.poll(async () => (await dotted(page)).length).toBe(4);
    // The server's own content never reaches the page, so only the rev gate's fetch can patch it
    mock.holdContent = true;
    const edited = LINKS_DOC.replace("Read [b]", "Fetched: read [b]");
    served.write("pr.md", edited);
    // The page's fetch must find the new text, so wait until the server has read it
    await expect.poll(async () => (await fetch(`${served.url}/render/pr.md`)).text()).toContain("Fetched: read");
    // Named, since a doc the server reports before the hello lands can move the mock's own path
    mock.pushState(withRev(state, revOf(edited)), "pr.md");
    await expect(page.locator("#content")).toContainText("Fetched: read b", { timeout: 10_000 });
    await expect.poll(async () => (await dotted(page)).length).toBe(4);
  });
});

const EDGE_DOC = [
  "# Edges",
  "",
  "[bad](%E0%A4%A.md) [ext](https://example.com/view/b.md) [proto](//example.com/view/b.md) [top](#edges) [hash](c%231.md) [b](b.md)",
  "",
].join("\n");

test.describe("link dots, more", () => {
  test("dots skip other hosts and same-page anchors, decode after the fragment, and survive a bad escape", async ({
    page,
    birta,
  }) => {
    const served = await birta({ "pr.md": EDGE_DOC, "b.md": "# B\n", "c#1.md": "# Hash\n", "c.md": "# C\n" }, GATE_ON, ["."]);
    await page.setViewportSize(WIDE);
    const state = withRev(fixtureState("drafts"), revOf(EDGE_DOC));
    Object.assign(state, { items: [], changes: [], deletions: [], lost: [], later: [], rewrite: null });
    // This page has news itself, so an href resolved against it would dot its own anchors
    const dir = dirState([listed("pr.md", null, true), listed("b.md", null, true), listed("c#1.md", null, true)]);
    await mockReview(page, { state, dir });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => dotted(page)).toEqual(["/view/c%231.md", "/view/b.md"]);
  });

  test("dots come back after a theme change re-renders the doc", async ({ page, birta }) => {
    const { served, mock } = await openFolder(page, birta, { dirOnce: true });
    await expect.poll(async () => (await dotted(page)).length).toBe(4);
    mock.holdContent = true;
    const edited = LINKS_DOC.replace("Read [b]", "Themed: read [b]");
    served.write("pr.md", edited);
    const html = () => fetch(`${served.url}/render/pr.md`).then((r) => r.text());
    await expect.poll(html).toContain("Themed: read");
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
    await expect(page.locator("#content")).toContainText("Themed: read b", { timeout: 10_000 });
    await expect.poll(async () => (await dotted(page)).length).toBe(4);
  });

  test("a patch elsewhere keeps the dotted links' paragraph as it was", async ({ page, birta }) => {
    const { served } = await openFolder(page, birta, { dirOnce: true });
    await expect.poll(async () => (await dotted(page)).length).toBe(4);
    const para = "document.querySelector('#content a[href=\"/view/b.md\"]').parentElement";
    await page.evaluate(`${para}.__kept = true`);
    served.write("pr.md", LINKS_DOC.replace("Closing paragraph.", "Closing paragraph, edited."));
    await expect(page.locator("#content")).toContainText("Closing paragraph, edited.", { timeout: 10_000 });
    expect(await page.evaluate(`${para}.__kept`)).toBe(true);
  });
});

// --- Shift+J and Shift+K -----------------------------------------------------------------------

test.describe("Shift+J and Shift+K", () => {
  test("go to the next and the previous file with news or drafts", async ({ page, birta }) => {
    const { served } = await openFolder(page, birta);
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.keyboard.press("Shift+J");
    await page.waitForURL(`${served.url}/view/my%20notes.md`);

    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    // plan.md has no news, but its drafts make it a file to go to
    await page.keyboard.press("Shift+K");
    await page.waitForURL(`${served.url}/view/notes/plan.md`);
  });

  test("with no other file with news or notes they stay put, and pill why", async ({ page, birta }) => {
    const dir = dirState([listed("pr.md", summary("drafts"), true), listed("b.md", summary("approved"))], { drafts: 3 });
    const { mock } = await openFolder(page, birta, { dir });
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.keyboard.press("Shift+J");
    await page.keyboard.press("Shift+K");
    await page.waitForTimeout(SETTLE_MS);
    expect(viewing(page)).toBe("/view/pr.md");
    expect(await stripMessage(page)).toBe("no other file has notes or news");
    expect(mock.ops()).toEqual([]);
  });

  test("Shift+J reaches a file whose name has # and ?", async ({ page, birta }) => {
    const served = await birta({ "pr.md": LINKS_DOC, "q #1?.md": "# Q\n" }, GATE_ON, ["."]);
    await page.setViewportSize(WIDE);
    await mockReview(page, { state: pageState("drafts"), dir: dirState([listed("q #1?.md", null, true)], { drafts: 3 }) });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.keyboard.press("Shift+J");
    await expect.poll(() => viewing(page)).toBe("/view/q #1?.md");
    await expect(page.locator("#content h1")).toHaveText("Q");
  });

  test("Caps Lock J is j: it moves focus and stays on the page", async ({ page, birta }) => {
    await openFolder(page, birta);
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await dispatchKey(page, { key: "J", code: "KeyJ" }, "body");
    await expect.poll(() => focused(page)).toBe("1:1-1:14");
    await page.waitForTimeout(SETTLE_MS);
    expect(viewing(page)).toBe("/view/pr.md");
  });

  test("typed in a note they are letters", async ({ page, birta }) => {
    await openFolder(page, birta);
    await page.keyboard.press("c");
    await expect(composer(page)).toBeFocused();
    await page.keyboard.press("Shift+J");
    await page.keyboard.press("Shift+K");
    await page.waitForTimeout(SETTLE_MS);
    expect(viewing(page)).toBe("/view/pr.md");
    await expect(composer(page)).toHaveValue("JK");
  });
});

// --- n and p stay in the file -------------------------------------------------------------------

const WALK: Record<string, string> = {
  "unseen.md": fixtureDoc("unseen"),
  "replied.md": fixtureDoc("replied"),
  "fresh.md": fixtureDoc("fresh"),
};
const WALK_STATES = () => [fixtureState("unseen"), fixtureState("replied"), fixtureState("fresh")];
// In doc order unseen.json walks #13, 9.0, nh:5, 10.0, 11.0; replied.json #11, 9.0, nh:5; fresh.json nothing

async function openWalk(page: Page, birta: Start, dir: State) {
  const served = await birta(WALK, GATE_ON, ["."]);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { states: WALK_STATES(), dir });
  await page.goto(`${served.url}/view/unseen.md`);
  await booted(page);
  await expect.poll(() => phraseOf(page)).not.toBeNull();
  return { served, mock };
}

const seenOn = (mock: Mock, socket: number) =>
  mock.frames.filter((f) => f.socket === socket && f.msg.type === "op" && f.msg.op === "seen").map((f) => f.msg.ids);

test.describe("n and p stay in the file", () => {
  // ⇧J and ⇧K change files; n and p walk this file only, so its ends say so and nothing crosses
  test("n past this file's last stop says no more · p back, and stays, though the next file has news", async ({
    page,
    birta,
  }) => {
    const dir = dirState([
      listed("unseen.md", summary("unseen"), true),
      listed("replied.md", summary("replied"), true),
      listed("fresh.md", summary("fresh")),
    ]);
    const { mock } = await openWalk(page, birta, dir);
    for (let i = 0; i < 5; i++) await page.keyboard.press("n");
    await page.waitForTimeout(SETTLE_MS);
    expect(viewing(page)).toBe("/view/unseen.md");
    expect(seenOn(mock, 0)).toEqual([["#13"], ["9.0"], ["10.0"], ["11.0"]]);

    await page.keyboard.press("n");
    await expect.poll(() => stripMessage(page)).toBe("no more · p back");
    await page.waitForTimeout(1_000);
    expect(viewing(page)).toBe("/view/unseen.md");
    // The last stop is the removal in its gap under ## Stacked on
    expect(await focusMark(page)).toMatchObject({ target: "removal", change: "11.0", after: "14:1-14:13" });
  });

  // Both neighbours have news, so a crossing either way would leave
  for (const [key, order] of [
    ["n", ["fresh.md", "unseen.md", "replied.md"]],
    ["p", ["replied.md", "unseen.md", "fresh.md"]],
  ] as const) {
    test(`${key} at its end never crosses, though both neighbours have news`, async ({ page, birta }) => {
      const dir = dirState(order.map((p) => listed(p, summary(p.replace(".md", "")), true)));
      const { mock } = await openWalk(page, birta, dir);
      if (key === "n") for (let i = 0; i < 6; i++) await page.keyboard.press("n");
      else {
        await page.keyboard.press("n");
        await expect.poll(() => seenOn(mock, 0)).toEqual([["#13"]]);
        await page.keyboard.press("p");
      }
      await expect.poll(() => stripMessage(page)).toBe("no more · p back");
      await page.waitForTimeout(1_000);
      expect(viewing(page)).toBe("/view/unseen.md");
    });
  }

  test("a landing is used once, by the page it was for", async ({ page, birta }) => {
    const dir = dirState([listed("unseen.md", summary("unseen"), true), listed("fresh.md", summary("fresh"), true)]);
    const { served, mock } = await openWalk(page, birta, dir);
    await page.keyboard.press("Shift+J");
    await page.waitForURL(`${served.url}/view/fresh.md`);
    await booted(page);
    // fresh.md had no stop to land on; the next page must not take the landing
    await page.goto(`${served.url}/view/unseen.md`);
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe("doc");
    expect(seenOn(mock, 2)).toEqual([]);
  });

  test("a landing whose page never got its state is not taken by the next page", async ({ page, birta }) => {
    // other.md has no state in the mock, so it never gets to land
    const served = await birta({ ...WALK, "other.md": "# Other\n" }, GATE_ON, ["."]);
    await page.setViewportSize(WIDE);
    const dir = dirState([listed("unseen.md", summary("unseen"), true), listed("other.md", null, true)]);
    const mock = await mockReview(page, { states: WALK_STATES(), dir });
    await page.goto(`${served.url}/view/unseen.md`);
    await booted(page);
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.keyboard.press("Shift+J");
    await page.waitForURL(`${served.url}/view/other.md`);
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    await page.goto(`${served.url}/view/unseen.md`);
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe("doc");
    expect(seenOn(mock, 2)).toEqual([]);
  });

  test("p before this file's first stop says no more · p back, and stays", async ({ page, birta }) => {
    const dir = dirState([
      listed("replied.md", summary("replied"), true),
      listed("unseen.md", summary("unseen"), true),
      listed("fresh.md", summary("fresh")),
    ]);
    const { mock } = await openWalk(page, birta, dir);
    await page.keyboard.press("n");
    await expect.poll(() => seenOn(mock, 0)).toEqual([["#13"]]);
    await page.keyboard.press("p");
    await expect.poll(() => stripMessage(page)).toBe("no more · p back");
    await page.waitForTimeout(1_000);
    expect(viewing(page)).toBe("/view/unseen.md");
    expect(await focused(page)).toBe("doc");
  });

  test("Shift+J onto a file with nothing to walk lands on whole doc and stays", async ({ page, birta }) => {
    const dir = dirState([listed("unseen.md", summary("unseen"), true), listed("fresh.md", summary("fresh"), true)]);
    const { served, mock } = await openWalk(page, birta, dir);
    await page.keyboard.press("Shift+J");
    await page.waitForURL(`${served.url}/view/fresh.md`);
    await booted(page);
    await page.waitForTimeout(1_000);
    expect(viewing(page)).toBe("/view/fresh.md");
    expect(await focused(page)).toBe("doc");
    expect(seenOn(mock, 1)).toEqual([]);
  });

  test("with no other file with news, n at the last stop stays", async ({ page, birta }) => {
    const dir = dirState([listed("unseen.md", summary("unseen"), true), listed("fresh.md", summary("fresh"))]);
    const { mock } = await openWalk(page, birta, dir);
    for (let i = 0; i < 7; i++) await page.keyboard.press("n");
    await page.waitForTimeout(SETTLE_MS);
    expect(viewing(page)).toBe("/view/unseen.md");
    expect(seenOn(mock, 0)).toEqual([["#13"], ["9.0"], ["10.0"], ["11.0"]]);
  });
});

// --- Frames that come before review starts -------------------------------------------------------

/** Holds the review script back, so the hello's frames all reach the page before review starts. */
async function holdReviewScript(page: Page, ms = 800) {
  await page.route(/\/_birta\/review\.js$/, async (route) => {
    await new Promise((r) => setTimeout(r, ms));
    await route.continue();
  });
}

interface Bus {
  birtaBus: { on(fn: (m: { type: string; i?: number; path?: string }) => void): void; emit(m: object): void };
  __got: Record<string, { type: string; i?: number; path?: string }[]>;
}

/** Subscribes a collector named `name` to the page's bus. */
function subscribe(page: Page, name: string): Promise<void> {
  return page.evaluate((name) => {
    const w = window as unknown as Bus;
    w.__got = w.__got ?? {};
    w.__got[name] = [];
    w.birtaBus.on((m) => w.__got[name].push({ type: m.type, i: m.i, path: m.path }));
  }, name);
}

const got = (page: Page, name: string) =>
  page.evaluate((name) => (window as unknown as Bus).__got[name], name);

/** Opens pr.md while review.js is held, once the hello's frames have reached the page. */
async function openHeld(page: Page, birta: Start, env: Record<string, string> = GATE_ON) {
  await holdReviewScript(page, 4_000);
  const served = await birta(FOLDER, env, ["."], FOLDER_LINKS);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state: pageState("drafts"), dir: folderDir() });
  await page.goto(`${served.url}/view/pr.md`, { waitUntil: "commit" });
  await expect.poll(() => mock.frames.some((f) => f.msg.type === "hello")).toBe(true);
  await page.waitForTimeout(SETTLE_MS);
  return mock;
}

test.describe("the bus before review starts", () => {
  test("every subscriber gets the frames that came before it, in order", async ({ page, birta }) => {
    await openHeld(page, birta);
    await subscribe(page, "a");
    await subscribe(page, "b");
    for (const name of ["a", "b"]) {
      const seen = (await got(page, name)).filter((x) => x.type !== "open" && x.type !== "theme_update");
      // The hello's own three, in the order they came; the watcher may report other docs first
      const at = seen.findIndex((x) => x.type === "content" && x.path === "pr.md");
      expect(at, name).toBeGreaterThanOrEqual(0);
      expect(seen.slice(at, at + 3).map((x) => x.type), name).toEqual(["content", "review_state", "dir_state"]);
    }
  });

  test("it keeps the newest 256", async ({ page, birta }) => {
    await openHeld(page, birta);
    await page.evaluate(() => {
      for (let i = 0; i < 300; i++) (window as unknown as Bus).birtaBus.emit({ type: "x", i });
    });
    await subscribe(page, "a");
    // `open` is the subscriber's own greeting, never buffered
    const all = (await got(page, "a")).filter((m) => m.type !== "open");
    const xs = all.filter((m) => m.type === "x").map((m) => m.i);
    expect(all.length).toBeLessThanOrEqual(256);
    expect(xs[xs.length - 1]).toBe(299);
    expect(xs).toEqual(Array.from({ length: xs.length }, (_, k) => 300 - xs.length + k));
  });

  test("with the gate off nothing is kept for later", async ({ page, birta }) => {
    const served = await birta(FOLDER, {}, ["."], FOLDER_LINKS);
    await page.goto(`${served.url}/view/pr.md`);
    await expect(page.locator("h1")).toHaveText("Folder index");
    await page.evaluate(() => (window as unknown as Bus).birtaBus.emit({ type: "x", i: 1 }));
    await subscribe(page, "a");
    expect((await got(page, "a")).filter((m) => m.type !== "open")).toEqual([]);
  });
});

test.describe("a listing that comes before review starts", () => {
  test("still drives the phrase, the menu, the dots and Shift+J", async ({ page, birta }) => {
    await holdReviewScript(page);
    const { served } = await openFolder(page, birta);
    await expect.poll(() => phraseOf(page)).toBe("your turn · 3 notes · 1 of 3 blocks decided · 4 other files ▾");
    await expect.poll(() => turnOf(page)).toEqual({ text: "Hand back · 6", kbd: await hint(page), disabled: false });
    await expect.poll(async () => (await dotted(page)).length).toBe(4);
    await openMenu(page);
    expect(await rowTexts(page)).toContain("note on this folder");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Shift+J");
    await page.waitForURL(`${served.url}/view/my%20notes.md`);
  });

  test("Shift+J still lands on the next file's first stop", async ({ page, birta }) => {
    await holdReviewScript(page);
    const dir = dirState([listed("unseen.md", summary("unseen"), true), listed("replied.md", summary("replied"), true)]);
    const { served, mock } = await openWalk(page, birta, dir);
    await page.keyboard.press("Shift+J");
    await page.waitForURL(`${served.url}/view/replied.md`);
    await booted(page);
    await expect.poll(() => seenOn(mock, 1)).toEqual([["#11"]]);
    expect(await focused(page)).toBe("doc");
  });
});

// --- Real servers ------------------------------------------------------------------------------

/** A review file holding doc-level drafts with `bodies`, as the page writes them. */
function seededLog(name: string, text: string, bodies: string[]): string {
  const t = Date.now() - 60_000;
  const lines = [
    { birta: "review/1", doc: name, about: "ops after the last hand-back are drafts; don't act on them" },
    { by: { type: "birta" }, op: "snap", rev: revOf(text), text, ts: t },
    { by: { type: "birta" }, drafted_by: null, op: "owner", ts: t + 1 },
    ...bodies.map((body, i) => ({
      at: "doc",
      body,
      by: { type: "human", via: "browser" },
      cid: `seed-${i}`,
      op: "comment",
      ts: t + 2 + i,
    })),
  ];
  return lines.map((l) => JSON.stringify(l)).join("\n") + "\n";
}

/** Each line of a review file with its line number, `n`. */
function numbered(file: string): State[] {
  if (!fs.existsSync(file)) return [];
  return fs
    .readFileSync(file, "utf-8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l, i) => ({ ...JSON.parse(l), n: i + 1 }));
}

const linesNamed = (file: string, op: string) => numbered(file).filter((l) => l.op === op);
const bodies = (file: string) => linesNamed(file, "comment").map((l) => l.body);

/** Runs `birta review` against the served folder, as an agent in that folder would. */
function birtaReview(root: string, args: string[]) {
  const base = path.dirname(root);
  return spawnSync(BIN, ["review", ...args], {
    cwd: root,
    env: {
      PATH: process.env.PATH ?? "",
      HOME: path.join(base, "home"),
      XDG_STATE_HOME: path.join(base, "state"),
      BIRTA_REVIEW: "1",
    },
    encoding: "utf-8",
    timeout: 30_000,
  });
}

function dirmode(rel: string): string {
  return fs.readFileSync(path.join(REPO, "e2e", "fixtures", "dirmode", rel), "utf-8");
}

const VAULT_DOC = "# Vault notes\n\nA paragraph in the linked folder.\n";

test.describe("on a real server", () => {
  test("a note under a linked folder and one at the root go back as one batch, and birta review shows both", async ({
    page,
    birta,
  }) => {
    const served = await birta(
      {
        "index.md": dirmode("index.md"),
        "README.md": dirmode("README.md"),
        "sub/page.md": dirmode("sub/page.md"),
        "../vault/notes.md": VAULT_DOC,
      },
      { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "300" },
      ["."],
      { linked: "../vault" },
    );
    const vault = path.join(path.dirname(served.root), "vault");
    const linkedLog = path.join(vault, "notes.md.birta.jsonl");
    const indexLog = path.join(served.root, "index.md.birta.jsonl");
    await page.setViewportSize(WIDE);

    await page.goto(`${served.url}/view/linked/notes.md`);
    await booted(page);
    await page.keyboard.press("j");
    await page.keyboard.press("c");
    await composer(page).fill("LINKED-NOTE");
    await composer(page).press("Enter");
    // The review file sits beside the doc's canonical path, in the link's target
    await expect.poll(() => bodies(linkedLog)).toEqual(["LINKED-NOTE"]);
    expect(fs.realpathSync(path.join(served.root, "linked", "notes.md.birta.jsonl"))).toBe(fs.realpathSync(linkedLog));

    await page.goto(`${served.url}/view/index.md`);
    await booted(page);
    await page.keyboard.press("c");
    await composer(page).fill("INDEX-NOTE");
    await composer(page).press("Enter");
    await expect.poll(() => bodies(indexLog)).toEqual(["INDEX-NOTE"]);

    await expect.poll(async () => (await turnOf(page))?.text, { timeout: 5_000 }).toBe("Hand back · 2");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => [linesNamed(indexLog, "submit").length, linesNamed(linkedLog, "submit").length]).toEqual([1, 1]);
    const [a] = linesNamed(indexLog, "submit");
    const [b] = linesNamed(linkedLog, "submit");
    expect(typeof a.batch).toBe("number");
    expect(a.batch).toBe(b.batch);

    const r = birtaReview(served.root, [".", "--wait", "--timeout", "10", "--as", "claude"]);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("INDEX-NOTE");
    expect(r.stdout).toContain("LINKED-NOTE");
    expect(r.stdout).toContain("linked/notes.md");
    expect(r.stdout.indexOf("INDEX-NOTE")).toBeLessThan(r.stdout.indexOf("LINKED-NOTE"));
    // One run picked up the whole batch
    for (const [log, submit] of [
      [indexLog, a],
      [linkedLog, b],
    ] as [string, State][]) {
      const claims = linesNamed(log, "claim");
      expect(claims.length, log).toBe(1);
      expect(claims[0].round, log).toBe(submit.n);
      expect(claims[0].by.name, log).toBe("claude");
    }
  });

  test("serving one file, the button counts and hands back that doc's drafts only", async ({ page, birta }) => {
    const served = await birta(
      { "pr.md": "# PR\n\nText.\n", "b.md": "# B\n\nText.\n", "b.md.birta.jsonl": seededLog("b.md", "# B\n\nText.\n", ["B-NOTE"]) },
      { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "300" },
    );
    await page.setViewportSize(WIDE);
    // Opened once, so this one-file server lists the sibling
    await page.goto(`${served.url}/view/b.md`);
    await booted(page);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("c");
    await composer(page).fill("PR-NOTE");
    await composer(page).press("Enter");
    await expect.poll(() => bodies(path.join(served.root, "pr.md.birta.jsonl"))).toEqual(["PR-NOTE"]);
    // The sibling is listed, so its draft is in the totals the button must not count
    await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toBe("your turn · 1 note · 1 other file ▾");
    expect((await turnOf(page))?.text).toBe("Hand back · 1");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => linesNamed(path.join(served.root, "pr.md.birta.jsonl"), "submit").length).toBe(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(linesNamed(path.join(served.root, "b.md.birta.jsonl"), "submit")).toEqual([]);
  });

  test("serving a folder, the button counts every file's drafts and hands them all back", async ({ page, birta }) => {
    const served = await birta(
      {
        "pr.md": "# PR\n\nText.\n",
        "b.md": "# B\n\nText.\n",
        "b.md.birta.jsonl": seededLog("b.md", "# B\n\nText.\n", ["B-NOTE", "B-OTHER"]),
        "c.md": "# C\n\nText.\n",
      },
      { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "300" },
      ["."],
    );
    await page.setViewportSize(WIDE);
    // A doc with no drafts of its own still sends the folder's
    await page.goto(`${served.url}/view/c.md`);
    await booted(page);
    await page.keyboard.press("j");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 2");

    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("c");
    await composer(page).fill("PR-NOTE");
    await composer(page).press("Enter");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    await page.keyboard.press("ControlOrMeta+Enter");
    const pr = path.join(served.root, "pr.md.birta.jsonl");
    const b = path.join(served.root, "b.md.birta.jsonl");
    await expect.poll(() => [linesNamed(pr, "submit").length, linesNamed(b, "submit").length]).toEqual([1, 1]);
    expect(linesNamed(pr, "submit")[0].batch).toBe(linesNamed(b, "submit")[0].batch);
    expect(fs.existsSync(path.join(served.root, "c.md.birta.jsonl"))).toBe(false);
  });

  test("a folder note lands in the folder's review file and goes back with the docs", async ({ page, birta }) => {
    const served = await birta({ "pr.md": "# PR\n\nText.\n" }, { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "300" }, ["."]);
    const folderLog = path.join(served.root, ".birta.jsonl");
    const prLog = path.join(served.root, "pr.md.birta.jsonl");
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("j");
    await openMenu(page);
    await page.locator("birta-pop [role=menuitem]", { hasText: "note on this folder" }).click();
    await composer(page).fill("FOLDER-NOTE");
    await composer(page).press("Enter");
    await expect.poll(() => bodies(folderLog)).toEqual(["FOLDER-NOTE"]);
    expect(linesNamed(folderLog, "comment")[0].at).toBe("dir");
    expect(fs.existsSync(prLog)).toBe(false);

    await page.keyboard.press("Escape");
    await page.keyboard.press("c");
    await composer(page).fill("DOC-NOTE");
    await composer(page).press("Enter");
    await expect.poll(() => bodies(prLog)).toEqual(["DOC-NOTE"]);
    await expect.poll(async () => (await turnOf(page))?.text, { timeout: 5_000 }).toBe("Hand back · 2");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => [linesNamed(folderLog, "submit").length, linesNamed(prLog, "submit").length]).toEqual([1, 1]);
    expect(linesNamed(folderLog, "submit")[0].batch).toBe(linesNamed(prLog, "submit")[0].batch);

    const r = birtaReview(served.root, [".", "--wait", "--timeout", "10", "--as", "claude"]);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout.indexOf("FOLDER-NOTE")).toBeGreaterThanOrEqual(0);
    expect(r.stdout.indexOf("FOLDER-NOTE")).toBeLessThan(r.stdout.indexOf("DOC-NOTE"));
  });

  test("z inside a folder's hold takes back every file's hand-back, and the pill counts them all", async ({
    page,
    birta,
  }) => {
    const served = await birta(
      {
        "pr.md": "# PR\n\nText.\n",
        "b.md": "# B\n\nText.\n",
        "b.md.birta.jsonl": seededLog("b.md", "# B\n\nText.\n", ["B-NOTE", "B-OTHER"]),
      },
      { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "6000" },
      ["."],
    );
    const pr = path.join(served.root, "pr.md.birta.jsonl");
    const b = path.join(served.root, "b.md.birta.jsonl");
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("c");
    await composer(page).fill("PR-NOTE");
    await composer(page).press("Enter");
    await expect.poll(async () => (await turnOf(page))?.text, { timeout: 5_000 }).toBe("Hand back · 3");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => [linesNamed(pr, "submit").length, linesNamed(b, "submit").length]).toEqual([1, 1]);
    await expect.poll(() => stripMessage(page)).toBe("handing back · 3 notes · z to undo");

    await page.keyboard.press("z");
    for (const log of [pr, b]) {
      const submit = linesNamed(log, "submit")[0];
      await expect.poll(() => linesNamed(log, "withdraw").map((w) => w.to), { message: log }).toEqual([submit.n]);
    }
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
  });

  test("a doc with no drafts of its own hands back the folder's, and z there takes them back", async ({ page, birta }) => {
    const served = await birta(
      {
        "c.md": "# C\n\nText.\n",
        "b.md": "# B\n\nText.\n",
        "b.md.birta.jsonl": seededLog("b.md", "# B\n\nText.\n", ["B-NOTE", "B-OTHER"]),
      },
      { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "6000" },
      ["."],
    );
    const b = path.join(served.root, "b.md.birta.jsonl");
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/c.md`);
    await booted(page);
    await page.keyboard.press("j");
    await expect.poll(async () => (await turnOf(page))?.text, { timeout: 5_000 }).toBe("Hand back · 2");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => linesNamed(b, "submit").length).toBe(1);
    await expect.poll(() => stripMessage(page)).toBe("handing back · 2 notes · z to undo");
    await page.keyboard.press("z");
    const submit = linesNamed(b, "submit")[0];
    await expect.poll(() => linesNamed(b, "withdraw").map((w) => w.to)).toEqual([submit.n]);
    expect(fs.existsSync(path.join(served.root, "c.md.birta.jsonl"))).toBe(false);
  });

  test("another tab opening a file clears its dot here without a reload", async ({ page, context, birta }) => {
    const served = await birta({ "index.md": "# Index\n\nSee [b](b.md).\n", "b.md": "# B\n" }, GATE_ON, ["."]);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/index.md`);
    await booted(page);
    served.write("b.md", "# B edited\n");
    await expect.poll(() => dotted(page), { timeout: 5_000 }).toEqual(["/view/b.md"]);
    const other = await context.newPage();
    await other.goto(`${served.url}/view/b.md`);
    await booted(other);
    await expect.poll(() => dotted(page), { timeout: 5_000 }).toEqual([]);
  });

  test("z after a folder note takes back nothing of this doc's", async ({ page, birta }) => {
    const text = "# PR\n\nText.\n";
    const served = await birta(
      { "pr.md": text, "pr.md.birta.jsonl": seededLog("pr.md", text, ["KEEP-ME"]) },
      GATE_ON,
      ["."],
    );
    const folderLog = path.join(served.root, ".birta.jsonl");
    const prLog = path.join(served.root, "pr.md.birta.jsonl");
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await openMenu(page);
    await page.locator("birta-pop [role=menuitem]", { hasText: "note on this folder" }).click();
    await composer(page).fill("FOLDER-NOTE");
    await composer(page).press("Enter");
    // The folder note sits on the same line number as this doc's own note
    await expect.poll(() => linesNamed(folderLog, "comment").map((l) => l.n)).toEqual([linesNamed(prLog, "comment")[0].n]);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(linesNamed(prLog, "retract")).toEqual([]);
    expect(linesNamed(folderLog, "retract")).toEqual([]);
  });

  test("links to files written since start get a dot, which goes once the file is viewed", async ({ page, birta }) => {
    const index = "# Index\n\nSee [b](b.md) and [notes](linked/notes.md).\n";
    const served = await birta(
      { "index.md": index, "b.md": "# B\n", "../vault/notes.md": VAULT_DOC },
      GATE_ON,
      ["."],
      { linked: "../vault" },
    );
    const vault = path.join(path.dirname(served.root), "vault");
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/index.md`);
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await dotted(page)).toEqual([]);

    served.write("b.md", "# B edited\n");
    await expect.poll(() => dotted(page), { timeout: 5_000 }).toEqual(["/view/b.md"]);
    fs.writeFileSync(path.join(vault, "notes.md"), VAULT_DOC + "\nMore.\n");
    await expect.poll(() => dotted(page), { timeout: 5_000 }).toEqual(["/view/b.md", "/view/linked/notes.md"]);

    await page.getByRole("link", { name: "b", exact: true }).click();
    await page.waitForURL(`${served.url}/view/b.md`);
    await booted(page);
    await page.goto(`${served.url}/view/index.md`);
    await booted(page);
    await expect.poll(() => dotted(page), { timeout: 5_000 }).toEqual(["/view/linked/notes.md"]);
  });
});

// --- No dir_state, no change -------------------------------------------------------------------

test("with no folder listing the chrome is what it was", async ({ page, birta }) => {
  const served = await birta(FOLDER, GATE_ON, ["."], FOLDER_LINKS);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state: pageState("drafts") });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  await expect.poll(() => phraseOf(page)).toBe("your turn · 3 notes · 1 of 3 blocks decided");
  expect(await turnOf(page)).toEqual({ text: "Hand back · 3", kbd: await hint(page), disabled: false });
  expect(await dotted(page)).toEqual([]);
  await page.keyboard.press("Shift+J");
  await page.waitForTimeout(SETTLE_MS);
  expect(viewing(page)).toBe("/view/pr.md");
  expect(mock.ops()).toEqual([]);
});
