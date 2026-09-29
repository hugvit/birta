import { test, expect } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as path from "path";

// --- Helpers ---

const PORT = 3460;
const BASE = `http://127.0.0.1:${PORT}`;

async function startBirta(
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
  throw new Error(`Birta did not start on port ${port} within 120s`);
}

function stopBirta(proc: ChildProcess): void {
  proc.kill("SIGTERM");
}

async function getSyntaxColor(
  page: import("@playwright/test").Page
): Promise<string> {
  return page.evaluate(() => {
    const span = document.querySelector("pre code span");
    if (!span) throw new Error("No syntax span found");
    return getComputedStyle(span).color;
  });
}

async function waitForThemeUpdate(
  page: import("@playwright/test").Page
): Promise<void> {
  // Wait for the theme_update WS message to be processed.
  // The theme_update replaces mdBody.innerHTML, so wait for a fresh render.
  await page.waitForTimeout(1000);
}

async function switchTheme(
  page: import("@playwright/test").Page,
  themeName: string
): Promise<void> {
  await page.locator("#theme-dropdown-btn").click();
  await page
    .locator(`.theme-dropdown-item[data-theme="${themeName}"]`)
    .click();
  await waitForThemeUpdate(page);
}

async function toggleVariant(
  page: import("@playwright/test").Page
): Promise<void> {
  await page.locator("#theme-toggle").click();
  await waitForThemeUpdate(page);
}

async function navigateToLink(
  page: import("@playwright/test").Page,
  linkText: string
): Promise<void> {
  await page.locator(`a:has-text("${linkText}")`).click();
  await page.waitForURL(/\/view\//);
}

// --- Tests ---

test.describe.serial("directory navigation", () => {
  let server: ChildProcess;

  test.beforeAll(async () => {
    server = await startBirta(PORT, "fixtures/directory/index.md");
  });

  test.afterAll(() => {
    stopBirta(server);
  });

  // --- Group 1: Basic navigation ---

  test("click relative link navigates to new page", async ({ page }) => {
    await page.goto(BASE);
    await expect(page.locator("h1")).toHaveText("Index Page");

    await navigateToLink(page, "Page One");

    await expect(page).toHaveURL(/\/view\/page-1\.md/);
    await expect(page.locator("h1")).toHaveText("Page One");
  });

  // --- Group 2: GitHub (native) theme — persistence & current-page re-render ---

  test("variant persists across navigation (github)", async ({ page }) => {
    await page.goto(BASE);
    await toggleVariant(page);
    const variant = await page.locator("html").getAttribute("data-theme");
    expect(variant).toBe("dark");

    await navigateToLink(page, "Page One");

    await expect(page.locator("h1")).toHaveText("Page One");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  });

  test("variant toggle stays on current page (github)", async ({ page }) => {
    await page.goto(BASE);
    // Set dark and navigate
    await toggleVariant(page);
    await navigateToLink(page, "Page One");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

    // Toggle back to light on page-1
    await toggleVariant(page);

    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
    await expect(page.locator("h1")).toHaveText("Page One");
  });

  test("syntax colors change on variant toggle (github)", async ({ page }) => {
    await page.goto(BASE);
    // Ensure light
    await page.evaluate(() => {
      document.documentElement.setAttribute("data-theme", "light");
      sessionStorage.setItem("birta-variant", "light");
    });
    await navigateToLink(page, "Page One");

    const colorBefore = await getSyntaxColor(page);

    await toggleVariant(page);
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

    const colorAfter = await getSyntaxColor(page);
    expect(colorAfter).not.toBe(colorBefore);
  });

  // --- Group 3: External theme — persistence & current-page re-render ---

  test("theme and variant persist across navigation (external)", async ({
    page,
  }) => {
    await page.goto(BASE);
    await switchTheme(page, "catppuccin");
    await expect(page.locator("html")).toHaveAttribute(
      "data-birta-theme",
      "catppuccin"
    );
    const variant = await page.locator("html").getAttribute("data-theme");

    await navigateToLink(page, "Page One");

    await expect(page.locator("h1")).toHaveText("Page One");
    await expect(page.locator("html")).toHaveAttribute(
      "data-birta-theme",
      "catppuccin"
    );
    await expect(page.locator("html")).toHaveAttribute("data-theme", variant!);
  });

  test("variant toggle stays on current page (external)", async ({ page }) => {
    await page.goto(BASE);
    await switchTheme(page, "catppuccin");
    await navigateToLink(page, "Page One");

    const variantBefore = await page.locator("html").getAttribute("data-theme");
    await toggleVariant(page);

    const variantAfter = await page.locator("html").getAttribute("data-theme");
    expect(variantAfter).not.toBe(variantBefore);
    await expect(page.locator("h1")).toHaveText("Page One");
  });

  test("switching external themes stays on current page", async ({ page }) => {
    await page.goto(BASE);
    await switchTheme(page, "catppuccin");
    await navigateToLink(page, "Page One");

    await switchTheme(page, "gruvbox");

    await expect(page.locator("html")).toHaveAttribute(
      "data-birta-theme",
      "gruvbox"
    );
    await expect(page.locator("h1")).toHaveText("Page One");
  });

  test("syntax colors change on variant toggle (external)", async ({
    page,
  }) => {
    await page.goto(BASE);
    await switchTheme(page, "gruvbox");
    await navigateToLink(page, "Page One");

    const colorBefore = await getSyntaxColor(page);
    await toggleVariant(page);

    const colorAfter = await getSyntaxColor(page);
    expect(colorAfter).not.toBe(colorBefore);
    await expect(page.locator("h1")).toHaveText("Page One");
  });

  // --- Group 4: Cross-category theme switching ---

  test("native to external stays on current page", async ({ page }) => {
    await page.goto(BASE);
    // Reset to github (native) — previous tests may have left a different theme
    await switchTheme(page, "github");
    await expect(page.locator("html")).not.toHaveAttribute("data-birta-theme");
    await navigateToLink(page, "Page One");

    // Switch to external
    await switchTheme(page, "catppuccin");

    await expect(page.locator("html")).toHaveAttribute(
      "data-birta-theme",
      "catppuccin"
    );
    await expect(page.locator("h1")).toHaveText("Page One");

    // External themes use inline styles on syntax spans
    const hasInlineStyle = await page.evaluate(() => {
      const span = document.querySelector("pre code span");
      return span ? span.hasAttribute("style") : false;
    });
    expect(hasInlineStyle).toBe(true);
  });

  test("external to native stays on current page", async ({ page }) => {
    await page.goto(BASE);
    await switchTheme(page, "catppuccin");
    await navigateToLink(page, "Page One");

    // Switch back to github (native)
    await switchTheme(page, "github");

    await expect(page.locator("html")).not.toHaveAttribute("data-birta-theme");
    await expect(page.locator("h1")).toHaveText("Page One");

    // Native theme uses CSS classes, no inline styles on syntax spans
    const hasInlineStyle = await page.evaluate(() => {
      const span = document.querySelector("pre code span");
      return span ? span.hasAttribute("style") : false;
    });
    expect(hasInlineStyle).toBe(false);
  });
});
