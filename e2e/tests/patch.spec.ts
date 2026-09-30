import { test, expect, Page, WebSocketRoute } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

// One server per project, since projects run this file in parallel
const PORTS: Record<string, number> = { chromium: 3470, webkit: 3471, firefox: 3472 };
const REPO = path.resolve(__dirname, "..", "..");
// mermaid.min.js is about 3.5 MB from the CDN
const CDN_TIMEOUT = 30_000;

async function startBirta(port: number, file: string, args: string[]): Promise<ChildProcess> {
  const proc = spawn(
    "cargo",
    ["run", "--", "--no-open", "--port", String(port), ...args, file],
    { cwd: REPO, stdio: "pipe" },
  );
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

/** Holds a socket open, so tests whose page socket is mocked can't idle the server out. */
function keepalive(port: number): WebSocket {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`);
  ws.onopen = () => ws.send(JSON.stringify({ type: "hello", path: "", tab: "patch-keepalive" }));
  return ws;
}

/** `n` top-level blocks: a title, then paragraphs with a heading every 20. */
function longDoc(title: string, n = 200): string {
  const blocks = [`# ${title}`];
  for (let i = 1; i < n; i++) {
    blocks.push(
      i % 20 === 0
        ? `## Section ${i / 20}`
        : `Block ${i}. ${"lorem ipsum dolor sit amet ".repeat(1 + (i % 4)).trim()}`,
    );
  }
  return blocks.join("\n\n") + "\n";
}

/** Puts `extra` blocks right after the title, well above a mid-page viewport. */
function insertAfterTitle(doc: string, extra: string[]): string {
  const cut = doc.indexOf("\n\n") + 2;
  return doc.slice(0, cut) + extra.join("\n\n") + "\n\n" + doc.slice(cut);
}

const INSERTED = Array.from(
  { length: 6 },
  (_, i) => `Inserted ${i + 1}. ${"new text above the reader ".repeat(3).trim()}`,
);

const IDENT = `# Ident

Alpha paragraph.

Delta has a [link](https://a.example/).

Beta has \`data-sourcepos="1:1-1:5"\` in code.

Inline $x^2$ math.

\`\`\`rust
fn main() {}
\`\`\`

\`\`\`mermaid
graph TD
  A-->B
\`\`\`

Gamma paragraph.

- [ ] one
- [ ] two

Omega paragraph.
`;

/** A long doc whose footnotes section, the last block, changes with any new note. */
function footnoteDoc(title: string): string {
  return longDoc(title).replace("Block 2. ", "Block 2[^1]. ") + "\n[^1]: Note one.\n";
}

const TASKS = "# Tasks\n\nIntro.\n\n- [ ] one\n- [ ] two\n\nEnd.\n";

const REFETCH = "# Refetch\n\nAlpha paragraph.\n\nGamma paragraph.\n\nOmega paragraph.\n";

const TALL_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="400"><rect width="40" height="400"/></svg>';

function randomRev(): string {
  return Array.from({ length: 40 }, () => "0123456789abcdef"[Math.floor(Math.random() * 16)]).join("");
}

async function nextFrames(page: Page): Promise<void> {
  await page.evaluate(
    () => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))),
  );
}

/** Loads `rel` with a mocked socket, so the only frames the page sees are the test's. */
async function openMocked(page: Page, base: string, rel: string): Promise<WebSocketRoute> {
  let route: WebSocketRoute | undefined;
  await page.routeWebSocket(/\/ws$/, (ws) => {
    route = ws;
  });
  await page.goto(`${base}/view/${rel}`);
  await expect.poll(() => route !== undefined).toBe(true);
  return route!;
}

/** Loads `rel` against the real server and waits for the reply to its hello. */
async function openLive(page: Page, base: string, rel: string): Promise<void> {
  let answered = false;
  page.on("websocket", (ws) =>
    ws.on("framereceived", ({ payload }) => {
      try {
        const msg = JSON.parse(payload.toString());
        if (msg.type === "content" && msg.path === rel) answered = true;
      } catch {
        // not JSON
      }
    }),
  );
  await page.goto(`${base}/view/${rel}`);
  await expect.poll(() => answered).toBe(true);
  await nextFrames(page);
}

/** The server's current render of `rel`, fetched by the page. */
function render(page: Page, rel: string): Promise<string> {
  return page.evaluate((p) => fetch("/render/" + p).then((r) => r.text()), rel);
}

function content(rel: string, html: string): string {
  return JSON.stringify({ type: "content", path: rel, rev: randomRev(), html });
}

/** Remembers the doc's top-level blocks, to compare node identity after a patch. */
async function stashBlocks(page: Page): Promise<void> {
  await page.evaluate(() => {
    (window as any).__blocks = Array.from(document.getElementById("content")!.children);
  });
}

/** Indices of stashed blocks that are no longer the node at the same place. */
function replacedBlocks(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const before: Element[] = (window as any).__blocks;
    const after = Array.from(document.getElementById("content")!.children);
    const out: number[] = [];
    before.forEach((el, i) => {
      if (after[i] !== el || !el.isConnected) out.push(i);
    });
    if (after.length !== before.length) out.push(-1);
    return out;
  });
}

function blockIndex(page: Page, prefix: string): Promise<number> {
  return page.evaluate(
    (p) =>
      Array.from(document.getElementById("content")!.children).findIndex((el) =>
        (el.textContent || "").trim().startsWith(p),
      ),
    prefix,
  );
}

