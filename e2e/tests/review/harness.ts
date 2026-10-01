import { test as base, expect, Page, WebSocketRoute } from "@playwright/test";
import { spawn, spawnSync, ChildProcess } from "child_process";
import * as crypto from "crypto";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as zlib from "zlib";

export { expect };

export const REPO = path.resolve(__dirname, "..", "..", "..");
export const BIN = path.join(REPO, "target", "debug", "birta");
const FIXTURES = path.join(REPO, "e2e", "fixtures", "review");

export const GATE_ON: Record<string, string> = { BIRTA_REVIEW: "1" };
/** Review code boots within a frame or two of load; a missing boot should fail fast. */
export const BOOT_TIMEOUT = 3_000;

// --- Fixtures -----------------------------------------------------------------------------

// Review-state JSON is loosely typed on purpose: specs clone and bend committed fixtures.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type State = any;

export function fixtureDoc(name: string): string {
  return fs.readFileSync(path.join(FIXTURES, `${name}.md`), "utf-8");
}

/** A deep copy of a committed state, so a spec can bend it without touching the others. */
export function fixtureState(name: string): State {
  return JSON.parse(fs.readFileSync(path.join(FIXTURES, `${name}.json`), "utf-8"));
}

/** `git hash-object` of the text, which is how birta names a doc revision. */
export function revOf(text: string): string {
  const bytes = Buffer.from(text, "utf-8");
  return crypto
    .createHash("sha1")
    .update(Buffer.concat([Buffer.from(`blob ${bytes.length}\0`), bytes]))
    .digest("hex");
}

export function withRev(state: State, rev: string): State {
  return { ...JSON.parse(JSON.stringify(state)), rev };
}

/** Sourcepos of the single-line block holding `text`, as comrak writes it. */
export function spAt(doc: string, text: string): string {
  const lines = doc.split("\n");
  const i = lines.findIndex((l) => l.includes(text));
  if (i < 0) throw new Error(`no line holds ${JSON.stringify(text)}`);
  return `${i + 1}:1-${i + 1}:${Buffer.byteLength(lines[i], "utf-8")}`;
}

/** Every kind of stop, a nested list, a raw HTML block and a subsection, for walking focus. */
export const STOPS_DOC = [
  "# Upload retries",
  "",
  "## Why",
  "",
  "When the upload hits a 503 we just fail and the user has to start again.",
  "Retrying three times with backoff fixes most of these.",
  "",
  "## What changes",
  "",
  "- retries on 503",
  "- backoff of 1, 2 and 4 seconds",
  "  - capped at 8 seconds",
  "  - with jitter",
  "- metrics dashboard",
  "",
  "| code | retry |",
  "|------|-------|",
  "| 503  | yes   |",
  "| 404  | no    |",
  "",
  "<div>raw html block</div>",
  "",
  "```js",
  "retry(upload, 3);",
  "```",
  "",
  "### Limits",
  "",
  "Detail words.",
  "",
  "## Stacked on",
  "",
  "Nothing yet.",
  "",
].join("\n");

/** `STOPS_DOC` with `n` filler paragraphs after it, so the page scrolls. */
export function longDoc(n = 40): string {
  return STOPS_DOC + Array.from({ length: n }, (_, i) => `\nFiller paragraph ${i + 1}.\n`).join("");
}

/** The page's focus stops, in order, from the boot data. */
export function stops(page: Page): Promise<{ sp: string; kind: string }[]> {
  return page.evaluate(() =>
    ((window as unknown as { BOOT: { stops?: { sp: string; kind: string }[] } }).BOOT.stops ?? []).map((s) => ({
      sp: s.sp,
      kind: s.kind,
    })),
  );
}

