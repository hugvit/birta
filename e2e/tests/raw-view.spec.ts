import { test, expect, Page } from "@playwright/test";
import { execFileSync } from "child_process";
import fs from "fs";
import os from "os";
import path from "path";

/** Gap between the file header and the source view, in px. */
async function sourceOffset(page: Page): Promise<number> {
  return page.evaluate(() => {
    const header = document.querySelector(".file-header")!.getBoundingClientRect();
    const source = document.getElementById("source-view")!.getBoundingClientRect();
    return source.top - header.bottom;
  });
}

async function showRaw(page: Page): Promise<void> {
  await page.click('#view-toggle .view-btn[data-view="raw"]');
  await expect(page.locator("#source-view")).toBeVisible();
}

test("the raw view sits right under the file header", async ({ page }) => {
  await page.goto("/view/test.md");
  await showRaw(page);
  expect(await sourceOffset(page)).toBe(0);
});

test("the raw view sits right under the file header in a --static bundle", async ({ page }) => {
  const out = fs.mkdtempSync(path.join(os.tmpdir(), "birta-raw-"));
  try {
    execFileSync("cargo", ["run", "-q", "--", "--static", "--no-open", "--out", out, "fixtures/test.md"], {
      cwd: path.join(__dirname, ".."),
      stdio: "pipe",
    });
    await page.goto("file://" + path.join(out, "test.html"));
    await showRaw(page);
    expect(await sourceOffset(page)).toBe(0);
  } finally {
    fs.rmSync(out, { recursive: true, force: true });
  }
});