/** Text and viewport top of the first block below the sticky headers, and of one lower down. */
function visibleBlocks(page: Page): Promise<{ text: string; top: number }[]> {
  return page.evaluate(() => {
    const header = document.querySelector(".header")!.getBoundingClientRect();
    const file = document.querySelector(".file-header")!.getBoundingClientRect();
    const covered = Math.max(header.bottom, file.top >= 0 ? file.bottom : 0);
    const kids = Array.from(document.getElementById("content")!.children);
    const first = kids.findIndex((el) => el.getBoundingClientRect().bottom > covered);
    return [kids[first], kids[first + 5]].map((el) => ({
      text: (el.textContent || "").trim(),
      top: el.getBoundingClientRect().top,
    }));
  });
}

/** Viewport top of the block whose text is `text`. */
function topOf(page: Page, text: string): Promise<number | null> {
  return page.evaluate((t) => {
    const el = Array.from(document.getElementById("content")!.children).find(
      (e) => (e.textContent || "").trim() === t,
    );
    return el ? el.getBoundingClientRect().top : null;
  }, text);
}

async function expectInPlace(page: Page, before: { text: string; top: number }[]): Promise<void> {
  for (const b of before) {
    const top = await topOf(page, b.text);
    expect(top, `"${b.text.slice(0, 12)}" is gone`).not.toBeNull();
    expect(Math.abs(top! - b.top), `"${b.text.slice(0, 12)}" moved from ${b.top} to ${top}`).toBeLessThanOrEqual(1);
  }
}

/** `scrollY` once a smooth scroll has stopped moving. */
async function settledScroll(page: Page): Promise<number> {
  let last = -1;
  for (;;) {
    const y = await page.evaluate(() => window.scrollY);
    if (y === last) return y;
    last = y;
    await page.waitForTimeout(150);
  }
}

async function scrollHalfway(page: Page): Promise<void> {
  await page.evaluate(() =>
    window.scrollTo(0, (document.documentElement.scrollHeight - window.innerHeight) / 2),
  );
  await nextFrames(page);
  expect(await page.evaluate(() => window.scrollY)).toBeGreaterThan(1000);
}

test.describe("page frame", () => {
  test("the doc is a zero-padding article inside #lane-frame", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/");
    const frame = await page.evaluate(() => {
      const el = document.getElementById("content")!;
      const lane = el.parentElement!;
      const cs = getComputedStyle(el);
      const ls = getComputedStyle(lane);
      return {
        parentId: lane.id,
        isArticle: el.tagName === "ARTICLE" && el.classList.contains("markdown-body"),
        padding: [cs.paddingTop, cs.paddingRight, cs.paddingBottom, cs.paddingLeft],
        lanePadding: [ls.paddingTop, ls.paddingRight, ls.paddingBottom, ls.paddingLeft],
        contain: cs.contain,
        anchorSupported: CSS.supports("overflow-anchor", "none"),
        overflowAnchor: cs.getPropertyValue("overflow-anchor"),
        firstChildMargin: getComputedStyle(el.firstElementChild!).marginTop,
      };
    });
    expect(frame.parentId).toBe("lane-frame");
    expect(frame.isArticle).toBe(true);
    expect(frame.padding).toEqual(["0px", "0px", "0px", "0px"]);
    expect(frame.lanePadding).toEqual(["40px", "40px", "40px", "40px"]);
    // Paint containment would clip heading anchors, so it waits for review mode
    expect(frame.contain).toBe("none");
    if (frame.anchorSupported) expect(frame.overflowAnchor).toBe("none");
    expect(frame.firstChildMargin).toBe("0px");

    await page.setViewportSize({ width: 700, height: 800 });
    const narrow = await page.evaluate(() => {
      const ls = getComputedStyle(document.getElementById("lane-frame")!);
      const cs = getComputedStyle(document.getElementById("content")!);
      return [ls.paddingTop, ls.paddingLeft, cs.paddingTop, cs.paddingLeft];
    });
    expect(narrow).toEqual(["16px", "16px", "0px", "0px"]);
  });

  test("the frame paints the doc background in both variants", async ({ page }) => {
    await page.goto("/");
    for (const variant of ["light", "dark"]) {
      const got = await page.evaluate((v) => {
        document.documentElement.setAttribute("data-theme", v);
        const content = document.getElementById("content")!;
        content.setAttribute("data-theme", v);
        // The doc's own colour, resolved where the doc's variables apply
        const probe = document.createElement("div");
        probe.style.backgroundColor = "var(--bgColor-default)";
        content.appendChild(probe);
        const doc = getComputedStyle(probe).backgroundColor;
        probe.remove();
        return { doc, frame: getComputedStyle(document.getElementById("lane-frame")!).backgroundColor };
      }, variant);
      expect(got.frame, variant).toBe(got.doc);
    }
  });

  test("the file header sticks, opaque, under the page header", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 400 });
    await page.goto("/");
    await page.evaluate(() => window.scrollTo(0, 600));
    await nextFrames(page);
    const got = await page.evaluate(() => {
      const fh = document.querySelector(".file-header")!;
      const cs = getComputedStyle(fh);
      return {
        scrollY: window.scrollY,
        position: cs.position,
        zIndex: cs.zIndex,
        background: cs.backgroundColor,
        top: fh.getBoundingClientRect().top,
        headerBottom: document.querySelector(".header")!.getBoundingClientRect().bottom,
      };
    });
    expect(got.scrollY).toBeGreaterThan(100);
    expect(got.position).toBe("sticky");
    expect(got.zIndex).toBe("50");
    expect(got.background).not.toMatch(/rgba\(.*,\s*0(\.\d+)?\)$|transparent/);
    expect(Math.abs(got.top - got.headerBottom)).toBeLessThanOrEqual(1);
  });
});

