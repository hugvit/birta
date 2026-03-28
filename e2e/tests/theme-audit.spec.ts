import { test, expect } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as path from "path";

// --- Helpers ---

function hexToRgb(hex: string): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgb(${r}, ${g}, ${b})`;
}

const TRANSPARENT = "rgba(0, 0, 0, 0)";

async function getStyle(
  page: import("@playwright/test").Page,
  selector: string,
  property: string
): Promise<string> {
  return page.evaluate(
    ([sel, prop]) => {
      const el = document.querySelector(sel!);
      if (!el) throw new Error(`Element not found: ${sel}`);
      return getComputedStyle(el).getPropertyValue(prop!);
    },
    [selector, property]
  );
}

async function startSheen(
  port: number,
  fixture: string,
  extraArgs: string[] = []
): Promise<ChildProcess> {
  const cwd = path.resolve(__dirname, "..");
  const args = [
    "run",
    "--",
    "--no-open",
    "--port",
    String(port),
    ...extraArgs,
    fixture,
  ];
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
  throw new Error(`Sheen did not start on port ${port} within 120s`);
}

function stopSheen(proc: ChildProcess): void {
  proc.kill("SIGTERM");
}

// --- GitHub Dark Mode ---

test.describe.serial("css audit: github dark", () => {
  let server: ChildProcess;
  const BASE = "http://127.0.0.1:3457";

  test.beforeAll(async () => {
    server = await startSheen(3457, "fixtures/theme-test.md");
  });

  test.afterAll(() => {
    stopSheen(server);
  });

  async function setupDarkMode(page: import("@playwright/test").Page) {
    await page.goto(BASE);
    await page.evaluate(() => {
      document.documentElement.setAttribute("data-theme", "dark");
      document.getElementById("content")!.setAttribute("data-theme", "dark");
    });
    // Let CSS transitions settle (background-color has 0.2s transition)
    await page.waitForTimeout(300);
  }

  test("page background", async ({ page }) => {
    await setupDarkMode(page);
    const bg = await getStyle(page, "body", "background-color");
    expect(bg).toBe(hexToRgb("#0d1117"));
  });

  test("header background", async ({ page }) => {
    await setupDarkMode(page);
    const bg = await getStyle(page, ".header", "background-color");
    expect(bg).toBe(hexToRgb("#161b22"));
  });

  test("header border", async ({ page }) => {
    await setupDarkMode(page);
    const border = await getStyle(page, ".header", "border-bottom-color");
    expect(border).toBe(hexToRgb("#30363d"));
  });

  test("container border", async ({ page }) => {
    await setupDarkMode(page);
    const border = await getStyle(page, ".container", "border-top-color");
    expect(border).toBe(hexToRgb("#30363d"));
  });

  test("body text color", async ({ page }) => {
    await setupDarkMode(page);
    const color = await getStyle(page, ".markdown-body", "color");
    expect(color).toBe(hexToRgb("#f0f6fc"));
  });

  test("code block background", async ({ page }) => {
    await setupDarkMode(page);
    const bg = await getStyle(page, ".markdown-body pre", "background-color");
    expect(bg).toBe(hexToRgb("#161b22"));
  });

  test("link color", async ({ page }) => {
    await setupDarkMode(page);
    const color = await getStyle(page, ".markdown-body a[href]", "color");
    expect(color).toBe(hexToRgb("#4493f8"));
  });

  // GitHub default (non-themed) alert borders come from vendored github-markdown.css
  // using --borderColor-*-emphasis variables, not our --alert-* variables.
  test("alert note border and title", async ({ page }) => {
    await setupDarkMode(page);
    const border = await getStyle(
      page,
      ".markdown-alert-note",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#1f6feb"));

    const titleColor = await getStyle(
      page,
      ".markdown-alert-note .markdown-alert-title",
      "color"
    );
    expect(titleColor).toBe(hexToRgb("#4493f8"));
  });

  test("alert tip border", async ({ page }) => {
    await setupDarkMode(page);
    const border = await getStyle(
      page,
      ".markdown-alert-tip",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#238636"));
  });

  test("alert important border", async ({ page }) => {
    await setupDarkMode(page);
    const border = await getStyle(
      page,
      ".markdown-alert-important",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#8957e5"));
  });

  test("alert warning border", async ({ page }) => {
    await setupDarkMode(page);
    const border = await getStyle(
      page,
      ".markdown-alert-warning",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#9e6a03"));
  });

  test("alert caution border", async ({ page }) => {
    await setupDarkMode(page);
    const border = await getStyle(
      page,
      ".markdown-alert-caution",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#da3633"));
  });

  test("alert border radius is 6px", async ({ page }) => {
    await setupDarkMode(page);
    const radius = await getStyle(
      page,
      ".markdown-alert-note",
      "border-radius"
    );
    expect(radius).toBe("6px");
  });
});

// --- Catppuccin Mocha ---

test.describe.serial("css audit: catppuccin-mocha", () => {
  let server: ChildProcess;
  const BASE = "http://127.0.0.1:3458";

  test.beforeAll(async () => {
    server = await startSheen(3458, "fixtures/theme-test.md", [
      "--theme",
      "../assets/themes/catppuccin-mocha",
    ]);
  });

  test.afterAll(() => {
    stopSheen(server);
  });

  test("page background", async ({ page }) => {
    await page.goto(BASE);
    const bg = await getStyle(page, "body", "background-color");
    expect(bg).toBe(hexToRgb("#1e1e2e"));
  });

  test("header background", async ({ page }) => {
    await page.goto(BASE);
    const bg = await getStyle(page, ".header", "background-color");
    expect(bg).toBe(hexToRgb("#181825"));
  });

  test("header border is transparent", async ({ page }) => {
    await page.goto(BASE);
    const border = await getStyle(page, ".header", "border-bottom-color");
    expect(border).toBe(TRANSPARENT);
  });

  test("container border is transparent", async ({ page }) => {
    await page.goto(BASE);
    const border = await getStyle(page, ".container", "border-top-color");
    expect(border).toBe(TRANSPARENT);
  });

  test("body text color", async ({ page }) => {
    await page.goto(BASE);
    const color = await getStyle(page, ".markdown-body", "color");
    expect(color).toBe(hexToRgb("#cdd6f4"));
  });

  test("code block background", async ({ page }) => {
    await page.goto(BASE);
    const bg = await getStyle(page, ".markdown-body pre", "background-color");
    expect(bg).toBe(hexToRgb("#181825"));
  });

  test("link color", async ({ page }) => {
    await page.goto(BASE);
    const color = await getStyle(page, ".markdown-body a[href]", "color");
    expect(color).toBe(hexToRgb("#89b4fa"));
  });

  test("alert note border and title", async ({ page }) => {
    await page.goto(BASE);
    const border = await getStyle(
      page,
      ".markdown-alert-note",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#89b4fa"));

    const titleColor = await getStyle(
      page,
      ".markdown-alert-note .markdown-alert-title",
      "color"
    );
    expect(titleColor).toBe(hexToRgb("#89b4fa"));
  });

  test("alert tip border", async ({ page }) => {
    await page.goto(BASE);
    const border = await getStyle(
      page,
      ".markdown-alert-tip",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#a6e3a1"));
  });

  test("alert important border", async ({ page }) => {
    await page.goto(BASE);
    const border = await getStyle(
      page,
      ".markdown-alert-important",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#cba6f7"));
  });

  test("alert warning border", async ({ page }) => {
    await page.goto(BASE);
    const border = await getStyle(
      page,
      ".markdown-alert-warning",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#fab387"));
  });

  test("alert caution border", async ({ page }) => {
    await page.goto(BASE);
    const border = await getStyle(
      page,
      ".markdown-alert-caution",
      "border-left-color"
    );
    expect(border).toBe(hexToRgb("#f38ba8"));
  });

  test("alert border radius is 0px", async ({ page }) => {
    await page.goto(BASE);
    const radius = await getStyle(
      page,
      ".markdown-alert-note",
      "border-radius"
    );
    expect(radius).toBe("0px");
  });

  test("data-sheen-theme attribute is set", async ({ page }) => {
    await page.goto(BASE);
    const attr = await page
      .locator("html")
      .getAttribute("data-sheen-theme");
    expect(attr).toBe("catppuccin-mocha");
  });

  test("theme toggle is hidden", async ({ page }) => {
    await page.goto(BASE);
    const toggle = page.locator("#theme-toggle");
    await expect(toggle).toBeHidden();
  });
});
