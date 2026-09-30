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
