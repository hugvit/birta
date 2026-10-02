import { Page } from "@playwright/test";
import { spawn, spawnSync, ChildProcess } from "child_process";
import * as crypto from "crypto";
import * as fs from "fs";
import * as net from "net";
import * as os from "os";
import * as path from "path";
import { test, expect, BIN, GATE_ON, State, booted, ensureBuilt, focused, reviewLog } from "./harness";

// A server that stops and comes back: the banner, the reconnect, and what a server lists as its own.

const WIDE = { width: 1280, height: 900 };

const DOC = [
  "# Upload retries",
  "",
  "## Why",
  "",
  "When the upload hits a 503 we just fail.",
  "",
  "## Stacked on",
  "",
  "Nothing yet.",
  "",
].join("\n");

/** The port birta derives for a canonical served path: SHA-1's first four bytes, big-endian, mod 8000, above 41000. */
function derived(served: string): number {
  return 41000 + (crypto.createHash("sha1").update(served).digest().readUInt32BE(0) % 8000);
}

interface Site {
  base: string;
  /** The served folder, canonical. */
  root: string;
}

/** A fresh temp folder holding `files`; HOME and XDG_STATE_HOME sit beside it. */
function site(files: Record<string, string>): Site {
  const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-life-")));
  const root = path.join(base, "root");
  fs.mkdirSync(root, { recursive: true });
  for (const [rel, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.writeFileSync(path.join(root, rel), text);
  }
  return { base, root };
}

function portFree(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const probe = net.createServer();
    probe.once("error", () => resolve(false));
    probe.listen(port, "127.0.0.1", () => probe.close(() => resolve(true)));
  });
}

/** `<stem><ext>`, or `<stem>-N<ext>` when an unrelated listener holds the first name's derived port. */
async function freeName(s: Site, stem: string, ext: string): Promise<string> {
  for (let i = 0; ; i++) {
    const name = i ? `${stem}-${i}${ext}` : `${stem}${ext}`;
    if (await portFree(derived(path.join(s.root, name)))) return name;
  }
}

interface Proc {
  url: string;
  port: number;
  proc: ChildProcess;
  exited: Promise<void>;
}

const running: Proc[] = [];
const sites: Site[] = [];

/** `birta args… --no-open` with no port of its own choosing, so the path picks it. */
async function launch(s: Site, args: string[], env: Record<string, string>, cwd = s.root): Promise<Proc> {
  return start(s, BIN, [...args, "--no-open"], env, cwd);
}

/** Runs `line` through a shell, as a person pasting it would, with the built `birta` on PATH. */
async function launchLine(s: Site, line: string, env: Record<string, string>, cwd: string): Promise<Proc> {
  return start(s, "sh", ["-c", `exec env ${line}`], env, cwd);
}

async function start(s: Site, cmd: string, args: string[], env: Record<string, string>, cwd: string): Promise<Proc> {
  ensureBuilt();
  const proc = spawn(cmd, args, {
    cwd,
    env: {
      PATH: `${path.dirname(BIN)}:${process.env.PATH ?? ""}`,
      HOME: path.join(s.base, "home"),
      XDG_STATE_HOME: path.join(s.base, "state"),
      ...env,
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  const exited = new Promise<void>((resolve) => proc.on("exit", () => resolve()));
  const url = await new Promise<string>((resolve, reject) => {
    let seen = "";
    const timer = setTimeout(() => reject(new Error(`birta did not start: ${seen}`)), 20_000);
    // Read to the end, so the server never writes to a closed pipe
    proc.stderr!.on("data", (chunk: Buffer) => {
      seen += chunk.toString();
      const m = seen.match(/ at (http:\/\/127\.0\.0\.1:\d+)/);
      if (m) {
        clearTimeout(timer);
        resolve(m[1]);
      }
    });
    proc.on("exit", (code) => reject(new Error(`birta exited with ${code}: ${seen}`)));
  });
  const p = { url, port: Number(new URL(url).port), proc, exited };
  running.push(p);
  return p;
}

/** Kills the server as a crash or a background limit would, and waits until it is gone. */
async function kill(p: Proc): Promise<void> {
  p.proc.kill("SIGKILL");
  await p.exited;
}

test.afterEach(async () => {
  for (const p of running.splice(0)) await kill(p);
  for (const s of sites.splice(0)) fs.rmSync(s.base, { recursive: true, force: true });
});

function tracked(files: Record<string, string>): Site {
  const s = site(files);
  sites.push(s);
  return s;
}

/** Records what the page copies. */
async function captureClipboard(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as { __copied: string[] };
    w.__copied = [];
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: (text: string) => {
          w.__copied.push(text);
          return Promise.resolve();
        },
      },
    });
  });
}

