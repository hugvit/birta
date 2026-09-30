import { test, expect, Page } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import {
  GESTURES,
  OBJ,
  P_PR,
  type Quote,
  T_PR,
  dblclickWord,
  waitDrawn,
} from "./gestures";

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