/** The sp of the last stop whose element's text holds `text`, so a nested item beats its parent. */
export async function stopSp(page: Page, text: string): Promise<string> {
  const all = await stops(page);
  const sp = await page.evaluate(
    ({ sps, text }) => {
      let hit: string | null = null;
      for (const sp of sps) {
        const els = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
        const el = els[els.length - 1];
        if (el && (el.textContent ?? "").includes(text)) hit = sp;
      }
      return hit;
    },
    { sps: all.map((s) => s.sp), text },
  );
  if (!sp) throw new Error(`no stop holds ${JSON.stringify(text)}`);
  return sp;
}

export interface Lane {
  bars?: { sp: string; asked?: "location" | "cited" | "unasked"; attr?: "agent" | "human" | "outside"; yourEdit?: boolean; seen?: boolean }[];
  glyphs?: { sp: string; verdict: string; status?: string }[];
  ticks?: { after: string | null }[];
}

/**
 * A state for `doc` holding only the given marks, built from the unseen fixture's own change,
 * item and deletion so every field keeps its real shape.
 */
export function laneState(doc: string, lane: Lane): State {
  const s = fixtureState("unseen");
  const change = s.changes[0];
  const item = s.items[1];
  const deletion = s.deletions.find((d: State) => d.kind === "block");
  s.rev = revOf(doc);
  s.changes = (lane.bars ?? []).map((b, i) => ({
    ...JSON.parse(JSON.stringify(change)),
    id: `${20 + i}.0`,
    op: 20 + i,
    attr: b.attr ?? "agent",
    by: b.attr === "human" ? { type: "human", via: "browser" } : change.by,
    asked: { kind: b.asked ?? "location", notes: [] },
    range: { ...change.range, sp: b.sp },
    seen: b.seen ?? false,
    ...(b.yourEdit ? { changes_your_edit: "3.0" } : {}),
  }));
  s.items = (lane.glyphs ?? []).map((g, i) => ({
    ...JSON.parse(JSON.stringify(item)),
    n: 40 + i,
    id: undefined,
    cid: `g-${i}`,
    verdict: g.verdict,
    status: g.status ?? "draft",
    at: { ...item.at, loc: { sp: g.sp } },
  }));
  s.deletions = (lane.ticks ?? []).map((t, i) => ({ ...deletion, change: `${60 + i}.0`, after_sp: t.after }));
  s.rewrite = null;
  return s;
}

// --- Servers ------------------------------------------------------------------------------

let built = false;

/** Review assets are embedded at compile time, so the binary must match the tree. */
function ensureBuilt(): void {
  if (built) return;
  const r = spawnSync("cargo", ["build", "--quiet"], { cwd: REPO, stdio: "inherit" });
  if (r.status !== 0) throw new Error("cargo build failed");
  built = true;
}

export interface Served {
  url: string;
  /** The served folder. */
  root: string;
  write(rel: string, text: string): void;
  stop(): void;
}

/**
 * Runs `target/debug/birta` over a fresh copy of `files` on a port the OS picks. `args`
 * defaults to the first file. HOME and XDG_STATE_HOME are private, so no user config applies.
 * `links` maps a path under the root to its symlink target; `../x` files land beside the root.
 */
export async function startBirta(
  files: Record<string, string>,
  env: Record<string, string> = {},
  args?: string[],
  links: Record<string, string> = {},
): Promise<Served> {
  ensureBuilt();
  // realpath: the watcher compares canonical paths and macOS tmpdir is a symlink
  const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "birta-e2e-review-")));
  const root = path.join(base, "root");
  fs.mkdirSync(root, { recursive: true });
  for (const [rel, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.writeFileSync(path.join(root, rel), text);
  }
  for (const [rel, target] of Object.entries(links)) {
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.symlinkSync(target, path.join(root, rel));
  }
  const proc: ChildProcess = spawn(
    BIN,
    [...(args ?? [Object.keys(files)[0]]), "--no-open", "--port", "0"],
    {
      cwd: root,
      env: {
        PATH: process.env.PATH ?? "",
        HOME: path.join(base, "home"),
        XDG_STATE_HOME: path.join(base, "state"),
        ...env,
      },
      stdio: ["ignore", "ignore", "pipe"],
    },
  );
  const stop = () => {
    proc.kill("SIGKILL");
    fs.rmSync(base, { recursive: true, force: true });
  };
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
  }).catch((e) => {
    stop();
    throw e;
  });
  return {
    url,
    root,
    write: (rel, text) => fs.writeFileSync(path.join(root, rel), text),
    stop,
  };
}

