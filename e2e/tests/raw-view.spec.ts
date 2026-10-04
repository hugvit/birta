import { test, expect, Page } from "@playwright/test";
import { execFileSync } from "child_process";
import fs from "fs";
import os from "os";
import path from "path";
import { test as reviewTest, GATE_ON, booted } from "./review/harness";

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

// A private server per test, so no user config on this machine decides the keys
reviewTest("a plain preview tab keeps u for the source view, and ⌥u leaves it alone", async ({ page, birta }) => {
  const served = await birta({ "pr.md": "# Plain\n\nWords.\n" }, {});
  await page.goto(`${served.url}/view/pr.md`);
  await page.keyboard.press("Alt+KeyU");
  await page.waitForTimeout(400);
  await expect(page.locator("#source-view")).toBeHidden();
  await page.keyboard.press("u");
  await expect(page.locator("#source-view")).toBeVisible();
  await page.keyboard.press("u");
  await expect(page.locator("#lane-frame")).toBeVisible();
});

reviewTest("a review tab moves the source view to ⌥u, and u leaves it alone", async ({ page, birta }) => {
  const served = await birta({ "pr.md": "# Review\n\nWords.\n" }, GATE_ON);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  await page.keyboard.press("u");
  await page.waitForTimeout(400);
  await expect(page.locator("#source-view")).toBeHidden();
  await page.keyboard.press("Alt+KeyU");
  await expect(page.locator("#source-view")).toBeVisible();
  expect(await sourceOffset(page)).toBe(0);
  await page.keyboard.press("Alt+KeyU");
  await expect(page.locator("#lane-frame")).toBeVisible();
});
