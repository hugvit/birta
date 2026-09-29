import { test, expect } from "@playwright/test";
import { spawn, ChildProcess } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

const PORT = 3463;
const BASE = `http://127.0.0.1:${PORT}`;

// Raw HTML that tries to plant viewer-owned attributes and elements in the DOM
const DOC = `# Reserved

<div title='

&#39; data-sourcepos=50:1-60:1 data-br-id=zz x=&#39;

&#39;

<div title="

<p>" data-sourcepos="9:9-9:9">split</p>

<section>
<div data-sourcepos="40:1-90:1" data-br-note="n">raw</div>
<birta-flow>flow</birta-flow>
</section>
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

test.describe("reserved markup", () => {
  let server: ChildProcess;
  let dir: string;

  test.beforeAll(async () => {
    dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-")));
    const file = path.join(dir, "reserved.md");
    fs.writeFileSync(file, DOC);
    server = await startBirta(PORT, file);
  });

  test.afterAll(() => {
    server?.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("raw HTML cannot plant sourcepos, data-br or birta elements", async ({
    page,
  }) => {
    await page.goto(`${BASE}/`);
    await expect(page.locator("h1").first()).toHaveText("Reserved");

    const found = await page.evaluate(() => ({
      forged: ["50:1-60:1", "9:9-9:9", "40:1-90:1"].filter((v) =>
        document.querySelector(`[data-sourcepos="${v}"]`)
      ),
      dataBr: Array.from(document.querySelectorAll("#content *")).flatMap(
        (el) => el.getAttributeNames().filter((n) => n.startsWith("data-br-"))
      ),
      birta: Array.from(document.querySelectorAll("*"))
        .map((el) => el.localName)
        .filter((n) => n.startsWith("birta-")),
      renamed: document.querySelector('[data-x-sourcepos="40:1-90:1"]') !== null,
    }));
    expect(found).toEqual({ forged: [], dataBr: [], birta: [], renamed: true });
    await expect(page.locator("#content")).toContainText("<birta-flow>flow");
  });
});