// --- CDN cache ----------------------------------------------------------------------------

const cdn = new Map<string, { status: number; headers: Record<string, string>; body: Buffer }>();

/** Serves the pinned CDN scripts from memory after the first fetch; bytes are kept for SRI. */
export async function cacheCdn(page: Page): Promise<void> {
  await page.route(/^https:\/\/cdn\.jsdelivr\.net\//, async (route) => {
    const url = route.request().url();
    let hit = cdn.get(url);
    if (!hit) {
      const res = await route.fetch();
      const headers = { ...res.headers() };
      // The body is already decoded, so its transfer headers no longer apply
      for (const h of ["content-encoding", "content-length", "transfer-encoding"]) delete headers[h];
      hit = { status: res.status(), headers, body: await res.body() };
      if (res.ok()) cdn.set(url, hit);
    }
    await route.fulfill(hit);
  });
}

// --- Socket mock --------------------------------------------------------------------------

export interface Frame {
  /** Which page socket sent it, counting from 0 in open order. */
  socket: number;
  msg: State;
}

export interface MockOptions {
  /** Pushed after each `content` frame whose rev equals the state's rev. */
  state?: State;
  states?: State[];
  /** Pushed as-is after each `content` frame. */
  dir?: State;
  /** `auto` acks every op; `manual` leaves it to the spec. */
  acks?: "auto" | "manual";
}

export interface Mock {
  /** Every frame the page sent, in order. */
  frames: Frame[];
  ops(): State[];
  /** The path of the latest `content` the page was sent. */
  path: string;
  /** While set, the server's `content` frames, and theme updates that carry the doc, never reach the page. */
  holdContent: boolean;
  push(msg: State): void;
  pushState(state: State, path?: string): void;
  ack(cid: string): void;
  nack(cid: string, reason: string): void;
  /** Closes every page socket, as a server restart would. */
  drop(): Promise<void>;
  /** Called after each state the mock pushes. */
  onStatePushed?: () => void;
}

/**
 * Routes every page socket through a proxy to the real server. The page's review frames
 * (`op`, `resolve`, `compose`) stop here, and the server's review frames are replaced by
 * the given state. Call before `page.goto`.
 */
export async function mockReview(page: Page, opts: MockOptions = {}): Promise<Mock> {
  const states = [...(opts.states ?? []), ...(opts.state ? [opts.state] : [])];
  const open: WebSocketRoute[] = [];
  let acked = 0;
  const mock: Mock = {
    frames: [],
    ops: () => mock.frames.filter((f) => f.msg.type === "op").map((f) => f.msg),
    path: "",
    holdContent: false,
    push(msg) {
      for (const ws of open) ws.send(JSON.stringify(msg));
    },
    pushState(state, p) {
      mock.push({ type: "review_state", path: p ?? mock.path, rev: state.rev, state });
      mock.onStatePushed?.();
    },
    ack(cid) {
      mock.push({ type: "ack", cid, n: ++acked + 100 });
    },
    nack(cid, reason) {
      mock.push({ type: "nack", cid, reason });
    },
    async drop() {
      const all = open.splice(0);
      await Promise.all(all.map((ws) => ws.close()));
    },
  };
  let sockets = 0;
  await page.routeWebSocket(/\/ws$/, (ws) => {
    const socket = sockets++;
    const server = ws.connectToServer();
    open.push(ws);
    ws.onClose(() => {
      const i = open.indexOf(ws);
      if (i >= 0) open.splice(i, 1);
      server.close();
    });
    ws.onMessage((raw) => {
      const msg = JSON.parse(raw.toString());
      mock.frames.push({ socket, msg });
      if (msg.type === "op" || msg.type === "resolve" || msg.type === "compose") {
        if (msg.type === "op" && (opts.acks ?? "auto") === "auto") {
          ws.send(JSON.stringify({ type: "ack", cid: msg.cid, n: ++acked + 100 }));
        }
        return;
      }
      server.send(raw);
    });
    server.onMessage((raw) => {
      const msg = JSON.parse(raw.toString());
      if (["review_state", "dir_state", "compose_restore"].includes(msg.type)) return;
      // A page whose OS variant differs syncs it on open; the answer can land late and carry the doc
      if (msg.type === "theme_update" && msg.html != null && mock.holdContent) return;
      if (msg.type === "content") {
        if (mock.holdContent) return;
        ws.send(raw);
        mock.path = msg.path;
        const state = states.find((s) => s.rev === msg.rev);
        if (state) {
          ws.send(JSON.stringify({ type: "review_state", path: msg.path, rev: state.rev, state }));
          mock.onStatePushed?.();
        }
        if (opts.dir) ws.send(JSON.stringify(opts.dir));
        return;
      }
      ws.send(raw);
    });
  });
  return mock;
}

