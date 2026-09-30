import { test, expect, Page } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

// One server per project, since projects run this file in parallel
const PORTS: Record<string, number> = { chromium: 3474, webkit: 3475, firefox: 3476 };
// The fixtures folder in directory mode, for owner-text parity over every fixture
const DIR_PORTS: Record<string, number> = { chromium: 3477, webkit: 3478, firefox: 3479 };
const REPO = path.resolve(__dirname, "..", "..");
const QUOTE_JS = path.join(REPO, "assets", "review", "quote.js");
const FIXTURE = path.join(REPO, "e2e", "fixtures", "anchor.md");
const GESTURES_JSON = path.join(REPO, "tests", "fixtures", "anchor_gestures.json");
const FIXTURES = path.join(REPO, "tests", "fixtures");
const FIXTURE_NAMES = fs.readdirSync(FIXTURES).filter((n) => n.endsWith(".md")).sort();
const golden = (name: string) => path.join(FIXTURES, "anchor_goldens", `${name}.txt`);
// Set by scripts/gen-gestures.mjs: record this engine's output there instead of comparing
const RECORD_DIR = process.env.BIRTA_GESTURES_OUT;
// mermaid.min.js is about 3.5 MB from the CDN
const CDN_TIMEOUT = 30_000;

const OBJ = "\u{fffc}";
const T_PR =
  "This PR adds retry with jittered backoff to the upload() client – because S3 returns 503 & drops events… See the docs 🎉. He said “we just fail” and don’t *panic*.";