/** The BOOT JSON of a served page. */
function bootOf(html: string): State {
  const at = html.indexOf('id="birta-boot"');
  const open = html.indexOf(">", at) + 1;
  return JSON.parse(html.slice(open, html.indexOf("</script>", open)));
}

const copied = (page: Page) => page.evaluate(() => (window as unknown as { __copied: string[] }).__copied);

/** Text of the shadow-rooted host `sel`, or null while it isn't shown. */
function hostText(page: Page, sel: string): Promise<string | null> {
  return page.evaluate((sel) => {
    const h = document.querySelector(sel);
    if (!h || !h.getClientRects().length) return null;
    let out = "";
    const walk = (x: Node) => {
      if (x.nodeType === 3) out += x.textContent;
      else if (x.nodeType === 1 && (x as Element).localName !== "style") x.childNodes.forEach(walk);
    };
    (h.shadowRoot ?? h).childNodes.forEach(walk);
    return out.replace(/\s+/g, " ").trim();
  }, sel);
}
const phraseOf = (page: Page) => hostText(page, ".file-header birta-phrase");

/** The pill's text while it shows, else null. */
function pillOf(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    for (const host of Array.from(document.querySelectorAll("birta-pop"))) {
      for (const el of Array.from((host.shadowRoot ?? host).querySelectorAll('[role="status"]'))) {
        let open = false;
        try {
          open = el.matches(":popover-open");
        } catch {
          // no popover support
        }
        const text = (el.textContent ?? "").replace(/\s+/g, " ").trim();
        if (open && text) return text;
      }
    }
    return null;
  });
}

/** The phrase menu's rows, opened by a click. */
async function menuRows(page: Page): Promise<string[]> {
  await page.locator(".file-header birta-phrase").click();
  return page.evaluate(() => {
    for (const host of Array.from(document.querySelectorAll("birta-pop"))) {
      const menu = (host.shadowRoot ?? host).querySelector('[role="menu"]');
      if (menu && menu.matches(":popover-open"))
        return Array.from(menu.querySelectorAll('[role="menuitem"]')).map((r) => (r.textContent ?? "").replace(/\s+/g, " ").trim());
    }
    return [];
  });
}

const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"] textarea');

async function keep(page: Page, body: string) {
  await page.keyboard.press("c");
  await expect(composer(page)).toBeFocused();
  await page.keyboard.type(body);
  await page.keyboard.press("Enter");
  await expect(composer(page)).toHaveCount(0);
}

const comments = (root: string, rel: string) => (reviewLog(root, rel) ?? []).filter((l: State) => l.op === "comment");

/** Runs the agent CLI in the served folder. */
function agent(s: Site, args: string[]) {
  const r = spawnSync(BIN, args, {
    cwd: s.root,
    env: {
      PATH: process.env.PATH ?? "",
      HOME: path.join(s.base, "home"),
      XDG_STATE_HOME: path.join(s.base, "state"),
      BIRTA_REVIEW: "1",
    },
    encoding: "utf-8",
    timeout: 30_000,
  });
  expect(r.status, r.stderr).toBe(0);
}