// --- Page probes --------------------------------------------------------------------------

/** Passes once review code has booted: the page is marked and both lane hosts exist. */
export async function booted(page: Page): Promise<void> {
  await expect(page.locator("html[data-review]")).toBeAttached({ timeout: BOOT_TIMEOUT });
  await expect(page.locator("#lane-frame > birta-under")).toBeAttached({ timeout: BOOT_TIMEOUT });
  await expect(page.locator("#lane-frame > birta-over")).toBeAttached({ timeout: BOOT_TIMEOUT });
}

export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export interface Mark extends Box {
  mark: string;
  sp: string | null;
  after: string | null;
  style: string | null;
  role: string | null;
  verdict: string | null;
  n: number | null;
  count: number | null;
  /** What a focus mark stands on when it is not a stop, e.g. `summary`. */
  target: string | null;
}

/** Every `[data-mark]` element in the lane's shadow roots (forced open by the harness). */
export function marks(page: Page): Promise<Mark[]> {
  return page.evaluate(() => {
    const out: Mark[] = [];
    for (const tag of ["birta-under", "birta-over"]) {
      const root = document.querySelector(`#lane-frame > ${tag}`)?.shadowRoot;
      if (!root) continue;
      root.querySelectorAll("[data-mark]").forEach((el) => {
        const r = el.getBoundingClientRect();
        const a = (n: string) => el.getAttribute(n);
        out.push({
          mark: a("data-mark")!,
          sp: a("data-sp"),
          after: a("data-after"),
          style: a("data-style"),
          role: a("data-role"),
          verdict: a("data-verdict"),
          n: a("data-n") === null ? null : Number(a("data-n")),
          count: a("data-count") === null ? null : Number(a("data-count")),
          target: a("data-target"),
          left: r.left,
          top: r.top,
          right: r.right,
          bottom: r.bottom,
          width: r.width,
          height: r.height,
        });
      });
    }
    return out;
  });
}

/** `mark sp style|verdict` keys of the drawn bars, glyphs, washes and ticks, sorted. */
export async function markKeys(page: Page): Promise<string[]> {
  return (await marks(page))
    .filter((m) => ["bar", "glyph", "wash", "tick"].includes(m.mark))
    .map((m) =>
      m.mark === "tick"
        ? `tick after ${m.after || "top"}`
        : [m.mark, m.sp, m.style ?? m.verdict].filter(Boolean).join(" "),
    )
    .sort();
}

export interface BlockBox extends Box {
  /** The first line box of the block's text. */
  line: { top: number; bottom: number };
  /** The enclosing list's box, for items. */
  list: Box | null;
}

