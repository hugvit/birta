import { test, expect } from "@playwright/test";

test("page loads with correct title", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveTitle("test.md — sheen");
});

test("renders h1 heading", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("h1").first()).toHaveText("Sheen Test Document");
});

test("renders table with correct headers", async ({ page }) => {
  await page.goto("/");
  const table = page.locator("table");
  await expect(table).toBeVisible();
  const headers = table.locator("th");
  await expect(headers).toHaveCount(3);
  await expect(headers.nth(0)).toHaveText("Language");
  await expect(headers.nth(1)).toHaveText("Parser");
  await expect(headers.nth(2)).toHaveText("GFM Compliance");
});

test("renders task list with checkboxes", async ({ page }) => {
  await page.goto("/");
  const checkboxes = page.locator('input[type="checkbox"]');
  await expect(checkboxes).toHaveCount(4);
});

test("renders code block", async ({ page }) => {
  await page.goto("/");
  const codeBlock = page.locator("pre code");
  await expect(codeBlock.first()).toBeVisible();
  await expect(codeBlock.first()).toContainText("Hello, sheen!");
});

test("renders blockquote", async ({ page }) => {
  await page.goto("/");
  const blockquote = page.locator("blockquote");
  await expect(blockquote).toBeVisible();
  await expect(blockquote).toContainText("This is a blockquote");
});

test("shows filename in header", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#filename")).toHaveText("test.md");
});

test("theme toggle switches between light and dark", async ({ page }) => {
  await page.goto("/");
  const html = page.locator("html");
  const toggleBtn = page.locator("#theme-toggle");

  // Click to set an explicit theme
  await toggleBtn.click();
  const firstTheme = await html.getAttribute("data-theme");
  expect(firstTheme).toBeTruthy();

  // Click again to switch
  await toggleBtn.click();
  const secondTheme = await html.getAttribute("data-theme");
  expect(secondTheme).toBeTruthy();
  expect(secondTheme).not.toBe(firstTheme);
});

test("health endpoint returns ok", async ({ request }) => {
  const response = await request.get("/health");
  expect(response.ok()).toBeTruthy();
  expect(await response.text()).toBe("ok");
});

test("markdown body has correct class", async ({ page }) => {
  await page.goto("/");
  const article = page.locator("article.markdown-body");
  await expect(article).toBeVisible();
});

test.describe("visual regression", () => {
  test("light theme rendering", async ({ page }) => {
    await page.goto("/");
    // Force light theme
    await page.evaluate(() => {
      document.documentElement.setAttribute("data-theme", "light");
      document.getElementById("content")!.setAttribute("data-theme", "light");
    });
    await expect(page.locator(".markdown-body")).toHaveScreenshot(
      "markdown-body-light.png",
      { maxDiffPixelRatio: 0.01 }
    );
  });

  test("dark theme rendering", async ({ page }) => {
    await page.goto("/");
    // Force dark theme
    await page.evaluate(() => {
      document.documentElement.setAttribute("data-theme", "dark");
      document.getElementById("content")!.setAttribute("data-theme", "dark");
    });
    await expect(page.locator(".markdown-body")).toHaveScreenshot(
      "markdown-body-dark.png",
      { maxDiffPixelRatio: 0.01 }
    );
  });
});