async function startBirta(port: number, file: string): Promise<ChildProcess> {
  const proc = spawn("cargo", ["run", "--", "--no-open", "--port", String(port), file], {
    cwd: REPO,
    stdio: "pipe",
  });
  const url = `http://127.0.0.1:${port}/health`;
  for (let i = 0; i < 240; i++) {
    try {
      const res = await fetch(url);
      if (res.ok) return proc;
    } catch {
      // server not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  proc.kill();
  throw new Error(`Birta did not start on port ${port} within 120s`);
}

/** Holds a socket open so the server never idles out between tests. */
function keepalive(port: number): WebSocket {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`);
  ws.onopen = () => ws.send(JSON.stringify({ type: "hello", path: "", tab: "selection-keepalive" }));
  return ws;
}

/** What `BR.quote.fromRange` reports, reduced to the fields the server consumes. */
interface Quote {
  sp: string | null;
  quote: string;
  pre: string;
  suf: string;
  off: number | null;
  tl: number | null;
  crossBlock: boolean;
}

interface Gesture {
  id: string;
  act: (page: Page) => Promise<void>;
  /** Expected quote, from the fixture text; omitted where the plan leaves it open. */
  quote?: string;
  /** Expected `crossBlock`, per engine where the engines' selections end differently. */
  crossBlock: boolean | Record<string, boolean>;
  /** Expected code-point offset and text length, where they are the point of the gesture. */
  at?: { off: number; tl: number };
}

type Rect = { left: number; right: number; top: number; bottom: number };

/**
 * The on-screen box of the `nth` occurrence of `word` in the text under `scope`,
 * so gestures land on glyphs rather than on font-dependent offsets.
 */
async function wordRect(page: Page, scope: string, word: string, nth = 0): Promise<Rect> {
  const rect = await page.evaluate(
    ({ scope, word, nth }) => {
      const root = document.querySelector(scope);
      if (!root) return null;
      const box = root.getBoundingClientRect();
      if (box.top < 60 || box.bottom > innerHeight) root.scrollIntoView({ block: "center" });
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
        // KaTeX output is not text a reader would aim at
        acceptNode: (n) =>
          n.parentElement?.closest("[data-math-style]") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
      });
      let seen = 0;
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const data = (n as Text).data;
        for (let at = data.indexOf(word); at >= 0; at = data.indexOf(word, at + 1)) {
          if (seen++ < nth) continue;
          const r = document.createRange();
          r.setStart(n, at);
          r.setEnd(n, at + word.length);
          const b = r.getBoundingClientRect();
          return { left: b.left, right: b.right, top: b.top, bottom: b.bottom };
        }
      }
      return null;
    },
    { scope, word, nth },
  );
  if (!rect) throw new Error(`no "${word}" #${nth} under ${scope}`);
  return rect;
}

const mid = (r: Rect) => ({ x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 });

async function dblclickWord(page: Page, scope: string, word: string, nth = 0) {
  const { x, y } = mid(await wordRect(page, scope, word, nth));
  await page.mouse.dblclick(x, y);
}

async function tripleClickWord(page: Page, scope: string, word: string) {
  const { x, y } = mid(await wordRect(page, scope, word));
  await page.mouse.click(x, y, { clickCount: 3 });
}

/** Drags from just inside the first glyph of `from` to just inside the last glyph of `to`. */
async function dragWords(page: Page, from: [string, string], to: [string, string]) {
  await wordRect(page, ...from);
  const b = await wordRect(page, ...to);
  // Measured after `to` is on screen, in case reaching it scrolled
  const a = await wordRect(page, ...from);
  await page.mouse.move(a.left + 1, (a.top + a.bottom) / 2);
  await page.mouse.down();
  await page.mouse.move(b.right - 1, (b.top + b.bottom) / 2, { steps: 8 });
  await page.mouse.up();
}

const sp = (s: string) => `[data-sourcepos="${s}"]`;
const P_PR = sp("3:1-3:213");
const P_RETRY = sp("5:1-7:70");

// The research gesture matrix (1-11) plus repeats, code, raw HTML and hidden text.
const GESTURES: Gesture[] = [
  {
    id: "01-dblclick-emphasis",
    act: (p) => dblclickWord(p, P_PR, "jittered"),
    quote: "jittered",
    crossBlock: false,
  },
  {
    id: "02-triple-click-paragraph",
    act: (p) => tripleClickWord(p, P_PR, "backoff"),
    quote: T_PR,
    crossBlock: true,
  },
  {
    id: "03-triple-click-heading",
    act: (p) => tripleClickWord(p, "h1" + sp("1:1-1:26"), "design"),
    quote: "Retry “layer” — design",
    crossBlock: true,
  },
  {
    id: "04-triple-click-item-with-nested-list",
    act: (p) => tripleClickWord(p, "li" + sp("10:1-11:22"), "idempotency"),
    quote: "Checked idempotency on every POST",
    crossBlock: true,
  },
  {
    id: "05-triple-click-cell",
    act: (p) => tripleClickWord(p, "td" + sp("16:2-16:10"), "retries"),
    quote: "retries",
    crossBlock: { chromium: false, webkit: true },
  },
  {
    id: "06-triple-click-task-item",
    act: (p) => tripleClickWord(p, "li" + sp("9:1-9:31"), "integration"),
    quote: "Ran the integration suite",
    crossBlock: true,
  },
  {
    id: "07-drag-strong-to-link-end",
    act: (p) => dragWords(p, [P_PR, "adds"], [P_PR, "docs"]),
    quote:
      "adds retry with jittered backoff to the upload() client – because S3 returns 503 & drops events… See the docs",
    crossBlock: false,
  },
  {
    id: "08-drag-into-next-paragraph",
    act: (p) => dragWords(p, [P_PR, "adds"], [P_RETRY, "hard"]),
    quote: T_PR.slice(T_PR.indexOf("adds")),
    crossBlock: true,
  },
  {
    id: "09-drag-across-inline-math",
    act: (p) => dragWords(p, [P_RETRY, "three"], [P_RETRY, "end"]),
    quote: `three Ctrl and math ${OBJ} end`,
    crossBlock: false,
  },
  {
    id: "10-drag-across-footnote-ref",
    act: (p) => dragWords(p, [sp("99:1-99:39"), "Second"], [sp("99:1-99:39"), "with"]),
    quote: `Second ref${OBJ} with`,
    crossBlock: false,
  },
  {
    id: "11-dblclick-after-emoji",
    act: (p) => dblclickWord(p, P_PR, "panic"),
    quote: "panic",
    crossBlock: false,
  },
  {
    // Chromium won't double-click-select an emoji, so this one drags
    id: "12-drag-to-emoji-at-end",
    act: (p) => dragWords(p, [sp("74:5-74:35"), "café"], [sp("74:5-74:35"), "🎉"]),
    quote: "café and 🎉",
    crossBlock: false,
  },
  {
    id: "13-dblclick-repeated-word",
    act: (p) => dblclickWord(p, P_RETRY, "just", 1),
    quote: "just",
    crossBlock: false,
  },
  {
    id: "14-dblclick-in-code",
    act: (p) => dblclickWord(p, "pre" + sp("43:1-47:3"), "retry"),
    quote: "retry",
    crossBlock: false,
  },
  {
    id: "15-drag-in-raw-html",
    act: (p) => dragWords(p, ["details summary", "More"], ["details summary", "More"]),
    quote: "More",
    crossBlock: false,
  },
  {
    // The server can't see the hidden text, so this must never resolve to Exact
    id: "16-triple-click-hidden-span-paragraph",
    act: (p) => tripleClickWord(p, sp("89:1-89:46"), "Hidden"),
    quote: "Hidden inline text.",
    crossBlock: true,
  },
  {
    // The nested <ul> carries the same sp as this <li> and comes first on the page
    id: "17-dblclick-nested-item",
    act: (p) => dblclickWord(p, "li" + sp("11:3-11:22"), "nested"),
    quote: "nested",
    crossBlock: false,
  },
  {
    // A skin tone, a combining accent and a flag: 6 code points, 3 graphemes
    id: "18-dblclick-after-graphemes",
    act: (p) => dblclickWord(p, sp("125:1-125:34"), "ok"),
    quote: "ok",
    crossBlock: false,
    at: { off: 19, tl: 21 },
  },
  {
    // A range can start and end inside an atomic, which no mouse gesture does reliably
    id: "19-range-inside-footnote-ref",
    act: (p) =>
      useRange(p, () => {
        const t = document.querySelector('[data-sourcepos="5:1-7:70"] sup.footnote-ref a')!.firstChild!;
        const r = document.createRange();
        r.setStart(t, 0);
        r.setEnd(t, 1);
        return r;
      }),
    quote: OBJ,
    crossBlock: false,
  },
  {
    id: "20-range-from-inside-math",
    act: (p) =>
      useRange(p, () => {
        const math = document.querySelector('[data-sourcepos="5:1-7:70"] [data-math-style]')!;
        const walker = document.createTreeWalker(math.querySelector(".katex-html") ?? math, NodeFilter.SHOW_TEXT);
        const inside = walker.nextNode()!;
        const after = math.nextSibling as Text;
        const r = document.createRange();
        r.setStart(inside, 0);
        r.setEnd(after, " end".length);
        return r;
      }),
    quote: `${OBJ} end`,
    crossBlock: false,
  },
];

/** Hands `make`'s range to `selectionQuote` in place of the selection, which engines may move. */
async function useRange(page: Page, make: () => Range) {
  await page.evaluate(`window.__range = (${make.toString()})()`);
}

/** Server owner texts (`anchor::owner_text`) for owners with tricky rendering. */
const SERVER_T: Record<string, string> = {
  "1:1-1:26": "Retry “layer” — design",
  "3:1-3:213": T_PR,
  "5:1-7:70": `We just fail hard here, and we just fail again. Soft break line two with spaces hard break line three Ctrl and math ${OBJ} end${OBJ}.`,
  "9:1-9:31": "Ran the integration suite",
  "10:1-11:22": "Checked idempotency on every POST",
  "11:3-11:22": "nested item – one",
  "16:16-16:30": "5 × 8s = 40s",
  "24:7-24:24": "The footnote body.",
  "30:1-30:1": "Term one",
  "31:1-31:32": "Definition with emph – dash",
  "43:1-47:3": "fn retry() {\n\tlet n = 5;\n}\n",
  "49:3-52:5": "indented fence\n  kept indent\n",
  "54:5-56:0": "indented code block\nsecond line\n",
  "74:5-74:35": "inner quote with café and 🎉",
  "87:2-87:9": "🎉",
  "99:1-99:39": `Second ref${OBJ} with blocks in its note.`,
  "117:2-117:9": "a | b",
  "121:2-121:9": "a b",
  "123:1-123:65": `Raw ${OBJ} image and upper break.`,
  "125:1-125:34": "Graphemes 👍🏽 e\u0301 🇮🇸 ok",
  "127:1-127:33": "Spaces a b c d end",
};

const cps = (s: string) => Array.from(s);

/** The quote one owner's whole contents give, keyed by sp, in page order. */
async function ownerTexts(page: Page): Promise<[string, string][]> {
  return page.evaluate(() => {
    const out: [string, string][] = [];
    const owners = document.querySelectorAll(
      "#content :is(p,li,h1,h2,h3,h4,h5,h6,td,th,dt,dd,pre)[data-sourcepos]",
    );
    for (const el of owners) {
      // Diagrams are drawn client-side and anchor at block level only
      if (el.matches("pre.mermaid, pre[data-math-style]")) continue;
      const r = document.createRange();
      r.selectNodeContents(el);
      const q = (window as any).BR.quote.fromRange(r);
      if (!q || !q.quote) continue;
      out.push([el.getAttribute("data-sourcepos")!, q.quote]);
    }
    return out;
  });
}

async function selectionQuote(page: Page): Promise<Quote | null> {
  return page.evaluate(() => {
    const s = getSelection();
    const range = (window as any).__range ?? (s && s.rangeCount ? s.getRangeAt(0) : null);
    if (!range) return null;
    const q = (window as any).BR.quote.fromRange(range);
    if (!q) return null;
    return {
      sp: q.sp ?? null,
      quote: q.quote,
      pre: q.pre ?? "",
      suf: q.suf ?? "",
      off: q.off ?? null,
      tl: q.tl ?? null,
      crossBlock: !!q.crossBlock,
    };
  });
}

/** Waits until diagrams and math are drawn, since drawing moves every block below them. */
async function waitDrawn(page: Page) {
  await expect(page.locator("#content pre.mermaid:not(:has(svg))")).toHaveCount(0, { timeout: CDN_TIMEOUT });
  await expect(page.locator("#content [data-math-style]:not([data-math-rendered])")).toHaveCount(0, {
    timeout: CDN_TIMEOUT,
  });
}

function readCommitted(file: string): string {
  if (!fs.existsSync(file)) {
    throw new Error(`${path.relative(REPO, file)} is missing: run node e2e/scripts/gen-gestures.mjs`);
  }
  return fs.readFileSync(file, "utf-8");
}

const goldenLines = (owners: [string, string][]) =>
  owners.map(([s, t]) => `${s} | ${JSON.stringify(t)}`).join("\n") + "\n";

let server: ChildProcess;
let dirServer: ChildProcess;
let live: WebSocket;
let liveDir: WebSocket;
let dir: string;
let base: string;
let dirBase: string;
const recorded: Record<string, Quote | null> = {};

test.beforeAll(async ({}, testInfo) => {
  const port = PORTS[testInfo.project.name];
  const dirPort = DIR_PORTS[testInfo.project.name];
  base = `http://127.0.0.1:${port}`;
  dirBase = `http://127.0.0.1:${dirPort}`;
  // Copies, so no gesture can ever write to a committed fixture
  dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-selection-")));
  fs.copyFileSync(FIXTURE, path.join(dir, "anchor.md"));
  fs.cpSync(FIXTURES, path.join(dir, "fixtures"), {
    recursive: true,
    filter: (src) => !src.includes("anchor_goldens") && !src.endsWith(".json"),
  });
  [server, dirServer] = await Promise.all([
    startBirta(port, path.join(dir, "anchor.md")),
    startBirta(dirPort, path.join(dir, "fixtures")),
  ]);
  live = keepalive(port);
  liveDir = keepalive(dirPort);
});

test.afterAll(({}, testInfo) => {
  live?.close();
  liveDir?.close();
  server?.kill("SIGTERM");
  dirServer?.kill("SIGTERM");
  fs.rmSync(dir, { recursive: true, force: true });
  if (RECORD_DIR && Object.keys(recorded).length) {
    fs.writeFileSync(
      path.join(RECORD_DIR, `${testInfo.project.name}.json`),
      JSON.stringify(recorded, null, 2) + "\n",
    );
  }
});

test.describe("quote extractor", () => {
  // The script is added as a plain <script>, which the page CSP would refuse without a nonce
  test.use({ bypassCSP: true });

  test.beforeEach(async ({ page }) => {
    await page.setViewportSize({ width: 1100, height: 1000 });
    await page.goto(base);
    await waitDrawn(page);
    await page.addScriptTag({ path: QUOTE_JS });
  });

  test("loads as a classic script without touching an existing BR", async ({ page }) => {
    await page.reload();
    await page.evaluate(() => {
      (window as any).BR = { keep: 1 };
    });
    await page.addScriptTag({ path: QUOTE_JS });
    const shape = await page.evaluate(() => {
      const BR = (window as any).BR;
      return { keep: BR.keep, fromRange: typeof BR.quote?.fromRange, paint: typeof BR.quote?.paint };
    });
    expect(shape).toEqual({ keep: 1, fromRange: "function", paint: "function" });
  });

  test("owner text matches the server's on tricky owners", async ({ page }) => {
    const got = Object.fromEntries(await ownerTexts(page));
    const picked = Object.fromEntries(Object.keys(SERVER_T).map((s) => [s, got[s]]));
    expect(picked).toEqual(SERVER_T);
  });

  test("a whole owner quotes from offset zero with no context", async ({ page }) => {
    const { bad, checked } = await page.evaluate(() => {
      const out: string[] = [];
      let checked = 0;
      for (const el of document.querySelectorAll("#content :is(p,li,h1,h2,h3,h4,h5,h6,td,th,dt,dd,pre)[data-sourcepos]")) {
        if (el.matches("pre.mermaid, pre[data-math-style]")) continue;
        const r = document.createRange();
        r.selectNodeContents(el);
        const q = (window as any).BR.quote.fromRange(r);
        if (!q || !q.quote) continue;
        checked++;
        const want = el.getAttribute("data-sourcepos");
        const len = Array.from(q.quote as string).length;
        if (q.sp !== want || q.off !== 0 || q.tl !== len || q.pre !== "" || q.suf !== "" || q.crossBlock !== false) {
          const got = { sp: q.sp, off: q.off, tl: q.tl, len, pre: q.pre, suf: q.suf, crossBlock: q.crossBlock };
          out.push(`${want}: ${JSON.stringify(got)}`);
        }
      }
      return { bad: out, checked };
    });
    expect(bad).toEqual([]);
    // The server's owners in this doc, less its mermaid and math fences
    expect(checked).toBe(66);
  });

  test("reading quotes leaves the page and the selection alone", async ({ page }) => {
    await dblclickWord(page, P_PR, "jittered");
    const before = await page.evaluate(() => ({
      html: document.getElementById("content")!.innerHTML,
      sel: getSelection()!.toString(),
    }));
    await ownerTexts(page);
    await selectionQuote(page);
    const after = await page.evaluate(() => ({
      html: document.getElementById("content")!.innerHTML,
      sel: getSelection()!.toString(),
    }));
    expect(after).toEqual(before);
  });

  test("a collapsed range or one outside the doc gives no quote", async ({ page }) => {
    const got = await page.evaluate(() => {
      const BR = (window as any).BR;
      const p = document.querySelector('[data-sourcepos="3:1-3:213"]')!;
      const collapsed = document.createRange();
      collapsed.setStart(p.firstChild!, 3);
      const header = document.createRange();
      header.selectNodeContents(document.querySelector(".file-header-name")!);
      // The newline text between two top-level blocks belongs to no owner
      const gap = document.createRange();
      const between = Array.from(document.getElementById("content")!.childNodes).find(
        (n) => n.nodeType === 3 && /^\s+$/.test((n as Text).data),
      )!;
      gap.selectNodeContents(between);
      return [BR.quote.fromRange(collapsed), BR.quote.fromRange(header), BR.quote.fromRange(gap)];
    });
    expect(got).toEqual([null, null, null]);
  });

  test("paint gives nothing for an owner the page lacks or an offset past its text", async ({ page }) => {
    const got = await page.evaluate(() => {
      const BR = (window as any).BR;
      const unknown = BR.quote.paint("999:1-999:9", [0, 1]);
      const past = BR.quote.paint("3:1-3:213", [0, 1000]);
      // Raw HTML can swallow a block the server still indexes
      document.querySelector('[data-sourcepos="99:1-99:39"]')!.remove();
      const removed = BR.quote.paint("99:1-99:39", [0, 6]);
      const kept = BR.quote.paint("3:1-3:213", [0, 7]);
      return [unknown, past, removed, kept ? kept.toString() : "no range"];
    });
    expect(got).toEqual([null, null, null, "This PR"]);
  });

  test("paint resolves an sp shared by a list and its first item", async ({ page }) => {
    const got = await page.evaluate(() => {
      const r = (window as any).BR.quote.paint("11:3-11:22", [0, 6]);
      return r ? r.toString() : null;
    });
    expect(got).toBe("nested");
  });

  test("element boundaries inside an owner map by child index", async ({ page }) => {
    const got = await page.evaluate(() => {
      const BR = (window as any).BR;
      const pick = (s: string, a: number, b: number) => {
        const el = document.querySelector(`[data-sourcepos="${s}"]`)!;
        const r = document.createRange();
        r.setStart(el, a);
        r.setEnd(el, b);
        const q = BR.quote.fromRange(r);
        return q && { sp: q.sp, quote: q.quote, off: q.off, crossBlock: q.crossBlock };
      };
      // Child 1 of the paragraph is <strong>; child 1 of the task item is its text after the box
      return [pick("3:1-3:213", 1, 2), pick("9:1-9:31", 1, 2)];
    });
    expect(got).toEqual([
      { sp: "3:1-3:213", quote: "adds retry", off: 8, crossBlock: false },
      { sp: "9:1-9:31", quote: "Ran the integration suite", off: 0, crossBlock: false },
    ]);
  });

  test("paint follows a block the patcher replaced", async ({ page }) => {
    const got = await page.evaluate(() => {
      const BR = (window as any).BR;
      const first = BR.quote.paint("3:1-3:213", [0, 7]).toString();
      const old = document.querySelector('[data-sourcepos="3:1-3:213"]')!;
      const fresh = old.cloneNode(true) as Element;
      (fresh.firstChild as Text).data = (fresh.firstChild as Text).data.replace("This", "That");
      old.replaceWith(fresh);
      const r = BR.quote.paint("3:1-3:213", [0, 7]);
      return [first, r && r.toString(), r && r.startContainer.isConnected];
    });
    expect(got).toEqual(["This PR", "That PR", true]);
  });

  test("hidden inline text is not owner text", async ({ page }) => {
    const got = await page.evaluate(() => {
      const BR = (window as any).BR;
      const p = document.querySelector('[data-sourcepos="89:1-89:46"]')!;
      const span = p.querySelector("span")!;
      const text = () => {
        const r = document.createRange();
        r.selectNodeContents(p);
        return BR.quote.fromRange(r).quote;
      };
      const out: Record<string, string> = { hidden: text() };
      span.removeAttribute("hidden");
      out.shown = text();
      span.style.display = "none";
      out.displayNone = text();
      span.style.display = "";
      span.style.visibility = "hidden";
      out.visibilityHidden = text();
      span.style.visibility = "";
      span.setAttribute("aria-hidden", "true");
      out.ariaHidden = text();
      return out;
    });
    const gone = "Hidden inline text.";
    expect(got).toEqual({
      hidden: gone,
      shown: "Hidden secret inline text.",
      displayNone: gone,
      visibilityHidden: gone,
      ariaHidden: gone,
    });
  });

  test("a selection in a diagram anchors to its block", async ({ page }) => {
    const got = await page.evaluate(() => {
      const BR = (window as any).BR;
      const read = (el: Node) => {
        const r = document.createRange();
        r.selectNodeContents(el);
        const q = BR.quote.fromRange(r);
        return q && { sp: q.sp, quote: q.quote, off: q.off, tl: q.tl };
      };
      const mermaid = document.querySelector("pre.mermaid")!;
      const label = document.createTreeWalker(mermaid.querySelector("svg")!, NodeFilter.SHOW_TEXT);
      let t = label.nextNode();
      while (t && !(t as Text).data.trim()) t = label.nextNode();
      return [read(mermaid), t && read(t), read(document.querySelector("pre[data-math-style]")!)];
    });
    const block = (sp: string) => ({ sp, quote: OBJ, off: 0, tl: 1 });
    expect(got).toEqual([block("61:1-64:3"), block("61:1-64:3"), block("66:1-68:3")]);
  });

  for (const g of GESTURES) {
    test(`gesture ${g.id}`, async ({ page }, testInfo) => {
      const engine = testInfo.project.name;
      await page.evaluate(() => getSelection()!.removeAllRanges());
      await g.act(page);
      const got = await selectionQuote(page);
      if (RECORD_DIR) recorded[g.id] = got;

      expect(got, "a quote for the selection").not.toBeNull();
      const q = got!;
      expect(q.quote.length, "non-empty quote").toBeGreaterThan(0);
      if (g.quote !== undefined) expect(q.quote).toBe(g.quote);
      const cross = typeof g.crossBlock === "boolean" ? g.crossBlock : g.crossBlock[engine];
      expect(q.crossBlock, "crossBlock").toBe(cross);
      if (g.at) expect({ off: q.off, tl: q.tl }).toEqual(g.at);

      if (g.id.includes("raw-html")) {
        // No owner: the server searches the whole doc for the quote
        expect([q.sp, q.off, q.tl]).toEqual([null, null, null]);
      } else {
        const owner = Object.fromEntries(await ownerTexts(page))[q.sp!];
        expect(owner, `owner ${q.sp} on the page`).toBeDefined();
        const t = cps(owner);
        const len = cps(q.quote).length;
        expect(q.tl).toBe(t.length);
        expect(t.slice(q.off!, q.off! + len).join("")).toBe(q.quote);
        expect(q.pre).toBe(t.slice(Math.max(0, q.off! - 32), q.off!).join(""));
        expect(q.suf).toBe(t.slice(q.off! + len, q.off! + len + 32).join(""));

        // The painted range reads back as the same quote at the same place
        const back = await page.evaluate(
          ({ sp, off }) => {
            const BR = (window as any).BR;
            const r = BR.quote.paint(sp, off);
            if (!r) return null;
            const q = BR.quote.fromRange(r);
            return q && { sp: q.sp, quote: q.quote, off: q.off, tl: q.tl };
          },
          { sp: q.sp!, off: [q.off!, q.off! + len] },
        );
        expect(back).toEqual({ sp: q.sp, quote: q.quote, off: q.off, tl: q.tl });
      }

      if (!RECORD_DIR) {
        const committed = JSON.parse(readCommitted(GESTURES_JSON));
        expect(q, "drift from tests/fixtures/anchor_gestures.json").toEqual(committed.engines?.[engine]?.[g.id]);
      }
    });
  }
});

test.describe("parity with the server", () => {
  test.use({ bypassCSP: true });

  for (const name of FIXTURE_NAMES) {
    test(`owner text of ${name} matches its golden`, async ({ page }, testInfo) => {
      // A remote image in a fixture must not hold up the load event
      await page.goto(`${dirBase}/view/${name}`, { waitUntil: "domcontentloaded" });
      await waitDrawn(page);
      await page.addScriptTag({ path: QUOTE_JS });
      const owners = await ownerTexts(page);
      const sps = owners.map(([s]) => s);
      expect(new Set(sps).size, "one text per sp").toBe(sps.length);
      if (RECORD_DIR) {
        fs.writeFileSync(path.join(RECORD_DIR, `${testInfo.project.name}.golden.${name}.txt`), goldenLines(owners));
        return;
      }
      expect(goldenLines(owners)).toBe(readCommitted(golden(name)));
    });
  }
});

test.describe("under the page CSP", () => {
  test("quote.js runs from a nonced script, where eval and inline handlers are refused", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(String(e)));
    await page.goto(base);
    await waitDrawn(page);
    await page.evaluate((src) => {
      const w = window as any;
      w.__src = src;
      w.__nonce = (document.querySelector("script[nonce]") as HTMLScriptElement).nonce;
      // Scheduled, because code run from the test's own evaluate is exempt from the policy
      const s = document.createElement("script");
      s.nonce = w.__nonce;
      s.textContent = `setTimeout(function () {
        var b = document.createElement("button");
        b.setAttribute("onclick", "window.__inline = 1");
        document.body.appendChild(b);
        b.click();
        b.remove();
        try { new Function("return 1")(); window.__eval = 1; } catch (e) { window.__eval = 0; }
        var q = document.createElement("script");
        q.nonce = window.__nonce;
        q.textContent = window.__src;
        document.head.appendChild(q);
        var r = document.createRange();
        r.selectNodeContents(document.querySelector('[data-sourcepos="3:1-3:213"]'));
        window.__result = {
          inline: window.__inline || null,
          eval: window.__eval,
          quote: BR.quote.fromRange(r).quote,
          painted: String(BR.quote.paint("3:1-3:213", [0, 7]))
        };
      }, 0);`;
      document.head.appendChild(s);
    }, fs.readFileSync(QUOTE_JS, "utf-8"));
    await expect.poll(() => page.evaluate(() => (window as any).__result ?? null)).toEqual({
      inline: null,
      eval: 0,
      quote: T_PR,
      painted: "This PR",
    });
    expect(errors).toEqual([]);
  });
});
