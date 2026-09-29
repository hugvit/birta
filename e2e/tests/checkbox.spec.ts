import { test, expect, Page } from "@playwright/test";
import { execFileSync } from "child_process";
import fs from "fs";
import path from "path";

const FIXTURE = path.join(__dirname, "..", "fixtures", "checkbox.md");
const ORIGINAL = "# Checkbox\n\n- [ ] first\n- [ ] second\n";

// Independent of the server's hashing: the same value as `git hash-object`.
function gitRev(text: string): string {
  return execFileSync("git", ["hash-object", "--no-filters", "--stdin"], { input: text })
    .toString()
    .trim();
}

function frame(data: string | Buffer): { type?: string; path?: string; rev?: string } {
  try {
    return JSON.parse(data.toString());
  } catch {
    return {};
  }
}

async function readFixture(): Promise<string> {
  return fs.readFileSync(FIXTURE, "utf-8");
}

function box(page: Page, n: number) {
  return page.locator('article.markdown-body input[type="checkbox"]').nth(n);
}

test.beforeEach(() => fs.writeFileSync(FIXTURE, ORIGINAL));
test.afterEach(() => fs.writeFileSync(FIXTURE, ORIGINAL));

test("back-to-back ticks both land", async ({ page }) => {
  const revs: string[] = [];
  page.on("websocket", (ws) =>
    ws.on("framereceived", ({ payload }) => {
      const msg = frame(payload);
      if (msg.type === "content" && msg.path === "checkbox.md" && msg.rev) revs.push(msg.rev);
    }),
  );
  await page.goto("/view/checkbox.md");
  await expect(box(page, 0)).toBeEnabled();
  // Marks the current boxes, so the refreshed content is visible as unmarked ones.
  const stale = page.locator("article.markdown-body input[data-stale]");
  await page.$$eval('article.markdown-body input[type="checkbox"]', (els) =>
    els.forEach((el) => el.setAttribute("data-stale", "")),
  );

  // The second tick goes out before any refreshed content can arrive.
  await box(page, 0).check();
  await box(page, 1).check();
  const after = "# Checkbox\n\n- [x] first\n- [x] second\n";
  await expect.poll(readFixture).toBe(after);
  await expect(page.locator("#status")).not.toHaveText("changed since · try again");

  // The watcher's content carries the rev of the bytes it rendered.
  await expect.poll(() => revs.includes(gitRev(after))).toBe(true);
  await expect(stale).toHaveCount(0);
  await expect(box(page, 0)).toBeChecked();
  await expect(box(page, 1)).toBeChecked();
});

test("a nack reverts the checkbox and shows the pill", async ({ page }) => {
  let dropContent = false;
  await page.routeWebSocket(/\/ws$/, (ws) => {
    const server = ws.connectToServer();
    ws.onMessage((m) => server.send(m));
    server.onMessage((m) => {
      // Holds back the refresh, so the page keeps its old rev.
      if (dropContent && frame(m).type === "content") return;
      ws.send(m);
    });
  });
  await page.goto("/view/checkbox.md");
  await expect(box(page, 0)).toBeEnabled();

  dropContent = true;
  const edited = "# Checkbox\n\n- [ ] first\n- [ ] second, edited elsewhere\n";
  fs.writeFileSync(FIXTURE, edited);
  await box(page, 0).check();

  await expect(page.locator("#status")).toHaveText("changed since · try again");
  await expect(page.locator("#status")).toHaveClass(/visible/);
  await expect(box(page, 0)).not.toBeChecked();
  expect(await readFixture()).toBe(edited);
});
