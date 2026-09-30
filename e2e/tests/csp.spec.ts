import { test, expect, Page } from "@playwright/test";
import { spawn, execFileSync, ChildProcess } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { pathToFileURL } from "url";

// One server per project, since projects run this file in parallel
const PORTS: Record<string, number> = { chromium: 3464, webkit: 3465, firefox: 3466 };
const REPO = path.resolve(__dirname, "..", "..");
// mermaid.min.js is about 3.5 MB from the CDN
const CDN_TIMEOUT = 30_000;

// Each line tries to run script without the page nonce
const HOSTILE = `# Hostile

<script>window.pwned = 1</script>

<img id="bad-img" src="missing.png" onerror="window.pwned = 2">

<a id="js-link" href="javascript:window.pwned=3">run</a>

<script nonce="{{NONCE}}">window.pwned = 6</script>

<iframe class="inj-frame" srcdoc="<script>parent.pwned = 5</script>"></iframe>
`;

// CSP can't stop a meta refresh, so the renderer defuses the attribute
const REFRESH_TAG = `<meta http-equiv="refresh" content="0;url=about:blank">`;
const REFRESH = `# Refresh\n\n${REFRESH_TAG}\n`;

// Each hides the refresh from a renderer that misreads comments, raw text or CDATA
const HIDDEN_REFRESH = [
  `<style><!--</style>${REFRESH_TAG}-->`,
  `<title><!--</title>${REFRESH_TAG}`,
  `<script><!--</script>${REFRESH_TAG}-->`,
  `<noscript><!--</noscript>${REFRESH_TAG}-->`,
  `x <xmp> <!--</xmp>${REFRESH_TAG}-->`,
  `<iframe>\n\n<!--</iframe>${REFRESH_TAG}-->`,
  `> <!--\n\n<a title='-->${REFRESH_TAG}'>x</a>`,
  `<svg>\n\n<![CDATA[><!--]]>${REFRESH_TAG}-->`,
  `<svg><title>${REFRESH_TAG}</title></svg>`,
  `<!-- --!>${REFRESH_TAG}-->`,
].map((md) => `# Refresh\n\n${md}\n`);

// Inserted by live reload or a /render refetch, where innerHTML still runs handlers and srcdoc
const INJECTED = `# Reloaded

<img class="inj-img" src="missing-reload.png" onerror="window.pwned = 4">

<iframe class="inj-frame" srcdoc="<script>parent.pwned = 5</script>"></iframe>
`;

const FEATURES = `# Features

Inline $x^2 + y^2$ math.

$$
\\int_0^1 x\\,dx
$$

\`\`\`mermaid
graph TD
  A[Start] --> B[End]
\`\`\`

- [ ] first
- [ ] second
`;

