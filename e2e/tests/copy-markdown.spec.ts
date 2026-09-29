import { test, expect, Page } from "@playwright/test";
import { execFileSync } from "child_process";
import fs from "fs";
import os from "os";
import path from "path";

const FIXTURES = path.join(__dirname, "..", "fixtures");
const COPY_FIXTURE = path.join(FIXTURES, "copy.md");
const LIVE_FIXTURE = path.join(FIXTURES, "copy-live.md");
const LIVE_ORIGINAL = "# Live Copy\n\nOriginal body.\n";

type CopyLog = { __copied: string[]; __paths: string[] };

// Records what gets copied, and via which path. `secure: false` mimics an
// insecure origin, where browsers expose no `navigator.clipboard` at all.
async function captureClipboard(page: Page, secure = true): Promise<void> {
  await page.addInitScript((secure: boolean) => {
    const w = window as unknown as CopyLog;
    w.__copied = [];
    w.__paths = [];
    if (secure) {
      Object.defineProperty(navigator, "clipboard", {
        value: {
          writeText: (text: string) => {
            w.__copied.push(text);
            w.__paths.push("clipboard-api");
            return Promise.resolve();
          },
        },
      });
      return;
    }
    Object.defineProperty(window, "isSecureContext", { value: false });
    Object.defineProperty(navigator, "clipboard", { value: undefined });
    document.addEventListener("copy", (e) => {
      const ta = e.target as HTMLTextAreaElement;
      w.__copied.push(ta.value.slice(ta.selectionStart, ta.selectionEnd));
      w.__paths.push("exec-command");
    });
  }, secure);
}

async function copied(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as CopyLog).__copied);
}

async function copyPaths(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as CopyLog).__paths);
}