/** The deepest element carrying `sp`, which is the item when a list shares its first item's sp. */
export function blockBox(page: Page, sp: string): Promise<BlockBox | null> {
  return page.evaluate((sp) => {
    const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
    const el = all[all.length - 1];
    if (!el) return null;
    const box = (r: DOMRect) => ({
      left: r.left,
      top: r.top,
      right: r.right,
      bottom: r.bottom,
      width: r.width,
      height: r.height,
    });
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => ((n.textContent || "").trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP),
    });
    const text = walker.nextNode();
    let line = { top: el.getBoundingClientRect().top, bottom: el.getBoundingClientRect().bottom };
    if (text) {
      const range = document.createRange();
      range.selectNodeContents(text);
      const first = range.getClientRects()[0];
      if (first) line = { top: first.top, bottom: first.bottom };
    }
    const list = el.tagName === "LI" ? el.parentElement!.getBoundingClientRect() : null;
    // Notes that close an item are not the item's own lines
    const own = el.getBoundingClientRect();
    let first: Element | null = null;
    for (let c = el.lastElementChild; c && c.localName === "birta-flow"; c = c.previousElementSibling) first = c;
    if (first) {
      const range = document.createRange();
      range.setStart(el, 0);
      range.setEndBefore(first);
      const bottom = range.getBoundingClientRect().bottom;
      return { ...box(own), bottom, height: bottom - own.top, line, list: list ? box(list) : null };
    }
    return { ...box(own), line, list: list ? box(list) : null };
  }, sp);
}

export function frameBox(page: Page): Promise<Box> {
  return page.evaluate(() => {
    const r = document.getElementById("lane-frame")!.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
  });
}

/**
 * Viewport box of the block after `sp`: its next rendered sibling, or its nearest ancestor's,
 * so a tick after a list item sits before the next item.
 */
export function nextBlockBox(page: Page, sp: string | null): Promise<Box | null> {
  return page.evaluate((sp) => {
    const content = document.getElementById("content")!;
    const skip = (k: Element | null) => {
      while (k && (k.localName.startsWith("birta-") || k.localName === "style" || !k.getClientRects().length))
        k = k.nextElementSibling;
      return k;
    };
    let next: Element | null = null;
    if (sp === null) next = skip(content.firstElementChild);
    else {
      const all = content.querySelectorAll(`[data-sourcepos="${sp}"]`);
      for (let at: Element | null = all[all.length - 1] ?? null; at && at !== content && !next; at = at.parentElement)
        next = skip(at.nextElementSibling);
    }
    if (!next) return null;
    const r = next.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
  }, sp);
}

export function contentBox(page: Page): Promise<Box> {
  return page.evaluate(() => {
    const r = document.getElementById("content")!.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
  });
}

/** Lane centre and glyph size from the frame's left edge, per the viewport width. */
export async function laneGeometry(page: Page): Promise<{ x: number; glyph: number }> {
  const w = await page.evaluate(() => window.innerWidth);
  return w < 768 ? { x: 8, glyph: 12 } : { x: 20, glyph: 16 };
}

/** Whether two boxes share any area; touching edges don't count. */
export function overlaps(a: Box, b: Box): boolean {
  return a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;
}

/**
 * What is out of place in the lane: bars and washes must span their block (±1px), glyphs sit
 * on its first line, ticks sit in the gap after their block, all centred on the lane.
 */
