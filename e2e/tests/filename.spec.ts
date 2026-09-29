import { test, expect } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

const PORT = 3461;
const BASE = `http://127.0.0.1:${PORT}`;
const NAMES = ["plain.md", "don't.md", "x';alert(1);'.md"];

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

test.describe("quotes in filenames", () => {
  let server: ChildProcess;
  let dir: string;

  test.beforeAll(async () => {
    // realpath: the watcher compares canonical paths and macOS tmpdir is a symlink
    dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-")));
    for (const name of NAMES) {
      fs.writeFileSync(path.join(dir, name), "# Before\n");
    }
    server = await startBirta(PORT, path.join(dir, NAMES[0]));
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  for (const name of NAMES) {
    test(`${name} shows its name and keeps the viewer script alive`, async ({
      page,
    }) => {
      const errors: string[] = [];
      const dialogs: string[] = [];
      page.on("pageerror", (e) => errors.push(e.message));
      page.on("dialog", (d) => {
        dialogs.push(d.message());
        void d.dismiss();
      });

      await page.goto(`${BASE}/view/${encodeURIComponent(name)}`);
      await expect(page.locator(".file-header-name")).toHaveText(name);
      await expect(page).toHaveTitle(`${name} — birta`);
      await expect(page.locator("h1").first()).toHaveText("Before");
      expect(errors).toEqual([]);
      expect(dialogs).toEqual([]);

      // Live reload runs only if the inline script parsed and its path matches the update
      fs.writeFileSync(path.join(dir, name), "# After\n");
      await expect(page.locator("h1").first()).toHaveText("After", {
        timeout: 5000,
      });

      expect(errors).toEqual([]);
      expect(dialogs).toEqual([]);
    });
  }
});
