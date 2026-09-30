import { test, expect } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

const PORT = 3467;
const BASE = `http://127.0.0.1:${PORT}`;

async function startBirta(port: number, file: string): Promise<ChildProcess> {
  const cwd = path.resolve(__dirname, "..");
  const args = ["run", "--", "--no-open", "--port", String(port), file];
  const proc = spawn("cargo", args, { cwd, stdio: "pipe" });

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

// `<dir>/r` is served; `<dir>/vault` and `<dir>/later` are reached only through links.
test.describe("symlinked folders", () => {
  let server: ChildProcess;
  let dir: string;

  test.beforeAll(async () => {
    // Not realpath'd: on macOS the served path goes through the /var -> /private/var link
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-dir-"));
    fs.mkdirSync(path.join(dir, "r"));
    fs.mkdirSync(path.join(dir, "vault"));
    fs.mkdirSync(path.join(dir, "later"));
    fs.writeFileSync(
      path.join(dir, "r", "index.md"),
      "# Index\n\n[plan](.context/plan.md)\n"
    );
    fs.writeFileSync(path.join(dir, "vault", "plan.md"), "# Plan\n");
    fs.writeFileSync(path.join(dir, "later", "x.md"), "# Later\n");
    fs.symlinkSync("../vault", path.join(dir, "r", ".context"));
    server = await startBirta(PORT, path.join(dir, "r", "index.md"));
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("a doc in a symlinked folder live-reloads", async ({ page }) => {
    await page.goto(BASE);
    await page.locator('a:has-text("plan")').click();
    await page.waitForURL(`${BASE}/view/.context/plan.md`);
    await expect(page.locator("h1")).toHaveText("Plan");

    fs.writeFileSync(path.join(dir, "vault", "plan.md"), "# Plan edited\n");

    await expect(page.locator("h1")).toHaveText("Plan edited", {
      timeout: 10_000,
    });
  });

  test("a folder linked after start live-reloads", async ({ page }) => {
    fs.symlinkSync("../later", path.join(dir, "r", "later"));
    await page.goto(`${BASE}/view/later/x.md`);
    await expect(page.locator("h1")).toHaveText("Later");

    fs.writeFileSync(path.join(dir, "later", "x.md"), "# Later edited\n");

    await expect(page.locator("h1")).toHaveText("Later edited", {
      timeout: 10_000,
    });
  });
});

const DIR_PORT = 3468;
const DIR_BASE = `http://127.0.0.1:${DIR_PORT}`;

// `birta fixtures/dirmode`: the folder is served and `index.md` is the entry.
test.describe("serving a folder", () => {
  let server: ChildProcess;

  test.beforeAll(async () => {
    server = await startBirta(DIR_PORT, "fixtures/dirmode");
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
  });

  test("/ opens the folder's index", async ({ page }) => {
    await page.goto(DIR_BASE);
    await page.waitForURL(`${DIR_BASE}/view/index.md`);
    await expect(page.locator("h1")).toHaveText("Dirmode Index");
    const logo = page.locator('img[alt="logo"]');
    await expect(logo).toHaveAttribute("src", "/local/img/logo.png");
    await expect
      .poll(() => logo.evaluate((img: HTMLImageElement) => img.naturalWidth))
      .toBeGreaterThan(0);
  });

  test("nested pages link, load images and keep escaping links raw", async ({
    page,
  }) => {
    await page.goto(`${DIR_BASE}/view/index.md`);
    await page.locator('a:has-text("Sub Page")').click();
    await page.waitForURL(`${DIR_BASE}/view/sub/page.md`);
    await expect(page.locator("h1")).toHaveText("Sub Page");

    const logo = page.locator('img[alt="logo"]');
    await expect(logo).toHaveAttribute("src", "/local/img/logo.png");
    await expect
      .poll(() => logo.evaluate((img: HTMLImageElement) => img.naturalWidth))
      .toBeGreaterThan(0);
    await expect(page.locator('a:has-text("Escape")')).toHaveAttribute(
      "href",
      "../../outside.md"
    );

    await page.locator('a:has-text("Index")').click();
    await page.waitForURL(`${DIR_BASE}/view/index.md`);
    await expect(page.locator("h1")).toHaveText("Dirmode Index");
  });
});

const VIEW_PORT = 3469;
const VIEW_BASE = `http://127.0.0.1:${VIEW_PORT}`;

// Each test owns its docs, so none depends on another's edits.
test.describe("per-viewer content", () => {
  let server: ChildProcess;
  let dir: string;
  const doc = (rel: string) => path.join(dir, rel);

  test.beforeAll(async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-view-"));
    fs.mkdirSync(path.join(dir, "sub"));
    fs.writeFileSync(doc("index.md"), "# Index\n");
    fs.writeFileSync(doc("sub/hello.md"), "# Hello\n");
    fs.writeFileSync(doc("sub/page.md"), "# Page\n");
    fs.writeFileSync(doc("sub/doomed.md"), "# Doomed\n");
    fs.writeFileSync(doc("sub/stays.md"), "# Stays\n");
    fs.writeFileSync(doc("other.md"), "# Other\n");
    server = await startBirta(VIEW_PORT, doc("index.md"));
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("the page says hello with its path and a per-tab id", async ({
    page,
    context,
  }) => {
    // Every socket in the context, in order; each page opens one at a time.
    const sent: string[] = [];
    await context.routeWebSocket(/\/ws$/, (ws) => {
      const server = ws.connectToServer();
      ws.onMessage((m) => {
        sent.push(String(m));
        server.send(m);
      });
    });
    const hellos = () =>
      sent.map((m) => JSON.parse(m)).filter((m) => m.type === "hello");

    await page.goto(`${VIEW_BASE}/view/sub/hello.md`);
    await expect.poll(() => hellos().length).toBe(1);
    const [first] = hellos();
    expect(first.path).toBe("sub/hello.md");
    expect(first.tab).toMatch(/^[A-Za-z0-9_-]{1,64}$/);

    await page.reload();
    await expect.poll(() => hellos().length).toBe(2);
    expect(hellos()[1].tab).toBe(first.tab);

    // window.open copies sessionStorage, as the browser's Duplicate Tab does.
    const [popup] = await Promise.all([
      page.waitForEvent("popup"),
      page.evaluate(() => window.open(location.href)),
    ]);
    await expect.poll(() => hellos().length).toBe(3);
    expect(hellos()[2].tab).not.toBe(first.tab);
    await popup.close();

    const other = await context.newPage();
    await other.goto(`${VIEW_BASE}/view/sub/hello.md`);
    await expect.poll(() => hellos().length).toBe(4);
    expect(hellos()[3].tab).not.toBe(first.tab);
    expect(hellos()[3].tab).not.toBe(hellos()[2].tab);
  });

  test("gone for another path is ignored", async ({ page }) => {
    // A mocked socket, so the frames the page sees are only these.
    let route: import("@playwright/test").WebSocketRoute | undefined;
    await page.routeWebSocket(/\/ws$/, (ws) => {
      route = ws;
    });
    const status = page.locator("#status");
    const h1 = page.locator("h1");
    const content = (p: string, title: string) =>
      JSON.stringify({ type: "content", path: p, html: `<h1>${title}</h1>` });
    await page.goto(`${VIEW_BASE}/view/sub/stays.md`);
    await expect.poll(() => route !== undefined).toBe(true);

    // Content for the page would hide the pill, so nothing follows the gone here.
    route!.send(JSON.stringify({ type: "gone", path: "other.md" }));
    await page.waitForTimeout(300);
    await expect(status).not.toHaveClass(/visible/);
    route!.send(content("sub/stays.md", "Marker"));
    await expect(h1).toHaveText("Marker");

    route!.send(JSON.stringify({ type: "gone", path: "sub/stays.md" }));
    await expect(status).toHaveText("file removed");
    route!.send(content("other.md", "Elsewhere"));
    route!.send(JSON.stringify({ type: "gone", path: "zzz.md" }));
    await page.waitForTimeout(300);
    await expect(h1).toHaveText("Marker");
    await expect(status).toHaveText("file removed");
    await expect(status).toHaveClass(/visible/);
  });

  test("content for the bytes already shown is not re-applied", async ({
    page,
  }) => {
    let route: import("@playwright/test").WebSocketRoute | undefined;
    await page.routeWebSocket(/\/ws$/, (ws) => {
      route = ws;
    });
    const h1 = page.locator("h1");
    await page.goto(`${VIEW_BASE}/view/sub/stays.md`);
    await expect.poll(() => route !== undefined).toBe(true);
    const shown = await page.evaluate(
      () => JSON.parse(document.getElementById("birta-boot")!.textContent!).rev as string
    );
    const content = (rev: string, title: string) =>
      JSON.stringify({
        type: "content",
        path: "sub/stays.md",
        rev,
        html: `<h1>${title}</h1>`,
      });

    route!.send(content(shown, "Same bytes"));
    await page.waitForTimeout(300);
    await expect(h1).toHaveText(/^Stays/);
    route!.send(content("f".repeat(40), "New bytes"));
    await expect(h1).toHaveText("New bytes");
    // A theme-only resync leaves the shown content stale, so the same rev applies again.
    route!.send(
      JSON.stringify({
        type: "theme_update",
        css_vars: "",
        theme_name: "github",
        theme_attr: "",
        variants: ["light", "dark"],
        active_variant: "dark",
        has_toggle: true,
      })
    );
    route!.send(content("f".repeat(40), "Restyled"));
    await expect(h1).toHaveText("Restyled");
  });

  test("a page that missed a change while offline shows it on reconnect", async ({
    page,
  }) => {
    // While held, the page's socket is mocked and never reaches the server.
    let hold = false;
    const routes: import("@playwright/test").WebSocketRoute[] = [];
    await page.routeWebSocket(/\/ws$/, (ws) => {
      routes.push(ws);
      if (!hold) ws.connectToServer();
    });
    await page.goto(`${VIEW_BASE}/view/sub/page.md`);
    await expect(page.locator("h1")).toHaveText("Page");
    await expect.poll(() => routes.length).toBe(1);

    hold = true;
    await routes[0].close();
    await expect.poll(() => routes.length).toBe(2);
    fs.writeFileSync(doc("sub/page.md"), "# Page changed\n");
    await page.waitForTimeout(1500);
    await expect(page.locator("h1")).toHaveText("Page");

    hold = false;
    await routes[1].close();
    await expect.poll(() => routes.length).toBe(3);
    await expect(page.locator("h1")).toHaveText("Page changed", {
      timeout: 10_000,
    });
  });

  test("a removed doc shows the file removed pill until it returns", async ({
    page,
  }) => {
    const status = page.locator("#status");
    await page.goto(`${VIEW_BASE}/view/sub/doomed.md`);
    await expect(page.locator("h1")).toHaveText("Doomed");

    fs.unlinkSync(doc("sub/doomed.md"));
    await expect(status).toHaveText("file removed", { timeout: 10_000 });
    await expect(status).toHaveClass(/visible/);

    fs.writeFileSync(doc("sub/doomed.md"), "# Doomed again\n");
    await expect(page.locator("h1")).toHaveText("Doomed again", {
      timeout: 10_000,
    });
    await expect(status).not.toHaveClass(/visible/);
  });

  test("removing another doc shows no pill", async ({ page }) => {
    const status = page.locator("#status");
    await page.goto(`${VIEW_BASE}/view/sub/stays.md`);
    await expect(page.locator("h1")).toHaveText("Stays");

    fs.unlinkSync(doc("other.md"));
    // The viewed doc's own update orders the check after the removal.
    fs.writeFileSync(doc("sub/stays.md"), "# Stays edited\n");
    await expect(page.locator("h1")).toHaveText("Stays edited", {
      timeout: 10_000,
    });
    await expect(status).not.toHaveClass(/visible/);
    await expect(status).not.toHaveText("file removed");
  });
});

const MOVED_PORT = 3473;
const MOVED_BASE = `http://127.0.0.1:${MOVED_PORT}`;

// The entry doc is renamed away after start; theme changes must still reach other pages.
test.describe("entry doc moved away", () => {
  let server: ChildProcess;
  let dir: string;

  test.beforeAll(async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-moved-"));
    fs.writeFileSync(path.join(dir, "index.md"), "# Index\n");
    fs.writeFileSync(path.join(dir, "other.md"), "# Other\n\n```rust\nfn main() {}\n```\n");
    server = await startBirta(MOVED_PORT, dir);
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("a variant change still re-themes another page", async ({ page }) => {
    const html = page.locator("html");
    await page.goto(`${MOVED_BASE}/view/other.md`);
    await expect(page.locator("h1")).toHaveText("Other");
    const before = await html.getAttribute("data-theme");

    fs.renameSync(path.join(dir, "index.md"), path.join(dir, "home.md"));
    await page.locator("#theme-toggle").click();

    await expect(html).toHaveAttribute("data-theme", before === "dark" ? "light" : "dark", {
      timeout: 10_000,
    });
    await expect(page.locator("h1")).toHaveText("Other");
  });
});