export async function misaligned(page: Page): Promise<string[]> {
  const frame = await frameBox(page);
  const content = await contentBox(page);
  const lane = await laneGeometry(page);
  const laneX = frame.left + lane.x;
  const out: string[] = [];
  const near = (a: number, b: number) => Math.abs(a - b) <= 1;
  const all = await marks(page);
  // Glyphs on one block stack down from its first line, in note order
  const stack = (m: Mark) =>
    all.filter((g) => g.mark === "glyph" && g.sp === m.sp && (g.n ?? 0) < (m.n ?? 0)).length * (lane.glyph + 2);
  for (const m of all) {
    const what = `${m.mark} ${m.sp ?? m.after ?? "top"}`;
    if (m.mark === "bar" || m.mark === "glyph" || m.mark === "wash") {
      const b = await blockBox(page, m.sp!);
      if (!b) {
        out.push(`${what}: no block`);
        continue;
      }
      if (m.mark === "bar") {
        const glyphs = all.filter((g) => g.mark === "glyph" && g.sp === m.sp);
        if (!near(m.top, b.top) || !near(m.bottom, b.bottom))
          out.push(`${what}: ${m.top}-${m.bottom} vs block ${b.top}-${b.bottom}`);
        // Glyphs on the same block sit beside the bar. The wide lane has room to keep the bar
        // on its centre line; the narrow one may shift it to fit both.
        for (const g of glyphs)
          if (Math.max(m.left - g.right, g.left - m.right) < 2) out.push(`${what}: not beside glyph ${g.verdict}`);
        if (glyphs.length && lane.x !== 20) {
          if (m.left < frame.left - 1 || m.right > content.left + 1) out.push(`${what}: ${m.left}-${m.right} outside the lane`);
        } else if (!near((m.left + m.right) / 2, laneX)) {
          out.push(`${what}: centre ${(m.left + m.right) / 2} vs lane ${laneX}`);
        }
      } else if (m.mark === "glyph") {
        const cy = (m.top + m.bottom) / 2 - stack(m);
        if (cy < b.line.top || cy > b.line.bottom) out.push(`${what}: centre ${cy} outside line ${b.line.top}-${b.line.bottom}`);
        if (all.some((o) => o.mark === "bar" && o.sp === m.sp)) {
          if (m.left < frame.left - 1 || m.right > content.left + 1) out.push(`${what}: ${m.left}-${m.right} outside the lane`);
        } else if (!near((m.left + m.right) / 2, laneX)) {
          out.push(`${what}: centre x ${(m.left + m.right) / 2} vs lane ${laneX}`);
        }
      } else {
        // A wash reaches over an item's marker, and stops at the doc's edge
        const left = Math.max((b.list ?? b).left, content.left) - 8;
        const right = Math.min(b.right, content.right) + 8;
        if (!near(m.top, b.top) || !near(m.bottom, b.bottom)) out.push(`${what}: ${m.top}-${m.bottom} vs block ${b.top}-${b.bottom}`);
        if (!near(m.left, left) || !near(m.right, right)) out.push(`${what}: ${m.left}-${m.right} vs ${left}-${right}`);
      }
    } else if (m.mark === "tick") {
      const after = m.after ? await blockBox(page, m.after) : null;
      const next = await nextBlockBox(page, m.after || null);
      const top = after ? after.bottom : frame.top;
      const bottom = next ? next.top : frame.bottom;
      if (m.top < top - 1 || m.bottom > bottom + 1) out.push(`${what}: ${m.top}-${m.bottom} outside gap ${top}-${bottom}`);
      if (after && next && !near((m.top + m.bottom) / 2, (top + bottom) / 2))
        out.push(`${what}: centre ${(m.top + m.bottom) / 2} vs gap centre ${(top + bottom) / 2}`);
      if (!near((m.left + m.right) / 2, laneX)) out.push(`${what}: centre x ${(m.left + m.right) / 2} vs lane ${laneX}`);
    }
  }
  return out;
}

// --- Keys, focus and the review file ------------------------------------------------------

/**
 * The sp of the focused stop, `summary` on a closed details' summary, or null while focus is
 * on the doc. Read from the `focus` mark, which stays in the lane's root while it is hidden.
 */
export async function focused(page: Page): Promise<string | null> {
  const found = (await marks(page)).filter((m) => m.mark === "focus");
  if (found.length > 1) throw new Error(`${found.length} focus marks`);
  return found[0]?.sp ?? found[0]?.target ?? null;
}