/** Every `dir_state` the page's sockets receive. */
function dirStates(page: Page): State[] {
  const out: State[] = [];
  page.on("websocket", (ws) => {
    ws.on("framereceived", (f) => {
      try {
        const msg = JSON.parse(typeof f.payload === "string" ? f.payload : f.payload.toString());
        if (msg.type === "dir_state") out.push(msg);
      } catch {
        // not a JSON frame
      }
    });
  });
  return out;
}

const listsPath = (d: State, p: string) => (d.files ?? []).some((f: State) => (f.paths ?? []).includes(p));

// --- Stop and reconnect ---------------------------------------------------------------------

/** A path as a POSIX shell reads it back: single-quoted, with each `'` closed, escaped and reopened. */
const quoted = (p: string) => `'${p.replace(/'/g, `'\\''`)}'`;
// The specs turn review on by env, so a restart has to carry it
const GATE_PREFIX = "BIRTA_REVIEW=1 ";
const restartLine = (served: string) => `${GATE_PREFIX}birta ${quoted(served)} --no-open`;
const bannerText = (served: string) => `birta stopped · run ${GATE_PREFIX}birta ${quoted(served)} to reconnect`;

/** Times of every `whoami` probe the page makes. */
function whoamiTimes(page: Page): number[] {
  const out: number[] = [];
  page.on("request", (r) => {
    if (r.url().endsWith("/birta/whoami")) out.push(Date.now());
  });
  return out;
}

/** Counts the sockets the page opens. */
function sockets(page: Page): { n: number } {
  const out = { n: 0 };
  page.on("websocket", () => out.n++);
  return out;
}

test.describe("a stopped server", () => {
  test("shows how to restart it, and the tab reconnects to the restart with its state", async ({ page }) => {
    test.setTimeout(90_000);
    const s = tracked({});
    // A space, so the copied command has to quote the path
    const name = await freeName(s, "my pr", ".md");
    fs.writeFileSync(path.join(s.root, name), DOC);
    const doc = path.join(s.root, name);
    await captureClipboard(page);
    const probes = whoamiTimes(page);
    const first = await launch(s, [name], GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${first.url}/view/${encodeURIComponent(name)}`);
    await booted(page);
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await keep(page, "way too verbose");
    await expect.poll(() => phraseOf(page)).toBe("your turn · 1 note");
    await expect(page.getByText("way too verbose")).toBeVisible();
    const at = await focused(page);
    expect(at).not.toBeNull();
    const before = await page.evaluate("BOOT.review.token");

    await kill(first);
    const banner = page.getByText(bannerText(doc), { exact: true });
    await expect(banner).toBeVisible({ timeout: 3_000 });
    await page.getByRole("button", { name: "copy", exact: true }).click();
    await expect.poll(() => copied(page)).toEqual([restartLine(doc)]);
    expect(first.port).toBe(derived(doc));

    // Long enough down that a doubling backoff would wait past the 2 s the tab promises
    const downFrom = Date.now();
    await page.waitForTimeout(8_000);
    const down = probes.filter((t) => t >= downFrom);
    expect(down.length).toBeGreaterThan(2);
    const gaps = down.slice(1).map((t, i) => t - down[i]);
    expect(Math.max(...gaps, Date.now() - down[down.length - 1])).toBeLessThanOrEqual(2_200);

    const again = await launchLine(s, (await copied(page))[0], {}, s.base);
    expect(again.port).toBe(first.port);

    await expect(banner).toBeHidden({ timeout: 3_000 });
    await expect.poll(() => pillOf(page), { timeout: 3_000 }).toBe("reconnected");
    expect(await phraseOf(page)).toBe("your turn · 1 note");
    expect(await focused(page)).toBe(at);
    await expect(page.getByText("way too verbose")).toBeVisible();

    // A beacon from here carries the restarted server's token
    const token = await page.evaluate("BOOT.review.token");
    expect(token).not.toBe(before);
    const fresh = await (await fetch(`${again.url}/view/${encodeURIComponent(name)}`)).text();
    expect(token).toBe(bootOf(fresh).review.token);
    const flushed = await page.evaluate(`fetch('/review/flush', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ token: BOOT.review.token, tab: birtaBus.tab(), path: CURRENT_PATH, seen: [], ops: [] }),
    }).then((r) => r.status)`);
    expect(flushed).not.toBe(403);

    // The restarted server takes the page's ops
    await page.keyboard.press("j");
    await keep(page, "and this one");
    await expect.poll(() => comments(s.root, name).map((l: State) => l.body)).toEqual(["way too verbose", "and this one"]);
  });

  test("never reconnects to another server on its port, and comes back once its own is", async ({ page }) => {
    test.setTimeout(60_000);
    const s = tracked({ "other.md": "# Someone else\n" });
    const name = await freeName(s, "pr", ".md");
    fs.writeFileSync(path.join(s.root, name), DOC);
    const doc = path.join(s.root, name);
    const first = await launch(s, [name], GATE_ON);
    const opened = sockets(page);
    await page.goto(`${first.url}/view/${name}`);
    await booted(page);
    await kill(first);
    const banner = page.getByText(bannerText(doc), { exact: true });
    await expect(banner).toBeVisible({ timeout: 3_000 });
    const before = opened.n;

    const foreign = await launch(s, ["other.md", "--port", String(first.port)], GATE_ON);
    await expect.poll(() => pillOf(page), { timeout: 5_000 }).toBe("another birta is on this port");
    await page.waitForTimeout(4_000);
    await expect(banner).toBeVisible();
    expect(await pillOf(page)).not.toBe("reconnected");
    expect(opened.n).toBe(before);
    await expect(page.locator("#content h1")).toHaveText("Upload retries");
    expect(new URL(page.url()).port).toBe(String(first.port));
    await kill(foreign);

    // Not a birta at all
    const stranger = net.createServer((c) => {
      c.on("data", () => c.end("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"));
    });
    await new Promise<void>((r) => stranger.listen(first.port, "127.0.0.1", () => r()));
    await page.waitForTimeout(3_000);
    await expect(banner).toBeVisible();
    expect(opened.n).toBe(before);
    await new Promise<void>((r) => stranger.close(() => r()));

    const again = await launch(s, [name], GATE_ON);
    expect(again.port).toBe(first.port);
    await expect(banner).toBeHidden({ timeout: 3_000 });
    await expect.poll(() => pillOf(page), { timeout: 3_000 }).toBe("reconnected");
  });

  test("names the served file, not the doc the tab moved to", async ({ page }) => {
    test.setTimeout(60_000);
    const s = tracked({ "docs/plan.md": DOC });
    const name = await freeName(s, "pr", ".md");
    fs.writeFileSync(path.join(s.root, name), "# PR\n\nSee [the plan](docs/plan.md).\n");
    const doc = path.join(s.root, name);
    await captureClipboard(page);
    const first = await launch(s, [name], GATE_ON);
    await page.goto(`${first.url}/view/${name}`);
    await page.getByRole("link", { name: "the plan" }).click();
    await expect(page).toHaveURL(/\/view\/docs\/plan\.md$/);
    await booted(page);
    await kill(first);
    const banner = page.getByText(bannerText(doc), { exact: true });
    await expect(banner).toBeVisible({ timeout: 3_000 });
    await page.getByRole("button", { name: "copy", exact: true }).click();
    await expect.poll(() => copied(page)).toEqual([restartLine(doc)]);
    const again = await launchLine(s, (await copied(page))[0], {}, s.base);
    expect(again.port).toBe(first.port);
    await expect(banner).toBeHidden({ timeout: 3_000 });
    await expect.poll(() => pillOf(page), { timeout: 3_000 }).toBe("reconnected");
    await expect(page).toHaveURL(/\/view\/docs\/plan\.md$/);
  });

  test("in folder mode names the folder, quoted so a shell runs it as written", async ({ page }) => {
    test.setTimeout(60_000);
    const s = tracked({});
    const folder = await freeName(s, "a b'$(touch pwned)", "");
    fs.mkdirSync(path.join(s.root, folder));
    fs.writeFileSync(path.join(s.root, folder, "index.md"), DOC);
    const dir = path.join(s.root, folder);
    await captureClipboard(page);
    const first = await launch(s, [folder], GATE_ON);
    await page.goto(`${first.url}/view/index.md`);
    await booted(page);
    await kill(first);
    const banner = page.getByText(bannerText(dir), { exact: true });
    await expect(banner).toBeVisible({ timeout: 3_000 });
    expect(first.port).toBe(derived(dir));
    await page.getByRole("button", { name: "copy", exact: true }).click();
    await expect.poll(() => copied(page)).toEqual([restartLine(dir)]);
    const line = (await copied(page))[0];
    expect(line.endsWith(" --no-open")).toBe(true);
    const again = await launchLine(s, line, {}, s.base);
    expect(again.port).toBe(first.port);
    await expect(banner).toBeHidden({ timeout: 3_000 });
    await expect.poll(() => pillOf(page), { timeout: 3_000 }).toBe("reconnected");
    for (const where of [s.base, s.root, dir]) expect(fs.existsSync(path.join(where, "pwned"))).toBe(false);
  });

  test("with review off shows no banner", async ({ page }) => {
    const s = tracked({ "pr.md": DOC });
    const first = await launch(s, ["pr.md"], {});
    await page.goto(`${first.url}/view/pr.md`);
    await expect(page.locator("#content")).toBeVisible();
    await kill(first);
    await expect(page.locator("#status")).toContainText("Disconnected", { timeout: 3_000 });
    await expect(page.getByText(/birta stopped/)).toHaveCount(0);
  });
});