test.describe("copy markdown button", () => {
  test.afterEach(() => {
    fs.writeFileSync(LIVE_FIXTURE, LIVE_ORIGINAL);
  });

  test("copies the exact source from preview mode", async ({ page }) => {
    await captureClipboard(page);
    await page.goto("/view/copy.md");
    await expect(page.locator("#content")).toBeVisible();
    await expect(page.locator("#source-view")).toBeHidden();

    await page.click("#copy-md-btn");

    await expect.poll(() => copied(page)).toEqual([fs.readFileSync(COPY_FIXTURE, "utf-8")]);
  });

  test("copies the exact source from raw mode", async ({ page }) => {
    await captureClipboard(page);
    await page.goto("/view/copy.md");
    await page.click('#view-toggle [data-view="raw"]');
    await expect(page.locator("#source-view")).toBeVisible();

    await page.click("#copy-md-btn");

    await expect.poll(() => copied(page)).toEqual([fs.readFileSync(COPY_FIXTURE, "utf-8")]);
  });

  test("copies updated source after live reload", async ({ page }) => {
    await captureClipboard(page);
    await page.goto("/view/copy-live.md");
    await expect(page.locator("h1")).toHaveText("Live Copy");

    const updated = "# Live Copy Updated\n\nNew <b>body</b> & more.\n";
    fs.writeFileSync(LIVE_FIXTURE, updated);
    await expect(page.locator("h1")).toHaveText("Live Copy Updated", { timeout: 5000 });

    await page.click("#copy-md-btn");

    await expect.poll(() => copied(page)).toEqual([updated]);
  });

  test("falls back to execCommand without a secure context", async ({ page }) => {
    await captureClipboard(page, false);
    await page.goto("/view/copy.md");

    await page.click("#copy-md-btn");

    await expect.poll(() => copied(page)).toEqual([fs.readFileSync(COPY_FIXTURE, "utf-8")]);
    expect(await copyPaths(page)).toEqual(["exec-command"]);
    await expect(page.locator("textarea")).toHaveCount(0);
    await expect(page.locator("#copy-md-btn")).toHaveClass(/copied/);
  });

  test("reports failure when the execCommand fallback fails", async ({ page }) => {
    await captureClipboard(page, false);
    await page.addInitScript(() => {
      document.execCommand = () => false;
    });
    await page.goto("/view/copy.md");
    const btn = page.locator("#copy-md-btn");

    await btn.click();

    await expect(btn).toHaveAttribute("title", "Copy failed");
    await expect(btn).not.toHaveClass(/copied/);
    await expect(page.locator("textarea")).toHaveCount(0);
    await expect(btn).toHaveAttribute("title", "Copy markdown", { timeout: 4000 });
  });

  test("preserves CRLF and lone CR line endings", async ({ page }) => {
    // Written at runtime so git line-ending conversion can't normalize it
    const crlfFixture = path.join(FIXTURES, "copy-crlf.md");
    const crlf = "# H1\r\n## H2 text\r\n\r\n- item\r\n\r\n```\r\ncode\r\n```\r\nlone\rcr\r\n";
    fs.writeFileSync(crlfFixture, crlf);
    try {
      await captureClipboard(page);
      await page.goto("/view/copy-crlf.md");

      await page.click("#copy-md-btn");

      await expect.poll(() => copied(page)).toEqual([crlf]);
    } finally {
      fs.rmSync(crlfFixture, { force: true });
    }
  });

  test("shows a checkmark after copying, then resets", async ({ page }) => {
    await captureClipboard(page);
    await page.goto("/view/copy.md");
    const btn = page.locator("#copy-md-btn");
    await expect(btn.locator(".icon-check")).toBeHidden();

    await btn.click();

    await expect(btn).toHaveClass(/copied/);
    await expect(btn).toHaveAttribute("title", "Copied!");
    await expect(btn.locator(".icon-check")).toBeVisible();
    await expect(btn.locator(".icon-copy")).toBeHidden();

    await expect(btn).not.toHaveClass(/copied/, { timeout: 4000 });
    await expect(btn).toHaveAttribute("title", "Copy markdown");
    await expect(btn.locator(".icon-copy")).toBeVisible();
  });

  test("reports failure without showing the checkmark", async ({ page }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        value: { writeText: () => Promise.reject(new Error("denied")) },
      });
    });
    await page.goto("/view/copy.md");
    const btn = page.locator("#copy-md-btn");

    await btn.click();

    await expect(btn).toHaveAttribute("title", "Copy failed");
    await expect(btn).not.toHaveClass(/copied/);
  });

  test("keeps the button on-screen at narrow widths", async ({ page }) => {
    const longName = path.join(FIXTURES, "a-really-long-markdown-file-name-for-overflow.md");
    fs.writeFileSync(longName, "# Long\n");
    try {
      await page.setViewportSize({ width: 360, height: 640 });
      await page.goto("/view/" + path.basename(longName));
      await page.click('#view-toggle [data-view="raw"]');

      const header = page.locator(".file-header");
      const overflow = await header.evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      await expect(page.locator("#copy-md-btn")).toBeInViewport({ ratio: 1 });
      await expect(page.locator("#view-toggle")).toBeInViewport({ ratio: 1 });
    } finally {
      fs.rmSync(longName, { force: true });
    }
  });

  test("copies the exact source in a --static bundle", async ({ page }) => {
    const out = fs.mkdtempSync(path.join(os.tmpdir(), "birta-copy-"));
    try {
      execFileSync(
        "cargo",
        ["run", "-q", "--", "--static", "--no-open", "--out", out, COPY_FIXTURE],
        { cwd: path.join(__dirname, ".."), stdio: "pipe" }
      );
      await captureClipboard(page);
      await page.goto("file://" + path.join(out, "copy.html"));

      await page.click("#copy-md-btn");

      await expect.poll(() => copied(page)).toEqual([fs.readFileSync(COPY_FIXTURE, "utf-8")]);
    } finally {
      fs.rmSync(out, { recursive: true, force: true });
    }
  });
});