export async function focusMark(page: Page): Promise<Mark | null> {
  return (await marks(page)).find((m) => m.mark === "focus") ?? null;
}

export interface KeyInit {
  key: string;
  code?: string;
  shiftKey?: boolean;
  altKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
  repeat?: boolean;
  isComposing?: boolean;
  /** Set on the event itself, since no engine takes it from the init dictionary. */
  keyCode?: number;
}

/**
 * Dispatches a keydown (and keyup) the keyboard can't produce: Caps Lock letters, IME
 * composition, key repeat. `target` is a selector, or the focused element.
 */
export async function dispatchKey(page: Page, init: KeyInit, target?: string): Promise<boolean> {
  return page.evaluate(
    ({ init, target }) => {
      const el = (target ? document.querySelector(target) : document.activeElement) ?? document.body;
      const fire = (type: string) => {
        const { keyCode, ...rest } = init;
        const e = new KeyboardEvent(type, { bubbles: true, cancelable: true, composed: true, ...rest });
        if (keyCode !== undefined) Object.defineProperty(e, "keyCode", { value: keyCode });
        el.dispatchEvent(e);
        return e.defaultPrevented;
      };
      const prevented = fire("keydown");
      fire("keyup");
      return prevented;
    },
    { init, target },
  );
}

/** Every frame the page sends, for pages whose socket is not mocked. Call before `goto`. */
export function watchSent(page: Page): State[] {
  const sent: State[] = [];
  page.on("websocket", (ws) => {
    ws.on("framesent", (f) => {
      try {
        sent.push(JSON.parse(typeof f.payload === "string" ? f.payload : f.payload.toString()));
      } catch {
        // not a JSON frame
      }
    });
  });
  return sent;
}

/** The parsed lines of `rel`'s review file under `root`, or null while there is none. */
export function reviewLog(root: string, rel = "pr.md"): State[] | null {
  const file = path.join(root, `${rel}.birta.jsonl`);
  if (!fs.existsSync(file)) return null;
  return fs
    .readFileSync(file, "utf-8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l));
}

/** Files under the served folder's drafts state dir (half-typed notes). */
export function draftFiles(root: string): string[] {
  const dir = path.join(root, "..", "state", "birta", "drafts");
  if (!fs.existsSync(dir)) return [];
  return (fs.readdirSync(dir, { recursive: true }) as string[]).filter((f) =>
    fs.statSync(path.join(dir, f)).isFile(),
  );
}

/** The text of every range in every CSS highlight on the page, whatever its name. */
export function highlighted(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const out: string[] = [];
    const registry = (CSS as unknown as { highlights?: Map<string, Set<AbstractRange>> }).highlights;
    if (!registry) return out;
    for (const [, hl] of registry) for (const r of hl) out.push((r as Range).toString());
    return out.sort();
  });
}

// --- Pixels -------------------------------------------------------------------------------

export type Rgb = [number, number, number];

export interface Shot {
  left: number;
  top: number;
  width: number;
  height: number;
  /** The colour at viewport point (x, y), which must lie inside the shot. */
  at(x: number, y: number): Rgb;
}

/** Decodes an 8-bit, non-interlaced RGB or RGBA PNG, as Playwright writes them. */
function decodePng(buf: Buffer): { width: number; height: number; bpp: number; data: Buffer } {
  let pos = 8;
  let width = 0;
  let height = 0;
  let bpp = 0;
  const idat: Buffer[] = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    const body = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      if (body[8] !== 8 || body[12] !== 0) throw new Error("unsupported PNG");
      bpp = body[9] === 6 ? 4 : body[9] === 2 ? 3 : 0;
      if (!bpp) throw new Error(`unsupported PNG colour type ${body[9]}`);
    } else if (type === "IDAT") {
      idat.push(body);
    }
    pos += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * bpp;
  const data = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? data[y * stride + x - bpp] : 0;
      const b = y > 0 ? data[(y - 1) * stride + x] : 0;
      const c = x >= bpp && y > 0 ? data[(y - 1) * stride + x - bpp] : 0;
      let pred = 0;
      if (filter === 1) pred = a;
      else if (filter === 2) pred = b;
      else if (filter === 3) pred = (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        pred = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      data[y * stride + x] = (line[x] + pred) & 0xff;
    }
  }
  return { width, height, bpp, data };
}