test.describe("block patcher", () => {
  let server: ChildProcess;
  let live: WebSocket;
  let dir: string;
  let base: string;
  const doc = (rel: string) => path.join(dir, rel);

  test.beforeAll(async ({}, testInfo) => {
    const port = PORTS[testInfo.project.name];
    base = `http://127.0.0.1:${port}`;
    // realpath: the watcher compares canonical paths and macOS tmpdir is a symlink
    dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-patch-")));
    // Each test owns its docs, so none depends on another's edits
    fs.writeFileSync(doc("index.md"), "# Index\n");
    for (const name of [
      "ident", "ident-mermaid", "ident-theme", "ident-theme-other", "ident-pre", "ident-sp", "ident-ime",
      "ident-math", "ident-math2", "ident-attr", "ident-same", "ident-lead",
    ]) {
      fs.writeFileSync(doc(`${name}.md`), IDENT);
    }
    fs.writeFileSync(doc("ident-before-title.md"), "Lead.\n\n" + IDENT);
    for (const name of ["tasks-new", "tasks-attr"]) fs.writeFileSync(doc(`${name}.md`), TASKS);
    for (const name of ["anchor-below", "anchor-self", "anchor-sync", "anchor-key", "raw"]) {
      fs.writeFileSync(doc(`${name}.md`), longDoc(name));
    }
    fs.writeFileSync(doc("anchor-foot.md"), footnoteDoc("anchor-foot"));
    fs.writeFileSync(doc("ident-insert.md"), insertAfterTitle(IDENT, ["Inserted one.", "Inserted two."]));
    fs.writeFileSync(doc("refetch.md"), REFETCH);
    fs.writeFileSync(doc("tasks.md"), TASKS);
    for (const name of ["anchor", "anchor-top", "anchor-image", "anchor-scrolled-wheel", "anchor-scrolled-key", "anchor-mermaid"]) {
      fs.writeFileSync(doc(`${name}.md`), longDoc(name));
    }
    // A styled syntax theme puts attributes on <pre> in no fixed order
    server = await startBirta(port, doc("index.md"), ["--theme", "dracula"]);
    live = keepalive(port);
  });

  test.afterAll(() => {
    live?.close();
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("an edit replaces only the changed block and keeps a selection", async ({ page }) => {
    test.setTimeout(60_000);
    const ws = await openMocked(page, base, "ident.md");
    // KaTeX and mermaid have mutated their blocks by now
    await expect(page.locator("#content .katex")).toHaveCount(1, { timeout: CDN_TIMEOUT });
    await expect(page.locator("#content pre.mermaid svg")).toHaveCount(1, { timeout: CDN_TIMEOUT });
    await stashBlocks(page);
    await page.evaluate(() => {
      const p = Array.from(document.getElementById("content")!.children).find((e) =>
        (e.textContent || "").startsWith("Omega"),
      )!;
      const range = document.createRange();
      range.setStart(p.firstChild!, 0);
      range.setEnd(p.firstChild!, 5);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(range);
      (window as any).__selected = p.firstChild;
    });
    const gamma = await blockIndex(page, "Gamma");

    const html = (await render(page, "ident.md")).replace("Gamma paragraph.", "Gamma paragraph, edited.");
    ws.send(content("ident.md", html));
    await expect(page.locator("#content")).toContainText("Gamma paragraph, edited.");

    expect(await replacedBlocks(page)).toEqual([gamma]);
    const selection = await page.evaluate(() => ({
      text: getSelection()!.toString(),
      same: getSelection()!.anchorNode === (window as any).__selected,
      connected: getSelection()!.anchorNode?.isConnected,
    }));
    expect(selection).toEqual({ text: "Omega", same: true, connected: true });
  });

  test("text that looks like a sourcepos attribute still counts as a change", async ({ page }) => {
    const ws = await openMocked(page, base, "ident.md");
    const html = (await render(page, "ident.md")).replace("1:1-1:5", "2:2-2:9");
    ws.send(content("ident.md", html));
    await expect(page.locator("#content code").first()).toHaveText('data-sourcepos="2:2-2:9"');
  });

  test("a diagram is not re-rendered when another block changes", async ({ page }) => {
    test.setTimeout(60_000);
    const ws = await openMocked(page, base, "ident-mermaid.md");
    await expect(page.locator("#content pre.mermaid svg")).toHaveCount(1, { timeout: CDN_TIMEOUT });
    await page.evaluate(() => {
      (window as any).__svg = document.querySelector("#content pre.mermaid svg");
    });

    const html = (await render(page, "ident-mermaid.md")).replace("Gamma paragraph.", "Gamma edited.");
    ws.send(content("ident-mermaid.md", html));
    await expect(page.locator("#content")).toContainText("Gamma edited.");
    // mermaid.run is async, so a re-render would land after the patch
    await page.waitForTimeout(500);

    expect(
      await page.evaluate(() => {
        const svg = (window as any).__svg as Element;
        return svg.isConnected && document.querySelector("#content pre.mermaid svg") === svg;
      }),
    ).toBe(true);
  });

  test("a variant change re-themes the diagram", async ({ page }) => {
    test.setTimeout(60_000);
    const ws = await openMocked(page, base, "ident-theme.md");
    await expect(page.locator("#content pre.mermaid svg")).toHaveCount(1, { timeout: CDN_TIMEOUT });
    const fill = () =>
      page.evaluate(() => {
        const shape = document.querySelector("#content pre.mermaid svg .node rect, #content pre.mermaid svg .node polygon");
        return shape ? getComputedStyle(shape).fill : null;
      });
    const before = await fill();
    expect(before).not.toBeNull();
    const variant = await page.evaluate(() => document.documentElement.getAttribute("data-theme"));
    const other = variant === "dark" ? "light" : "dark";

    // The html is the same in both variants; only mermaid's own theme differs
    ws.send(
      JSON.stringify({
        type: "theme_update",
        css_vars: "",
        theme_name: "dracula",
        theme_attr: "dracula",
        variants: ["light", "dark"],
        active_variant: other,
        has_toggle: true,
        path: "ident-theme.md",
        rev: randomRev(),
        html: await render(page, "ident-theme.md"),
      }),
    );
    await expect(page.locator("html")).toHaveAttribute("data-theme", other);
    await expect.poll(fill, { timeout: 10_000 }).not.toBe(before);
  });

  test("a variant change re-themes the diagram on a page that refetches", async ({ page }) => {
    test.setTimeout(60_000);
    const ws = await openMocked(page, base, "ident-theme-other.md");
    await expect(page.locator("#content pre.mermaid svg")).toHaveCount(1, { timeout: CDN_TIMEOUT });
    const fill = () =>
      page.evaluate(() => {
        const shape = document.querySelector("#content pre.mermaid svg .node rect, #content pre.mermaid svg .node polygon");
        return shape ? getComputedStyle(shape).fill : null;
      });
    const before = await fill();
    expect(before).not.toBeNull();
    const variant = await page.evaluate(() => document.documentElement.getAttribute("data-theme"));
    const other = variant === "dark" ? "light" : "dark";

    // Another doc's update: the page refetches its own, unchanged, render
    ws.send(
      JSON.stringify({
        type: "theme_update",
        css_vars: "",
        theme_name: "dracula",
        theme_attr: "dracula",
        variants: ["light", "dark"],
        active_variant: other,
        has_toggle: true,
        path: "index.md",
        rev: randomRev(),
        html: "<h1>Index</h1>",
      }),
    );
    await expect(page.locator("html")).toHaveAttribute("data-theme", other);
    await expect.poll(fill, { timeout: 10_000 }).not.toBe(before);
  });

  test("a code block keeps its node across repeated edits elsewhere", async ({ page }) => {
    const ws = await openMocked(page, base, "ident-pre.md");
    await page.evaluate(() => {
      (window as any).__pre = document.querySelector("#content > pre:not(.mermaid)");
    });
    expect(await page.evaluate(() => (window as any).__pre !== null)).toBe(true);

    // Each fresh render may order the <pre> attributes differently
    for (let i = 1; i <= 10; i++) {
      const html = (await render(page, "ident-pre.md")).replace("Alpha paragraph.", `Alpha paragraph ${i}.`);
      ws.send(content("ident-pre.md", html));
      await expect(page.locator("#content")).toContainText(`Alpha paragraph ${i}.`);
      expect(
        await page.evaluate(() => (window as any).__pre.isConnected),
        `code block replaced on edit ${i}`,
      ).toBe(true);
    }
  });

  test("sourcepos follows an insert above", async ({ page }) => {
    const ws = await openMocked(page, base, "ident-sp.md");
    await page.evaluate(() => {
      (window as any).__omega = Array.from(document.getElementById("content")!.children).find((e) =>
        (e.textContent || "").startsWith("Omega"),
      );
    });

    const html = await render(page, "ident-insert.md");
    ws.send(content("ident-sp.md", html));
    await expect(page.locator("#content")).toContainText("Inserted two.");

    const got = await page.evaluate((fresh) => {
      const list = (root: ParentNode) =>
        Array.from(root.querySelectorAll("[data-sourcepos]"), (el) => `${el.tagName}@${el.getAttribute("data-sourcepos")}`);
      const parsed = new DOMParser().parseFromString(fresh, "text/html").body;
      return {
        kept: (window as any).__omega.isConnected,
        live: list(document.getElementById("content")!),
        fresh: list(parsed),
      };
    }, html);
    expect(got.kept).toBe(true);
    expect(got.live).toEqual(got.fresh);
  });

  test("an edit during IME composition waits for it to end", async ({ page }) => {
    const ws = await openMocked(page, base, "ident-ime.md");
    const html = await render(page, "ident-ime.md");
    await page.evaluate(() =>
      document.body.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true })),
    );
    ws.send(content("ident-ime.md", html.replace("Gamma paragraph.", "Gamma first.")));
    ws.send(content("ident-ime.md", html.replace("Gamma paragraph.", "Gamma latest.")));
    await page.waitForTimeout(300);
    await expect(page.locator("#content")).toContainText("Gamma paragraph.");

    await page.evaluate(() =>
      document.body.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "x" })),
    );
    await expect(page.locator("#content")).toContainText("Gamma latest.");
    await expect(page.locator("#content")).not.toContainText("Gamma first.");
  });

  test("a theme update for this doc patches instead of replacing", async ({ page }) => {
    const ws = await openMocked(page, base, "ident.md");
    await stashBlocks(page);
    const gamma = await blockIndex(page, "Gamma");
    const html = (await render(page, "ident.md")).replace("Gamma paragraph.", "Gamma restyled.");
    ws.send(
      JSON.stringify({
        type: "theme_update",
        css_vars: "",
        theme_name: "dracula",
        theme_attr: "dracula",
        variants: ["dark"],
        active_variant: "dark",
        has_toggle: false,
        path: "ident.md",
        rev: randomRev(),
        html,
      }),
    );
    await expect(page.locator("#content")).toContainText("Gamma restyled.");
    expect(await replacedBlocks(page)).toEqual([gamma]);
  });

  test("a theme update for another doc refetches and patches", async ({ page }) => {
    const ws = await openMocked(page, base, "refetch.md");
    await stashBlocks(page);
    const gamma = await blockIndex(page, "Gamma");
    fs.writeFileSync(doc("refetch.md"), REFETCH.replace("Gamma paragraph.", "Gamma refetched."));
    ws.send(
      JSON.stringify({
        type: "theme_update",
        css_vars: "",
        theme_name: "dracula",
        theme_attr: "dracula",
        variants: ["dark"],
        active_variant: "dark",
        has_toggle: false,
        path: "index.md",
        rev: randomRev(),
        html: "<h1>Index</h1>",
      }),
    );
    await expect(page.locator("#content")).toContainText("Gamma refetched.");
    expect(await replacedBlocks(page)).toEqual([gamma]);
  });

  test("a checkbox still writes after a patch shifts its line", async ({ page }) => {
    const ticks: { line?: number }[] = [];
    page.on("websocket", (ws) =>
      ws.on("framesent", ({ payload }) => {
        const msg = JSON.parse(payload.toString());
        if (msg.type === "checkbox") ticks.push(msg);
      }),
    );
    await openLive(page, base, "tasks.md");
    await page.evaluate(() => {
      (window as any).__list = document.querySelector("#content > ul");
    });

    fs.writeFileSync(doc("tasks.md"), insertAfterTitle(TASKS, ["New intro line."]));
    await expect(page.locator("#content")).toContainText("New intro line.");
    expect(await page.evaluate(() => (window as any).__list.isConnected)).toBe(true);

    await page.locator('#content input[type="checkbox"]').nth(1).check();
    const after = "# Tasks\n\nNew intro line.\n\nIntro.\n\n- [ ] one\n- [x] two\n\nEnd.\n";
    await expect.poll(() => fs.readFileSync(doc("tasks.md"), "utf-8")).toBe(after);
    await expect(page.locator("#status")).not.toHaveText("changed since · try again");
    // A second listener on a kept box would send the tick twice
    expect(ticks.map((t) => t.line)).toEqual([8]);
  });

  test("a live edit above keeps the first visible block in place", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    await openLive(page, base, "anchor.md");
    await scrollHalfway(page);
    const before = await visibleBlocks(page);
    const original = fs.readFileSync(doc("anchor.md"), "utf-8");

    const grown = insertAfterTitle(original, INSERTED).replace(
      "Block 3. ",
      "Block 3, now much longer. " + "extra words ".repeat(40),
    );
    fs.writeFileSync(doc("anchor.md"), grown);
    await expect(page.locator("#content")).toContainText("Inserted 6.");
    await nextFrames(page);
    await expectInPlace(page, before);

    fs.writeFileSync(doc("anchor.md"), original);
    await expect(page.locator("#content")).not.toContainText("Inserted 1.");
    await nextFrames(page);
    await expectInPlace(page, before);
  });

  test("at the top of the page an insert shows the new top", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    await openLive(page, base, "anchor-top.md");
    expect(await page.evaluate(() => window.scrollY)).toBe(0);

    const original = fs.readFileSync(doc("anchor-top.md"), "utf-8");
    fs.writeFileSync(doc("anchor-top.md"), insertAfterTitle(original, INSERTED));
    await expect(page.locator("#content")).toContainText("Inserted 6.");
    await nextFrames(page);
    expect(await page.evaluate(() => window.scrollY)).toBe(0);
    await expect(page.locator("#content p", { hasText: "Inserted 1." })).toBeInViewport();
  });

  test("an image that loads late above the viewport does not move the view", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    await page.route("**/local/late.svg", async (route) => {
      await gate;
      await route.fulfill({ contentType: "image/svg+xml", body: TALL_SVG });
    });
    await openLive(page, base, "anchor-image.md");
    await scrollHalfway(page);
    const before = await visibleBlocks(page);

    const original = fs.readFileSync(doc("anchor-image.md"), "utf-8");
    fs.writeFileSync(doc("anchor-image.md"), insertAfterTitle(original, ["![late](late.svg)", ...INSERTED]));
    await expect(page.locator("#content")).toContainText("Inserted 6.");
    await nextFrames(page);
    await expectInPlace(page, before);

    release();
    await expect
      .poll(() =>
        page.evaluate(() => {
          const img = document.querySelector<HTMLImageElement>('#content img[alt="late"]');
          return !!img && img.complete && img.naturalHeight > 0;
        }),
      )
      .toBe(true);
    await nextFrames(page);
    await expectInPlace(page, before);
  });

  for (const gesture of ["wheel", "key"] as const) {
  test(`after the user scrolls (${gesture}), late growth is left alone`, async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    await page.route("**/local/late.svg", async (route) => {
      await gate;
      await route.fulfill({ contentType: "image/svg+xml", body: TALL_SVG });
    });
    await openLive(page, base, `anchor-scrolled-${gesture}.md`);
    await scrollHalfway(page);

    const original = fs.readFileSync(doc(`anchor-scrolled-${gesture}.md`), "utf-8");
    fs.writeFileSync(doc(`anchor-scrolled-${gesture}.md`), insertAfterTitle(original, ["![late](late.svg)", ...INSERTED]));
    await expect(page.locator("#content")).toContainText("Inserted 6.");
    await nextFrames(page);

    const patched = await page.evaluate(() => window.scrollY);
    if (gesture === "wheel") {
      await page.mouse.move(500, 400);
      await page.mouse.wheel(0, 120);
    } else {
      await page.keyboard.press("PageDown");
    }
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(patched + 50);
    const scrolled = await settledScroll(page);

    release();
    await expect
      .poll(() =>
        page.evaluate(() => {
          const img = document.querySelector<HTMLImageElement>('#content img[alt="late"]');
          return !!img && img.complete && img.naturalHeight > 0;
        }),
      )
      .toBe(true);
    await nextFrames(page);
    await page.waitForTimeout(200);
    expect(Math.abs((await page.evaluate(() => window.scrollY)) - scrolled)).toBeLessThanOrEqual(1);
  });
  }

  test("a diagram rendered above the viewport does not move the view", async ({ page }) => {
    test.setTimeout(60_000);
    await page.setViewportSize({ width: 1024, height: 768 });
    await openLive(page, base, "anchor-mermaid.md");
    await page.waitForFunction(() => typeof (window as any).mermaid !== "undefined", null, {
      timeout: CDN_TIMEOUT,
    });
    await scrollHalfway(page);
    const before = await visibleBlocks(page);

    const original = fs.readFileSync(doc("anchor-mermaid.md"), "utf-8");
    const diagram = "```mermaid\ngraph TD\n  A-->B\n  B-->C\n  C-->D\n  D-->E\n```";
    fs.writeFileSync(doc("anchor-mermaid.md"), insertAfterTitle(original, [diagram, ...INSERTED]));
    await expect(page.locator("#content pre.mermaid svg")).toHaveCount(1, { timeout: 10_000 });
    await nextFrames(page);
    await expectInPlace(page, before);
  });

  test("math in a patched block is typeset", async ({ page }) => {
    test.setTimeout(60_000);
    const ws = await openMocked(page, base, "ident-math.md");
    await expect(page.locator("#content .katex")).toHaveCount(1, { timeout: CDN_TIMEOUT });
    const html = (await render(page, "ident-math.md")).replace(">x^2<", ">y^2<");
    expect(html).toContain(">y^2<");
    ws.send(content("ident-math.md", html));
    await expect(page.locator("#content .katex annotation")).toHaveText("y^2");
    await expect(page.locator("#content .katex")).toHaveCount(1);
    await expect(page.locator("#content [data-math-style]:not([data-math-rendered])")).toHaveCount(0);
  });

  test("a patched-in math block is kept on the next edit", async ({ page }) => {
    test.setTimeout(60_000);
    const ws = await openMocked(page, base, "ident-math2.md");
    await expect(page.locator("#content .katex")).toHaveCount(1, { timeout: CDN_TIMEOUT });
    const html = (await render(page, "ident-math2.md")).replace(">x^2<", ">y^2<");
    ws.send(content("ident-math2.md", html));
    await expect(page.locator("#content .katex annotation")).toHaveText("y^2");
    await stashBlocks(page);
    await page.evaluate(() => {
      (window as any).__katex = document.querySelector("#content .katex");
    });
    const omega = await blockIndex(page, "Omega");

    ws.send(content("ident-math2.md", html.replace("Omega paragraph.", "Omega edited.")));
    await expect(page.locator("#content")).toContainText("Omega edited.");
    expect(await replacedBlocks(page)).toEqual([omega]);
    expect(
      await page.evaluate(() => (window as any).__katex === document.querySelector("#content .katex")),
    ).toBe(true);
  });

  test("an attribute-only change replaces the block", async ({ page }) => {
    const ws = await openMocked(page, base, "ident-attr.md");
    const html = (await render(page, "ident-attr.md")).replace("https://a.example/", "https://b.example/");
    ws.send(content("ident-attr.md", html));
    await expect(page.locator('#content a[href^="https://"]')).toHaveAttribute("href", "https://b.example/");
  });

  test("a ticked box on disk shows ticked", async ({ page }) => {
    await openLive(page, base, "tasks-attr.md");
    fs.writeFileSync(doc("tasks-attr.md"), TASKS.replace("- [ ] one", "- [x] one"));
    await expect(page.locator('#content input[type="checkbox"]').first()).toBeChecked();
    await expect(page.locator('#content input[type="checkbox"]').nth(1)).not.toBeChecked();
  });

  test("a checkbox in a new list writes", async ({ page }) => {
    await openLive(page, base, "tasks-new.md");
    const grown = TASKS.replace("- [ ] two\n", "- [ ] two\n- [ ] three\n");
    fs.writeFileSync(doc("tasks-new.md"), grown);
    const three = page.locator('#content input[type="checkbox"]').nth(2);
    await expect(three).toBeEnabled();
    await three.check();
    await expect
      .poll(() => fs.readFileSync(doc("tasks-new.md"), "utf-8"))
      .toBe(grown.replace("- [ ] three", "- [x] three"));
  });

  test("a same-variant theme update keeps the diagram", async ({ page }) => {
    test.setTimeout(60_000);
    const ws = await openMocked(page, base, "ident-same.md");
    await expect(page.locator("#content pre.mermaid svg")).toHaveCount(1, { timeout: CDN_TIMEOUT });
    await page.evaluate(() => {
      (window as any).__svg = document.querySelector("#content pre.mermaid svg");
    });
    const variant = await page.evaluate(() => document.documentElement.getAttribute("data-theme"));
    const html = (await render(page, "ident-same.md")).replace("Gamma paragraph.", "Gamma restyled.");
    ws.send(
      JSON.stringify({
        type: "theme_update",
        css_vars: "",
        theme_name: "dracula",
        theme_attr: "dracula",
        variants: [variant],
        active_variant: variant,
        has_toggle: false,
        path: "ident-same.md",
        rev: randomRev(),
        html,
      }),
    );
    await expect(page.locator("#content")).toContainText("Gamma restyled.");
    await page.waitForTimeout(500);
    expect(
      await page.evaluate(() => {
        const svg = (window as any).__svg as Element;
        return svg.isConnected && document.querySelector("#content pre.mermaid svg") === svg;
      }),
    ).toBe(true);
  });

  test("a block before the title", async ({ page }) => {
    const ws = await openMocked(page, base, "ident-lead.md");
    await page.evaluate(() => {
      (window as any).__title = document.querySelector("#content > h1");
    });
    const html = await render(page, "ident-before-title.md");
    ws.send(content("ident-lead.md", html));
    await expect(page.locator("#content > *").first()).toHaveText("Lead.");

    const got = await page.evaluate((fresh) => {
      const list = (root: ParentNode) =>
        Array.from(root.querySelectorAll("[data-sourcepos]"), (el) => `${el.tagName}@${el.getAttribute("data-sourcepos")}`);
      const parsed = new DOMParser().parseFromString(fresh, "text/html").body;
      return {
        kept: (window as any).__title.isConnected,
        live: list(document.getElementById("content")!),
        fresh: list(parsed),
      };
    }, html);
    expect(got.kept).toBe(true);
    expect(got.live).toEqual(got.fresh);
  });

  test("growth below the viewport does not scroll", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    await openLive(page, base, "anchor-below.md");
    await scrollHalfway(page);
    const before = await visibleBlocks(page);
    const original = fs.readFileSync(doc("anchor-below.md"), "utf-8");
    fs.writeFileSync(
      doc("anchor-below.md"),
      original.replace("Block 190. ", "Block 190, grown. " + "more words ".repeat(40)),
    );
    await expect(page.locator("#content")).toContainText("Block 190, grown.");
    await nextFrames(page);
    await expectInPlace(page, before);
  });

  test("a footnote added above keeps the view", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    await openLive(page, base, "anchor-foot.md");
    await scrollHalfway(page);
    const before = await visibleBlocks(page);
    await page.evaluate(() => {
      (window as any).__first = Array.from(document.getElementById("content")!.children).find(
        (el) => el.getBoundingClientRect().top >= 0,
      );
    });
    const original = fs.readFileSync(doc("anchor-foot.md"), "utf-8");
    fs.writeFileSync(
      doc("anchor-foot.md"),
      insertAfterTitle(original, INSERTED).replace("Block 3. ", "Block 3[^2]. ") + "[^2]: Note two.\n",
    );
    await expect(page.locator("#content")).toContainText("Note two.");
    await nextFrames(page);
    // The last block changed too, so everything after the title is new nodes
    expect(await page.evaluate(() => (window as any).__first.isConnected)).toBe(false);
    await expectInPlace(page, before);
  });

  test("editing the first visible block itself keeps the view", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    await openLive(page, base, "anchor-self.md");
    await scrollHalfway(page);
    const [first] = await visibleBlocks(page);
    const label = first.text.slice(0, first.text.indexOf("."));
    expect(label).toMatch(/^Block \d+$/);
    await page.evaluate((l) => {
      const kids = Array.from(document.getElementById("content")!.children);
      const i = kids.findIndex((el) => (el.textContent || "").startsWith(l + "."));
      (window as any).__edited = kids[i];
      (window as any).__next = kids[i + 1];
      (window as any).__tops = [kids[i].getBoundingClientRect().top, kids[i + 1].getBoundingClientRect().top];
    }, label);

    const original = fs.readFileSync(doc("anchor-self.md"), "utf-8");
    // Same length, so the edited block keeps its height
    const edited = insertAfterTitle(original, INSERTED).replace(`${label}. `, `${label}! `);
    fs.writeFileSync(doc("anchor-self.md"), edited);
    await expect(page.locator("#content")).toContainText(`${label}!`);
    await nextFrames(page);
    const got = await page.evaluate((l) => {
      const kids = Array.from(document.getElementById("content")!.children);
      const edited = kids.find((el) => (el.textContent || "").startsWith(l + "!"))!;
      return {
        tops: (window as any).__tops as number[],
        edited: edited.getBoundingClientRect().top,
        nextKept: (window as any).__next.isConnected,
        next: (window as any).__next.getBoundingClientRect().top,
      };
    }, label);
    expect(got.nextKept).toBe(true);
    expect(Math.abs(got.edited - got.tops[0])).toBeLessThanOrEqual(1);
    expect(Math.abs(got.next - got.tops[1])).toBeLessThanOrEqual(1);
  });

  test("the correction lands in the task that patches", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    await openLive(page, base, "anchor-sync.md");
    await scrollHalfway(page);
    const [first] = await visibleBlocks(page);
    await page.evaluate((text) => {
      const el = Array.from(document.getElementById("content")!.children).find(
        (e) => (e.textContent || "").trim() === text,
      )!;
      const w = window as any;
      w.__seen = null;
      // Runs at the first microtask checkpoint after the mutation, before any paint
      new MutationObserver((_, obs) => {
        w.__seen = el.isConnected ? el.getBoundingClientRect().top : "detached";
        obs.disconnect();
      }).observe(document.getElementById("content")!, { childList: true, subtree: true });
    }, first.text);

    const original = fs.readFileSync(doc("anchor-sync.md"), "utf-8");
    fs.writeFileSync(doc("anchor-sync.md"), insertAfterTitle(original, INSERTED));
    await expect(page.locator("#content")).toContainText("Inserted 6.");
    const seen = await page.evaluate(() => (window as any).__seen);
    expect(typeof seen, `anchor block was ${seen}`).toBe("number");
    expect(Math.abs(seen - first.top)).toBeLessThanOrEqual(1);
  });

  test("the raw view follows edits and preview shows them after", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.setViewportSize({ width: 1024, height: 768 });
    await openLive(page, base, "raw.md");
    await page.locator('#view-toggle .view-btn[data-view="raw"]').click();
    await expect(page.locator("#source-view")).toBeVisible();
    await scrollHalfway(page);

    const original = fs.readFileSync(doc("raw.md"), "utf-8");
    fs.writeFileSync(doc("raw.md"), insertAfterTitle(original, ["Raw edited."]));
    await expect(page.locator("#source-view")).toContainText("Raw edited.");
    await page.locator('#view-toggle .view-btn[data-view="preview"]').click();
    await expect(page.locator("#content p", { hasText: "Raw edited." })).toHaveCount(1);
    expect(errors).toEqual([]);
  });
});