async function startBirta(port: number, file: string): Promise<ChildProcess> {
  const args = ["run", "--", "--no-open", "--port", String(port), file];
  const proc = spawn("cargo", args, { cwd: REPO, stdio: "pipe" });

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

function tempDir(): string {
  // realpath: the watcher compares canonical paths and macOS tmpdir is a symlink
  return fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-")));
}

/** Records CSP violations from console messages and `securitypolicyviolation` events. */
async function watchViolations(page: Page): Promise<() => Promise<string[]>> {
  const logged: string[] = [];
  page.on("console", (m) => {
    if (/Content[- ]Security[- ]Policy/i.test(m.text())) logged.push(m.text());
  });
  await page.addInitScript(() => {
    const w = window as unknown as { __csp: string[] };
    w.__csp = [];
    document.addEventListener("securitypolicyviolation", (e) =>
      w.__csp.push(`${e.violatedDirective} ${e.blockedURI}`),
    );
  });
  return async () => [
    ...logged,
    ...(await page.evaluate(() => (window as unknown as { __csp: string[] }).__csp)),
  ];
}

function pwned(page: Page): Promise<unknown> {
  return page.evaluate(() => (window as unknown as { pwned?: unknown }).pwned);
}

/** Waits until every doc image has settled and every srcdoc frame has loaded. */
async function settled(page: Page): Promise<void> {
  await expect
    .poll(() =>
      page.evaluate(() => {
        const imgs = Array.from(document.querySelectorAll("article img"), (i) => (i as HTMLImageElement).complete);
        const frames = Array.from(
          document.querySelectorAll("article iframe"),
          (f) => (f as HTMLIFrameElement).contentDocument?.readyState === "complete",
        );
        return [...imgs, ...frames].every(Boolean);
      }),
    )
    .toBe(true);
}

/** Loads the hostile doc and asserts none of its script ran, but CSP saw it. */
async function expectHostileInert(page: Page, url: string): Promise<void> {
  const violations = await watchViolations(page);
  await page.goto(url);
  await expect(page.locator("h1").first()).toHaveText("Hostile");
  await expect(page.locator("#bad-img")).toHaveCount(1);
  await settled(page);
  expect(await pwned(page)).toBeUndefined();
  await expect.poll(async () => (await violations()).length).toBeGreaterThan(0);

  await expect.poll(async () => (await violations()).length).toBeGreaterThan(0);

  const before = (await violations()).length;
  await page.locator("#js-link").click();
  await expect.poll(async () => (await violations()).length).toBeGreaterThan(before);
  expect(await pwned(page)).toBeUndefined();
}

/** A doc meta refresh would have navigated away within the wait. */
async function expectNoRefresh(page: Page, url: string): Promise<void> {
  await page.goto(url);
  await page.waitForTimeout(1000);
  expect(page.url()).toBe(url);
  await expect(page.locator("h1").first()).toHaveText("Refresh");
}

async function expectMermaidAndMath(page: Page): Promise<void> {
  await expect(page.locator("pre.mermaid svg")).toBeVisible({ timeout: CDN_TIMEOUT });
  await expect(page.locator('[data-math-style="inline"] .katex')).toHaveCount(1, {
    timeout: CDN_TIMEOUT,
  });
  await expect(page.locator('[data-math-style="display"] .katex')).toHaveCount(1);
}

async function toggleVariant(page: Page): Promise<void> {
  const html = page.locator("html");
  const initial = await html.getAttribute("data-theme");
  await page.locator("#theme-toggle").click();
  await expect(html).toHaveAttribute("data-theme", initial === "light" ? "dark" : "light");
}

test.describe("page CSP", () => {
  let server: ChildProcess;
  let dir: string;
  let BASE: string;

  test.beforeAll(async ({}, testInfo) => {
    const port = PORTS[testInfo.project.name];
    BASE = `http://127.0.0.1:${port}`;
    dir = tempDir();
    fs.writeFileSync(path.join(dir, "index.md"), "# Index\n\n[features](features.md)\n");
    fs.writeFileSync(path.join(dir, "hostile.md"), HOSTILE);
    fs.writeFileSync(path.join(dir, "features.md"), FEATURES);
    fs.writeFileSync(path.join(dir, "reload.md"), "# Reload\n");
    fs.writeFileSync(path.join(dir, "refresh.md"), REFRESH);
    HIDDEN_REFRESH.forEach((md, i) => fs.writeFileSync(path.join(dir, `refresh-${i}.md`), md));
    server = await startBirta(port, path.join(dir, "index.md"));
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("pages carry a nonce policy that every page script matches", async ({ page }) => {
    const res = await page.goto(`${BASE}/view/features.md`);
    const csp = res!.headers()["content-security-policy"] ?? "";
    expect(csp).toContain("'strict-dynamic'");
    const nonce = /'nonce-([^']+)'/.exec(csp)?.[1];
    expect(nonce).toBeTruthy();
    const nonces = await page.evaluate(() => Array.from(document.scripts, (s) => s.nonce));
    expect(nonces.length).toBeGreaterThanOrEqual(4);
    expect(nonces.every((n) => n === nonce)).toBe(true);
  });

  test("doc scripts, inline handlers and javascript: links do not run", async ({ page }) => {
    await expectHostileInert(page, `${BASE}/view/hostile.md`);
  });

  test("a doc meta refresh does not navigate", async ({ page }) => {
    await expectNoRefresh(page, `${BASE}/view/refresh.md`);
  });

  HIDDEN_REFRESH.forEach((md, i) => {
    test(`a meta refresh hidden from the renderer does not navigate: ${JSON.stringify(md)}`, async ({
      page,
    }) => {
      await expectNoRefresh(page, `${BASE}/view/refresh-${i}.md`);
    });
  });

  test("script delivered by live reload or a /render refetch does not run", async ({ page }) => {
    const file = path.join(dir, "reload.md");
    await page.goto(`${BASE}/view/reload.md`);
    await expect(page.locator("h1").first()).toHaveText("Reload");
    fs.writeFileSync(file, INJECTED);
    await expect(page.locator("h1").first()).toHaveText("Reloaded");
    await settled(page);
    expect(await pwned(page)).toBeUndefined();

    // reload.md is not the entry doc, so a theme swap re-fetches it through /render.
    // Marking the hostile blocks makes them differ, so the patcher inserts them afresh.
    await page.route(/\/render\/reload\.md$/, async (route) => {
      const res = await route.fetch();
      const body = (await res.text()).replaceAll('class="inj-', 'class="refetched inj-');
      await route.fulfill({ response: res, body });
    });
    await page.locator("#theme-dropdown-btn").click();
    await page.locator('.theme-dropdown-item[data-theme="gruvbox"]').click();
    await expect(page.locator("article .refetched")).toHaveCount(2);
    await expect(page.locator("h1").first()).toHaveText("Reloaded");
    await settled(page);
    expect(await pwned(page)).toBeUndefined();
  });

  test("mermaid, KaTeX, theme swap and checkbox work without violations", async ({ page }) => {
    test.slow();
    const file = path.join(dir, "features.md");
    fs.writeFileSync(file, FEATURES);
    const violations = await watchViolations(page);
    // Not the entry doc, so a theme swap re-fetches it through /render
    await page.goto(`${BASE}/view/features.md`);
    await expectMermaidAndMath(page);

    await toggleVariant(page);
    await expect(page.locator("pre.mermaid svg")).toBeVisible();

    // Marked so the refetched diagram and math differ, and the patcher draws them afresh
    await page.route(/\/render\/features\.md$/, async (route) => {
      const res = await route.fetch();
      const body = (await res.text())
        .replace('class="mermaid"', 'class="mermaid refetched"')
        .replaceAll("<span data-math-style", '<span class="refetched" data-math-style');
      await route.fulfill({ response: res, body });
    });
    await page.locator("#theme-dropdown-btn").click();
    await page.locator('.theme-dropdown-item[data-theme="catppuccin"]').click();
    await expect(page.locator('.theme-dropdown-item.active[data-theme="catppuccin"]')).toHaveCount(1);
    await expect(page.locator("pre.mermaid.refetched svg")).toBeVisible({ timeout: CDN_TIMEOUT });
    await expect(page.locator(".refetched[data-math-style] .katex")).toHaveCount(2);

    await page.locator('article.markdown-body input[type="checkbox"]').first().check();
    await expect.poll(() => fs.readFileSync(file, "utf-8")).toContain("- [x] first");

    expect(await violations()).toEqual([]);
  });
});

test("shipped fixtures load without violations", async ({ page }) => {
  const violations = await watchViolations(page);
  for (const doc of ["test.md", "theme-test.md", "checkbox.md", "simple.md", "directory/index.md"]) {
    await page.goto(`/view/${doc}`, { waitUntil: "load" });
    await expect(page.locator("article.markdown-body")).toBeVisible();
    // The event log resets on each navigation, so read it per page
    expect(await violations(), doc).toEqual([]);
  }
});

test.describe("static export CSP", () => {
  let dir: string;
  let out: string;

  test.beforeAll(() => {
    dir = tempDir();
    out = path.join(dir, "out");
    fs.writeFileSync(
      path.join(dir, "index.md"),
      FEATURES.replace(
        "# Features",
        "# Static\n\n[hostile](hostile.md) [refresh](refresh.md) " +
          HIDDEN_REFRESH.map((_, i) => `[r${i}](refresh-${i}.md)`).join(" "),
      ),
    );
    fs.writeFileSync(path.join(dir, "hostile.md"), HOSTILE);
    fs.writeFileSync(path.join(dir, "refresh.md"), REFRESH);
    HIDDEN_REFRESH.forEach((md, i) => fs.writeFileSync(path.join(dir, `refresh-${i}.md`), md));
    execFileSync(
      "cargo",
      ["run", "--", "--static", "--no-open", "--out", out, path.join(dir, "index.md")],
      { cwd: REPO, stdio: "pipe" },
    );
  });

  test.afterAll(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("static pages keep doc scripts inert", async ({ page }) => {
    await expectHostileInert(page, pathToFileURL(path.join(out, "hostile.html")).href);
  });

  test("a static doc meta refresh does not navigate", async ({ page }) => {
    await expectNoRefresh(page, pathToFileURL(path.join(out, "refresh.html")).href);
  });

  HIDDEN_REFRESH.forEach((md, i) => {
    test(`a static meta refresh hidden from the renderer does not navigate: ${JSON.stringify(md)}`, async ({
      page,
    }) => {
      await expectNoRefresh(page, pathToFileURL(path.join(out, `refresh-${i}.html`)).href);
    });
  });

  test("static pages render mermaid and KaTeX without violations", async ({ page }) => {
    test.slow();
    const violations = await watchViolations(page);
    await page.goto(pathToFileURL(path.join(out, "index.html")).href);
    await expectMermaidAndMath(page);
    await toggleVariant(page);
    await expect(page.locator("pre.mermaid svg")).toBeVisible();
    expect(await violations()).toEqual([]);
  });
});