/** A screenshot of the viewport rect, with transitions run to their end. */
export async function shot(page: Page, box: { left: number; top: number; width: number; height: number }): Promise<Shot> {
  const left = Math.floor(box.left);
  const top = Math.floor(box.top);
  const width = Math.max(1, Math.ceil(box.left + box.width) - left);
  const height = Math.max(1, Math.ceil(box.top + box.height) - top);
  const png = decodePng(
    await page.screenshot({ clip: { x: left, y: top, width, height }, animations: "disabled", caret: "hide" }),
  );
  return {
    left,
    top,
    width,
    height,
    at(x, y) {
      const px = Math.floor(x) - left;
      const py = Math.floor(y) - top;
      if (px < 0 || py < 0 || px >= png.width || py >= png.height) throw new Error(`(${x}, ${y}) is outside the shot`);
      const i = (py * png.width + px) * png.bpp;
      return [png.data[i], png.data[i + 1], png.data[i + 2]];
    },
  };
}

export function hex(h: string): Rgb {
  const n = parseInt(h.replace("#", "").slice(0, 6), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** `#rrggbbaa` painted over an opaque background. */
export function over(h: string, bg: Rgb): Rgb {
  const fg = hex(h);
  const alpha = h.length === 9 ? parseInt(h.slice(7), 16) / 255 : 1;
  return [0, 1, 2].map((i) => Math.round(fg[i] * alpha + bg[i] * (1 - alpha))) as Rgb;
}

/** WebKit and Chromium differ by a few units after colour management. */
export function near(a: Rgb, b: Rgb, tol = 6): boolean {
  return a.every((v, i) => Math.abs(v - b[i]) <= tol);
}

export function distance(a: Rgb, b: Rgb): number {
  return Math.max(...a.map((v, i) => Math.abs(v - b[i])));
}

/** The colour in `pixels` furthest from `bg`, i.e. the fully covered ink of a mark. */
export function ink(pixels: Rgb[], bg: Rgb): Rgb | null {
  let best: Rgb | null = null;
  for (const p of pixels) if (distance(p, bg) > 24 && (!best || distance(p, bg) > distance(best, bg))) best = p;
  return best;
}

/** Every pixel of a box, row by row. */
export function pixelsOf(s: Shot, box: Box): Rgb[] {
  const out: Rgb[] = [];
  for (let y = Math.ceil(box.top); y < Math.floor(box.bottom); y++)
    for (let x = Math.ceil(box.left); x < Math.floor(box.right); x++) out.push(s.at(x, y));
  return out;
}

// --- Test objects -------------------------------------------------------------------------

/** Makes closed shadow roots open, so specs can look inside. Only the closed spec goes without. */
export const OPEN_ROOTS = `(() => {
  const attach = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function (init) {
    return attach.call(this, Object.assign({}, init, { mode: "open" }));
  };
})();`;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
  links?: Record<string, string>,
) => Promise<Served>;

/** Pages share the CDN cache; `birta` starts servers that stop when the test ends. */
export const testClosed = base.extend<{ birta: Start }>({
  page: async ({ page }, use) => {
    await cacheCdn(page);
    await use(page);
  },
  birta: async ({}, use) => {
    const started: Served[] = [];
    await use(async (files, env, args, links) => {
      const s = await startBirta(files, env, args, links);
      started.push(s);
      return s;
    });
    for (const s of started) s.stop();
  },
});

export const test = testClosed.extend({
  page: async ({ page }, use) => {
    await page.addInitScript(OPEN_ROOTS);
    await use(page);
  },
});
