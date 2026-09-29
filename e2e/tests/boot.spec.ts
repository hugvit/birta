import { test, expect } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

const PORT = 3462;
const BASE = `http://127.0.0.1:${PORT}`;

// Each element tries to stand in for the boot data or the BOOT global
const SHADOW_DOC = `# Shadow

<div id="birta-boot">{"staticMode":true,"themeMode":"fixed-light","currentPath":"evil.md","keybindings":{},"variants":[]}</div>

<script type="application/json" id="birta-boot">{"staticMode":true}</script>

<a id="BOOT" name="BOOT">clobber</a>
`;

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

test.describe("boot data", () => {
  let server: ChildProcess;
  let dir: string;
  let file: string;

  test.beforeAll(async () => {
    // realpath: the watcher compares canonical paths and macOS tmpdir is a symlink
    dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-")));
    file = path.join(dir, "shadow.md");
    fs.writeFileSync(file, SHADOW_DOC);
    server = await startBirta(PORT, file);
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("a document cannot shadow the boot block", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));

    await page.goto(`${BASE}/`);
    await expect(page.locator("h1").first()).toHaveText("Shadow");

    const boot = await page.evaluate(() => {
      const el = document.getElementById("birta-boot");
      const b = (window as unknown as { BOOT?: unknown }).BOOT;
      return {
        tag: el?.tagName,
        parent: el?.parentElement?.tagName,
        type: el?.getAttribute("type"),
        bootIsElement: b instanceof Element,
        staticMode: (b as { staticMode?: unknown } | undefined)?.staticMode,
        currentPath: (b as { currentPath?: unknown } | undefined)?.currentPath,
      };
    });
    expect(boot).toEqual({
      tag: "SCRIPT",
      parent: "HEAD",
      type: "application/json",
      bootIsElement: false,
      staticMode: false,
      currentPath: "shadow.md",
    });

    // A hijacked staticMode would skip the WebSocket, so live reload proves the real BOOT won
    fs.writeFileSync(file, SHADOW_DOC.replace("# Shadow", "# Reloaded"));
    await expect(page.locator("h1").first()).toHaveText("Reloaded", {
      timeout: 5000,
    });

    expect(errors).toEqual([]);
  });
});