test.describe("hidden page header", () => {
  let server: ChildProcess;
  let dir: string;
  let base: string;

  test.beforeAll(async ({}, testInfo) => {
    const port = PORTS[testInfo.project.name] + 10;
    base = `http://127.0.0.1:${port}`;
    dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-patch-nh-")));
    fs.writeFileSync(path.join(dir, "long.md"), longDoc("long"));
    server = await startBirta(port, path.join(dir, "long.md"), ["--no-header"]);
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("the file header sticks at the very top", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 600 });
    await page.goto(`${base}/view/long.md`);
    await expect(page.locator(".header")).toBeHidden();
    await page.evaluate(() => window.scrollTo(0, 2000));
    await nextFrames(page);
    const top = await page.evaluate(() => document.querySelector(".file-header")!.getBoundingClientRect().top);
    expect(Math.abs(top)).toBeLessThanOrEqual(1);
  });
});

// A theme whose doc background differs from its page background, so a frame painted from the wrong one shows
test.describe("custom theme frame", () => {
  let server: ChildProcess;
  let dir: string;
  let base: string;

  test.beforeAll(async ({}, testInfo) => {
    const port = PORTS[testInfo.project.name] + 20;
    base = `http://127.0.0.1:${port}`;
    dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-patch-theme-")));
    const theme = fs
      .readFileSync(path.join(REPO, "assets", "themes", "dracula.toml"), "utf-8")
      .replace('name = "dracula"', 'name = "frame-test"')
      .replace('bg_primary = "#282a36"', 'bg_primary = "#123456"')
      .replace("./syntax/dracula.tmTheme", path.join(REPO, "assets", "themes", "syntax", "dracula.tmTheme"));
    expect(theme).toContain('bg_primary = "#123456"');
    fs.writeFileSync(path.join(dir, "frame-test.toml"), theme);
    fs.writeFileSync(path.join(dir, "doc.md"), "# Doc\n\nText.\n");
    server = await startBirta(port, path.join(dir, "doc.md"), ["--theme", path.join(dir, "frame-test.toml")]);
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("the frame paints the doc background, not the page background", async ({ page }) => {
    await page.goto(`${base}/view/doc.md`);
    const got = await page.evaluate(() => {
      const probe = document.createElement("div");
      probe.style.backgroundColor = "var(--bgColor-default)";
      document.getElementById("content")!.appendChild(probe);
      const doc = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return {
        doc,
        page: getComputedStyle(document.body).backgroundColor,
        frame: getComputedStyle(document.getElementById("lane-frame")!).backgroundColor,
      };
    });
    expect(got.doc).toBe("rgb(18, 52, 86)");
    expect(got.page).not.toBe(got.doc);
    expect(got.frame).toBe(got.doc);
  });
});
