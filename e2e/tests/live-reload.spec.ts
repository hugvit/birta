import { test, expect } from "@playwright/test";
import fs from "fs";
import path from "path";

const SIMPLE_FIXTURE = path.join(__dirname, "..", "fixtures", "simple.md");
const ORIGINAL_CONTENT = "# Original Heading\n\nThis is the original content.\n";

test.beforeEach(() => {
  // Ensure fixture starts with original content
  fs.writeFileSync(SIMPLE_FIXTURE, ORIGINAL_CONTENT);
});

test.afterEach(() => {
  // Restore fixture
  fs.writeFileSync(SIMPLE_FIXTURE, ORIGINAL_CONTENT);
});

test("live-reloads on file change", async ({ page }) => {
  // This test needs its own server instance pointing at simple.md.
  // For now, we test the WebSocket connection against the main test server
  // and verify the mechanism works by modifying the main fixture.

  const testFixture = path.join(__dirname, "..", "fixtures", "test.md");
  const originalContent = fs.readFileSync(testFixture, "utf-8");

  await page.goto("/");
  await expect(page.locator("h1").first()).toHaveText("Sheen Test Document");

  // Modify the file
  fs.writeFileSync(
    testFixture,
    "# Updated Heading\n\nContent was updated.\n"
  );

  // Wait for WebSocket-driven update
  await expect(page.locator("h1").first()).toHaveText("Updated Heading", {
    timeout: 5000,
  });

  // Verify new content appears
  await expect(page.locator("article.markdown-body")).toContainText(
    "Content was updated"
  );

  // Restore the original file
  fs.writeFileSync(testFixture, originalContent);

  // Verify it reverts
  await expect(page.locator("h1").first()).toHaveText("Sheen Test Document", {
    timeout: 5000,
  });
});