// --- What a single-file server lists, and the folder breadcrumb --------------------------------

test("a single-file server lists no reviewed doc it never served under other files", async ({ page, birta }) => {
  const served = await birta({ "pr.md": DOC, "docs/plan.md": DOC }, GATE_ON);
  const s = { base: path.dirname(served.root), root: served.root };
  agent(s, ["comment", "pr.md", "--doc", "--as", "claude", "is this the final title?"]);
  agent(s, ["comment", "docs/plan.md", "--doc", "--as", "claude", "is this the final plan?"]);
  const dirs = dirStates(page);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  // The listing has been made: it names this page's own doc
  await expect.poll(() => dirs.some((d) => listsPath(d, "pr.md"))).toBe(true);
  expect(dirs.filter((d) => listsPath(d, "docs/plan.md"))).toEqual([]);
  await expect.poll(() => phraseOf(page)).not.toBeNull();
  expect(await phraseOf(page)).not.toMatch(/other file/);
  expect((await menuRows(page)).filter((r) => r.includes("plan.md"))).toEqual([]);
  expect(dirs.filter((d) => listsPath(d, "docs/plan.md"))).toEqual([]);
});

test("in folder mode the header names a nested doc by its path in the folder", async ({ page, birta }) => {
  const served = await birta(
    { "index.md": "# Index\n\nSee the [checklist](linked/checklist.md).\n", "linked/checklist.md": "# Checklist\n" },
    GATE_ON,
    ["."],
  );
  await page.goto(`${served.url}/view/index.md`);
  await expect(page.locator(".file-header-name")).toHaveText("index.md");
  await page.getByRole("link", { name: "checklist" }).click();
  await expect(page).toHaveURL(/\/view\/linked\/checklist\.md$/);
  await expect(page.locator(".file-header-name")).toHaveText("linked/checklist.md");
});
