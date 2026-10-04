import { Page } from "@playwright/test";
import { ChildProcess } from "child_process";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  AgentRun,
  GATE_ON,
  REPO,
  State,
  agent,
  agentWait,
  booted,
  hhmm,
  linesNamed,
  phraseOf,
  record,
  revOf,
  reviewLog,
  turnOf,
} from "../tests/review/harness";

// The reviewer's journey end to end, J-01 to J-16: keys only, the real server, a real CLI agent.

// The phrase prints local times; UTC lets the spec name them from the log's epoch ms
test.use({ timezoneId: "UTC" });

const NAME = "pr-draft.md";
const FIXTURES = path.join(REPO, "e2e", "fixtures", "review", "journey");
const DOC = fs.readFileSync(path.join(FIXTURES, "pr-draft.md"), "utf-8");
const ROUND1 = fs.readFileSync(path.join(FIXTURES, "round1.md"), "utf-8");
const CONFIG = ["[keybindings]", 'toggle_reading = "Alt+r"', 'toggle_dark = "Alt+d"', 'focus_theme = "Alt+t"', ""].join(
  "\n",
);
const WIDE = { width: 1280, height: 900 };
const MAX_PRESSES = 40;
/** Messages that follow a 5 s hold. */
const AFTER_HOLD = 10_000;

const ANSWER =
  "The default is 4 attempts (3 retries), not 5; with a 5 s cap that rides out typical blips and short deploys, and it's configurable. The integration suite hasn't been run yet, so that's still unverified.";
const TURN_NOTE_1 =
  "I trimmed the whole doc to roughly half, explained that we fail because put makes a single attempt with no retry logic, and dropped the Stacked on section.";
const SEPARATE = "I'll leave them separate.";
const TURN_NOTE_2 = "I left both bullets as they were and kept the rest.";

/** One line of `text` by its start, newline included. */
function lineOf(text: string, start: string): string {
  const hit = text.split("\n").filter((l) => l.startsWith(start));
  if (hit.length !== 1) throw new Error(`${hit.length} lines start ${JSON.stringify(start)}`);
  return `${hit[0]}\n`;
}

// J-10's revert puts the two old What bullets back in place of Claude's merged one
const MERGED = lineOf(ROUND1, "- Retry on network errors");
const REVERTED = ROUND1.replace(MERGED, lineOf(DOC, "- Wrap ") + lineOf(DOC, "- Retry only "));
/** Doc text only one version holds, rendered. */
const ONLY_HANDED = "#479 — add a storage proxy for local runs";
const ONLY_ROUND1 = "Most would succeed moments later.";
/** How long a landing gets to (wrongly) write before `y`. */
const LAND_SETTLE = 1_000;
/** The earliest a hold's end may show, from its key. */
const HOLD_MIN = 4_500;

// --- The review file and the agent --------------------------------------------------------

const named = (root: string, op: string): State[] => linesNamed(root, op, NAME);

function lastNamed(root: string, op: string): State {
  const all = named(root, op);
  if (!all.length) throw new Error(`no ${op} line in the review file`);
  return all[all.length - 1];
}

/** The number of the reviewer's one comment line that `pick` names. */
function noteNumber(root: string, what: string, pick: (l: State) => boolean): number {
  const hit = named(root, "comment").filter((l) => l.by?.type === "human" && pick(l));
  if (hit.length !== 1) throw new Error(`${hit.length} comment lines for ${what}`);
  return hit[0].n;
}

/** The review file's line count, to read what a key wrote after it. */
const logEnd = (root: string) => (reviewLog(root, NAME) ?? []).length;

/** The review file's lines after line `from`, each with its line number as `n`. */
const since = (root: string, from: number): State[] =>
  (reviewLog(root, NAME) ?? []).map((l, i) => ({ ...l, n: i + 1 })).filter((l) => l.n > from);

/** A line Claude reads as the reviewer's word: a note, verdict, reply or change to one. */
const toClaude = (l: State) => l.by?.type === "human" && ["comment", "reply", "amend", "retract"].includes(l.op);
const brief = (ls: State[]) => ls.map((l) => `${l.n} ${l.op} ${JSON.stringify(l.verdict ?? l.body ?? l.ids ?? "")}`);

const docFile = (root: string) => fs.readFileSync(path.join(root, NAME), "utf-8");

async function expectFile(root: string, J: string, want: string, what: string) {
  await expect.poll(() => docFile(root), { message: `${J} file: ${what}` }).toBe(want);
}

/** Landing on a stop checks nothing and tells Claude nothing, even after a settle. */
async function expectLandingQuiet(page: Page, root: string, J: string, from: number) {
  await page.waitForTimeout(LAND_SETTLE);
  const wrote = since(root, from).filter((l) => l.op === "seen" || toClaude(l));
  expect(brief(wrote), `${J} log: landing writes no seen and no note`).toEqual([]);
}

/** `y` on Claude's news checks it by a `seen` line and sends Claude nothing; returns the ids checked. */
async function expectChecked(root: string, J: string, from: number): Promise<string[]> {
  await expect
    .poll(() => since(root, from).filter((l) => l.op === "seen").length, { message: `${J} log: y checks by a seen line` })
    .toBeGreaterThan(0);
  expect(brief(since(root, from).filter(toClaude)), `${J} log: y on Claude's news sends Claude nothing`).toEqual([]);
  return since(root, from)
    .filter((l) => l.op === "seen")
    .flatMap((l) => l.ids ?? []);
}

/** A hold's key at `pressed`: its line claims from 5 s on, and its end shows no sooner. */
function expectHeld(J: string, pressed: number, notBefore: number, endSeen: number) {
  expect(notBefore - pressed, `${J} log: the hold runs its 5 s`).toBeGreaterThanOrEqual(HOLD_MIN);
  expect(notBefore - pressed, `${J} log: the hold runs its 5 s, no longer`).toBeLessThan(6_500);
  expect(endSeen - pressed, `${J} pill: the hold's end shows only once it's over`).toBeGreaterThanOrEqual(HOLD_MIN);
}

const spawned: ChildProcess[] = [];

/** `review --wait` in the background, so an agent is already waiting when the reviewer hands back. */
function waitForHandBack(root: string): Promise<AgentRun> {
  const { proc, done } = agentWait(root, ["review", NAME, "--wait", "--timeout", "0", "--as", "claude"]);
  spawned.push(proc);
  return done;
}

function killAgents() {
  for (const p of spawned.splice(0)) if (p.exitCode === null && p.signalCode === null) p.kill("SIGKILL");
}

// Agents run on past a failed step; stop them before the server's folder goes
test.afterEach(killAgents);
test.afterAll(killAgents);

/**
 * The change id of the one removal whose cut text holds `text`, from the newest state the page
 * was sent: its deletion's `head`, else a pure removal's `old_text`.
 */
function removalId(got: { msg: State }[], text: string): string {
  const state = [...got].reverse().find((f) => f.msg.type === "review_state" && f.msg.path === NAME)?.msg.state;
  const heads = (state?.deletions ?? []).filter((d: State) => (d.head ?? "").includes(text)).map((d: State) => d.change);
  const cuts = (state?.changes ?? [])
    .filter((c: State) => c.new_text === "" && (c.old_text ?? "").includes(text))
    .map((c: State) => c.id);
  const hit = heads.length ? heads : cuts;
  if (hit.length !== 1) {
    const ids = (state?.deletions ?? []).map((d: State) => `${d.change}: ${JSON.stringify(d.head ?? null)}`);
    throw new Error(`${hit.length} removals hold ${JSON.stringify(text)}; the state's deletions: ${ids.join(", ") || "none"}`);
  }
  return hit[0];
}

// --- Reading the page (never changes it) --------------------------------------------------

interface Row {
  lead: string | null;
  substance: string | null;
  tail: string | null;
  del: string[];
  ins: string[];
}

interface Snap {
  strip: {
    where: string | null;
    keys: string | null;
    message: boolean;
    dim: boolean;
    top: number;
    bottom: number;
  } | null;
  headerBottom: number;
  innerHeight: number;
  focus: {
    sp: string | null;
    target: string | null;
    after: string | null;
    change: string | null;
    top: number;
    bottom: number;
    text: string | null;
    /** The focused block's own box, when the mark names one. */
    block: { top: number; bottom: number } | null;
  }[];
  /** Every flow under the focused stop, in DOM order. */
  rows: { kind: string; text: string }[];
  receipts: Row[];
  lane: {
    mark: string;
    sp: string | null;
    check: string | null;
    verdict: string | null;
    top: number;
    bottom: number;
    width: number;
    height: number;
  }[];
  docStop: { bottom: number; visible: boolean } | null;
  contentTop: number;
  content: string;
  composer: { label: string | null; top: number; bottom: number } | null;
}

/** Everything the journey reads, in one pass over the page. */
function snap(page: Page): Promise<Snap> {
  return page.evaluate(() => {
    const squash = (s: string) => s.replace(/\s+/g, " ").trim();
    // Text of a node, reading into open shadow roots and skipping styles
    const textOf = (n: Node | null | undefined): string | null => {
      if (!n) return null;
      let out = "";
      const walk = (x: Node) => {
        if (x.nodeType === 3) out += x.textContent;
        else if (x.nodeType === 1) {
          const el = x as Element;
          if (el.localName === "style" || el.localName === "script") return;
          (el.shadowRoot ?? el).childNodes.forEach(walk);
        } else if (x.nodeType === 11) x.childNodes.forEach(walk);
      };
      walk(n);
      return squash(out);
    };
    const shown = (el: Element | null | undefined) => !!el && el.getClientRects().length > 0;
    const part = (host: Element, sel: string) => host.shadowRoot?.querySelector(sel) ?? host.querySelector(sel);
    const parts = (host: Element, sel: string) => [
      ...Array.from(host.shadowRoot?.querySelectorAll(sel) ?? []),
      ...Array.from(host.querySelectorAll(sel)),
    ];
    const content = document.getElementById("content")!;
    const blockAt = (sp: string) => {
      const all = content.querySelectorAll(`[data-sourcepos="${sp}"]`);
      return all[all.length - 1] ?? null;
    };

    const stripHost = document.querySelector("birta-strip");
    const strip = stripHost
      ? {
          where: textOf(part(stripHost, '[data-part="where"]')),
          keys: textOf(part(stripHost, '[data-part="keys"]')),
          message: stripHost.hasAttribute("data-message"),
          dim: stripHost.hasAttribute("data-dim"),
          top: stripHost.getBoundingClientRect().top,
          bottom: stripHost.getBoundingClientRect().bottom,
        }
      : null;

    const focus: Snap["focus"] = [];
    const lane: Snap["lane"] = [];
    for (const tag of ["birta-under", "birta-over"]) {
      const root = document.querySelector(`#lane-frame > ${tag}`)?.shadowRoot;
      root?.querySelectorAll("[data-mark]").forEach((m) => {
        const r = m.getBoundingClientRect();
        const a = (n: string) => m.getAttribute(n);
        if (a("data-mark") === "focus") {
          const sp = a("data-sp");
          const el = sp ? blockAt(sp) : null;
          const b = el?.getBoundingClientRect();
          focus.push({
            sp,
            target: a("data-target"),
            after: a("data-after"),
            change: a("data-change"),
            top: r.top,
            bottom: r.bottom,
            text: el ? squash(el.textContent ?? "") : null,
            block: b ? { top: b.top, bottom: b.bottom } : null,
          });
        } else {
          lane.push({
            mark: a("data-mark")!,
            sp: a("data-sp"),
            check: a("data-check"),
            verdict: a("data-verdict"),
            top: r.top,
            bottom: r.bottom,
            width: r.width,
            height: r.height,
          });
        }
      });
    }

    const flows = Array.from(document.querySelectorAll("birta-flow[data-kind]")).filter(
      (h) => shown(h) && !["doc-stop", "composer"].includes(h.getAttribute("data-kind")!),
    );
    let under: Element[] = [];
    if (focus.length === 1) {
      const f = focus[0];
      if (f.target === "doc") {
        // The whole doc's rows sit above the doc
        under = flows.filter((h) => !!(h.compareDocumentPosition(content) & Node.DOCUMENT_POSITION_FOLLOWING));
      } else if (f.target === "removal") {
        // A removal's row is the one open change row
        under = flows.filter((h) => h.getAttribute("data-kind") === "receipt");
      } else if (f.sp) {
        const el = blockAt(f.sp);
        if (el) {
          under = flows.filter((h) => el.contains(h));
          for (let k = el.nextElementSibling; k && k.localName === "birta-flow"; k = k.nextElementSibling)
            if (flows.includes(k)) under.push(k);
        }
      }
    }

    const receipts: Row[] = flows
      .filter((h) => h.getAttribute("data-kind") === "receipt")
      .map((h) => {
        const substance = part(h, '[data-part="substance"]');
        return {
          lead: textOf(part(h, '[data-part="lead"]')),
          substance: textOf(substance),
          tail: textOf(part(h, '[data-part="tail"]')),
          del: substance ? parts(substance, "del").map((d) => textOf(d) ?? "") : [],
          ins: substance ? parts(substance, "ins").map((d) => textOf(d) ?? "") : [],
        };
      });

    const stop = document.querySelector('birta-flow[data-kind="doc-stop"]');
    const composerHost = Array.from(document.querySelectorAll('birta-flow[data-kind="composer"]')).find(shown);
    return {
      strip,
      headerBottom: document.querySelector(".file-header")?.getBoundingClientRect().bottom ?? 0,
      innerHeight: window.innerHeight,
      focus,
      rows: under.map((h) => ({ kind: h.getAttribute("data-kind")!, text: textOf(h) ?? "" })),
      receipts,
      lane,
      docStop: stop ? { bottom: stop.getBoundingClientRect().bottom, visible: shown(stop) } : null,
      contentTop: content.getBoundingClientRect().top,
      content: squash(content.textContent ?? ""),
      composer: composerHost
        ? {
            label: textOf(part(composerHost, ".label")),
            top: composerHost.getBoundingClientRect().top,
            bottom: composerHost.getBoundingClientRect().bottom,
          }
        : null,
    };
  });
}

/** Where focus sits, as one comparable string. */
async function focusKey(page: Page): Promise<string> {
  const f = (await snap(page)).focus;
  if (f.length !== 1) return `${f.length} focus marks`;
  return [f[0].target ?? "block", f[0].sp, f[0].after, f[0].change].join(" ");
}

async function focusOne(page: Page, J: string) {
  const f = (await snap(page)).focus;
  expect(f.length, `${J} focus: exactly one focus mark`).toBe(1);
  return f[0];
}

const stripText = (s: Snap) => (s.strip ? `${s.strip.where} · ${s.strip.keys}` : null);
const joined = (parts: string[]) => parts.join(" ").replace(/\s+/g, " ");
const messageOf = (s: Snap) => (s.strip && s.strip.message ? s.strip.keys : null);

/** Viewport box of the deepest doc block whose text holds `text`. */
function blockOfText(page: Page, text: string): Promise<{ top: number; bottom: number } | null> {
  return page.evaluate((text) => {
    let hit: Element | null = null;
    document.querySelectorAll("#content [data-sourcepos]").forEach((el) => {
      if ((el.textContent ?? "").replace(/\s+/g, " ").includes(text)) hit = el;
    });
    if (!hit) return null;
    const r = (hit as Element).getBoundingClientRect();
    return { top: r.top, bottom: r.bottom };
  }, text);
}

// --- Expectations, each naming its J-NN and line ------------------------------------------

async function expectPhrase(page: Page, J: string, want: string) {
  await expect.poll(() => phraseOf(page), { message: `${J} phrase` }).toBe(want);
}

async function expectButton(page: Page, J: string, want: string) {
  await expect.poll(() => turnOf(page), { message: `${J} button` }).toBe(want);
}

/** The whole strip, `where · keys`, exactly. */
async function expectStrip(page: Page, J: string, want: string, timeout?: number) {
  await expect.poll(async () => stripText(await snap(page)), { message: `${J} strip`, timeout }).toBe(want);
}

/** The strip's left side alone, for stops whose keys side may still hold a message. */
async function expectWhere(page: Page, J: string, want: string) {
  await expect.poll(async () => (await snap(page)).strip?.where ?? null, { message: `${J} strip where` }).toBe(want);
}

/** A message in the strip's keys part, read as soon as it shows; returns when it was first seen. */
async function expectMessage(page: Page, J: string, want: string, timeout?: number): Promise<number> {
  // Polled tightly, so the time it was first seen is close to when it showed
  await expect.poll(async () => messageOf(await snap(page)), { message: `${J} message`, timeout, intervals: [50] }).toBe(want);
  return Date.now();
}

/** The message first seen at `t0` still shows at `there` ms and is gone by `gone` ms. */
async function expectLasts(page: Page, J: string, want: string, t0: number, there: number, gone: number) {
  const wait = t0 + there - Date.now();
  if (wait > 0) await page.waitForTimeout(wait);
  expect(messageOf(await snap(page)), `${J} message still shows at ${there / 1000} s`).toBe(want);
  await expect
    .poll(async () => messageOf(await snap(page)), {
      message: `${J} message gone by ${gone / 1000} s`,
      timeout: Math.max(1, t0 + gone - Date.now()),
    })
    .not.toBe(want);
}

async function expectNoMessage(page: Page, J: string) {
  await expect.poll(async () => (await snap(page)).strip?.message ?? null, { message: `${J} pill: none` }).toBe(false);
}

/**
 * The view: below the header and above the strip, clamped to the viewport, with the strip
 * fixed at the viewport's bottom; why not, when it isn't.
 */
function viewOf(s: Snap): { top: number; bottom: number } | string {
  if (!s.strip) return "no strip";
  if (Math.abs(s.strip.bottom - s.innerHeight) > 1) return `strip ${s.strip.top}-${s.strip.bottom}, not at the bottom ${s.innerHeight}`;
  return { top: Math.max(s.headerBottom, 0), bottom: Math.min(s.strip.top, s.innerHeight) };
}

/** The focus mark, and the block it names, lie in the view. */
async function expectInView(page: Page, J: string) {
  await expect
    .poll(
      async () => {
        const s = await snap(page);
        if (s.focus.length !== 1) return `${s.focus.length} focus marks`;
        const v = viewOf(s);
        if (typeof v === "string") return v;
        const f = s.focus[0];
        if (f.bottom - f.top <= 0) return `focus mark ${f.top}-${f.bottom} has no height`;
        if (f.top < v.top - 1 || f.bottom > v.bottom + 1) return `focus ${f.top}-${f.bottom}, view ${v.top}-${v.bottom}`;
        // The mark sits on its block, and the block starts in view
        const b = f.target === null ? f.block : null;
        if (b && !(b.top < f.bottom && b.bottom > f.top)) return `block ${b.top}-${b.bottom} off its mark ${f.top}-${f.bottom}`;
        if (b && (b.top < v.top - 1 || b.top > v.bottom)) return `block top ${b.top}, view ${v.top}-${v.bottom}`;
        return "in view";
      },
      { message: `${J} focus in view` },
    )
    .toBe("in view");
}

/** The open composer lies wholly in the view, at or under its stop's top. */
async function expectComposerInView(page: Page, J: string) {
  await expect
    .poll(
      async () => {
        const s = await snap(page);
        if (!s.composer) return "no composer";
        const v = viewOf(s);
        if (typeof v === "string") return v;
        const c = s.composer;
        if (c.top < v.top - 1 || c.bottom > v.bottom + 1) return `composer ${c.top}-${c.bottom}, view ${v.top}-${v.bottom}`;
        if (s.focus.length === 1 && c.top < s.focus[0].top - 1) return `composer ${c.top} above its stop ${s.focus[0].top}`;
        return "in view";
      },
      { message: `${J} composer fully in view` },
    )
    .toBe("in view");
}

/** The doc's own text, without notes, rows or composers. */
function docText(page: Page): Promise<string> {
  return page.evaluate(() => {
    const copy = document.getElementById("content")!.cloneNode(true) as Element;
    copy.querySelectorAll("birta-flow").forEach((f) => f.remove());
    return (copy.textContent ?? "").replace(/\s+/g, " ").trim();
  });
}

/** The page shows the doc as handed back: none of Claude's round-1 text, rows or bars. */
async function expectHandedBack(page: Page, J: string, when: string) {
  const text = await docText(page);
  expect(text, `${J} row: the handed-back text shows ${when}`).toContain(ONLY_HANDED);
  expect(text, `${J} row: Claude's text doesn't show ${when}`).not.toContain(ONLY_ROUND1);
  expect(await laneMarks(page, "bar"), `${J} row: no bar ${when}`).toEqual([]);
  expect((await snap(page)).receipts, `${J} row: no change row ${when}`).toEqual([]);
}

async function expectFocusDoc(page: Page, J: string) {
  await expect
    .poll(async () => (await snap(page)).focus.map((f) => f.target), { message: `${J} focus: whole doc` })
    .toEqual(["doc"]);
}

async function expectFocusOn(page: Page, J: string, text: string) {
  await expect
    .poll(
      async () => {
        const f = (await snap(page)).focus;
        return f.length === 1 && f[0].text?.includes(text) ? text : f.map((x) => x.text ?? x.target).join(" | ");
      },
      { message: `${J} focus on the block holding ${JSON.stringify(text)}` },
    )
    .toBe(text);
}

/** Focus on the removal stop of change `id`, which names its gap. */
async function expectFocusRemoval(page: Page, J: string, id: string) {
  await expect
    .poll(
      async () => (await snap(page)).focus.map((f) => `${f.target} ${f.change}${f.after === null ? " (no gap)" : ""}`),
      { message: `${J} focus: the removal stop of ${id}` },
    )
    .toEqual([`removal ${id}`]);
}

type Want = [kind: string, text: string | string[]];

/**
 * The flows under the focused stop hold `want` in this order: each entry is a flow of that
 * kind whose text holds every given string. Other flows may sit between them.
 */
async function expectRows(page: Page, J: string, want: Want[]) {
  await expect
    .poll(
      async () => {
        const rows = (await snap(page)).rows;
        let at = 0;
        for (const [kind, text] of want) {
          const all = Array.isArray(text) ? text : [text];
          while (at < rows.length && !(rows[at].kind === kind && all.every((t) => rows[at].text.includes(t)))) at++;
          if (at === rows.length) return rows.map((r) => `${r.kind}: ${r.text}`);
          at++;
        }
        return "in order";
      },
      { message: `${J} row: ${want.map(([k, t]) => `${k} ${JSON.stringify(t)}`).join(", then ")}` },
    )
    .toBe("in order");
}

/** The one open change row. */
async function openRow(page: Page, J: string): Promise<Row> {
  await expect.poll(async () => (await snap(page)).receipts.length, { message: `${J} row: one open change row` }).toBe(1);
  return (await snap(page)).receipts[0];
}

async function expectRowLead(page: Page, J: string, want: string) {
  await expect.poll(async () => (await openRow(page, J)).lead, { message: `${J} row lead` }).toBe(want);
}

async function expectRowTail(page: Page, J: string, want: string) {
  await expect.poll(async () => (await openRow(page, J)).tail, { message: `${J} row tail` }).toBe(want);
}

async function expectLabel(page: Page, J: string, want: string) {
  await expect.poll(async () => (await snap(page)).composer?.label ?? null, { message: `${J} composer label` }).toBe(want);
}

const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"] textarea');

/** Drawn lane marks of one kind; the clean view keeps some in the lane at zero size. */
async function laneMarks(page: Page, mark: string) {
  return (await snap(page)).lane.filter((m) => m.mark === mark && m.width > 0 && m.height > 0);
}

// --- Keys ---------------------------------------------------------------------------------

/** Presses `key` once and waits for focus to move. */
async function step(page: Page, J: string, key: string) {
  const before = await focusKey(page);
  await page.keyboard.press(key);
  await expect.poll(() => focusKey(page), { message: `${J}: ${key} moves focus from ${before}` }).not.toBe(before);
}

/** Presses `j` or `k` until focus sits on the block holding `text`; returns the presses. */
async function moveTo(page: Page, J: string, key: "j" | "k", text: string): Promise<number> {
  for (let i = 1; i <= MAX_PRESSES; i++) {
    await step(page, J, key);
    const f = (await snap(page)).focus;
    if (f.length === 1 && f[0].text?.includes(text)) return i;
  }
  throw new Error(`${J}: ${MAX_PRESSES} presses of ${key} never reached ${JSON.stringify(text)}`);
}

// --- The journey --------------------------------------------------------------------------

test("the reviewer's journey, J-01 to J-16, by keys with a real agent", async ({ page, birta }) => {
  const served = await birta({ [NAME]: DOC }, GATE_ON, ["--drafted-by", "claude", NAME], {}, {
    ".config/birta/config.toml": CONFIG,
  });
  const root = served.root;
  const frames = record(page);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/${NAME}`);
  await booted(page);

  await test.step("J-01 · page opens", async () => {
    const J = "J-01";
    await expectFocusDoc(page, J);
    await expect
      .poll(async () => (await snap(page)).docStop?.visible ?? false, { message: `${J} focus: the whole doc line shows` })
      .toBe(true);
    const s = await snap(page);
    expect(s.docStop!.bottom, `${J} focus: the whole doc line sits above #content`).toBeLessThanOrEqual(s.contentTop + 1);
    await expectInView(page, J);
    await expectPhrase(page, J, "drafted by Claude · your turn");
    await expectNoMessage(page, J);
    await expectStrip(page, J, "whole doc · c note on the whole doc · j down · ? keys");
    expect((await snap(page)).receipts, `${J} row: none`).toEqual([]);
    await expectButton(page, J, "Approve ⌘⏎");
  });

  await test.step("J-02 · whole-doc note", async () => {
    const J = "J-02";
    await page.keyboard.press("c");
    await expect(composer(page), `${J} composer opens`).toBeFocused();
    await expectComposerInView(page, J);
    await page.keyboard.type("way too verbose");
    await page.keyboard.press("Enter");
    const t0 = await expectMessage(page, J, "note saved · z to undo");
    await expectLasts(page, J, "note saved · z to undo", t0, 2_000, 6_000);
    await expectFocusDoc(page, J);
    await expectInView(page, J);
    await expectPhrase(page, J, "drafted by Claude · your turn");
    await expectRows(page, J, [["note", "you · way too verbose"]]);
    await expectButton(page, J, "Hand back · 1 ⌘⏎");
  });

  await test.step("J-03 · quote note by keyboard", async () => {
    const J = "J-03";
    const presses = await moveTo(page, J, "j", "Uploads to the storage service");
    expect(presses, `${J} key: j ×3 reaches the Why paragraph`).toBe(3);
    await page.keyboard.press("c");
    await expect(composer(page), `${J} composer opens`).toBeFocused();
    await expectLabel(page, J, "note on Uploads to the storage service… (paragraph)");
    await expectComposerInView(page, J);
    await page.keyboard.type('"we just fail"');
    await expectLabel(page, J, 'note on "we just fail"');
    await page.keyboard.type(" say why");
    await page.keyboard.press("Enter");
    await expectMessage(page, J, "note saved · z to undo");
    await expectStrip(page, J, "## Why › paragraph · your note · note saved · z to undo");
    await expectFocusOn(page, J, "Uploads to the storage service");
    await expectInView(page, J);
    await expectPhrase(page, J, "drafted by Claude · your turn");
    await expectRows(page, J, [["note", ['→ "we just fail"', "you · say why"]]]);
    const quoted = (await snap(page)).rows.find((r) => r.kind === "note" && r.text.includes("you · say why"))!.text;
    expect(quoted.indexOf('→ "we just fail"'), `${J} row: the quote leads the note`).toBeLessThan(
      quoted.indexOf("you · say why"),
    );
    await expectButton(page, J, "Hand back · 2 ⌘⏎");
  });

  await test.step("J-04 · section no, then a question", async () => {
    const J = "J-04";
    await moveTo(page, J, "j", "Stacked on");
    const heading = (await focusOne(page, J)).sp!;
    await page.keyboard.press("x");
    await expectMessage(page, J, "no · ## Stacked on (section) · z to undo");
    await expect
      .poll(async () => (await laneMarks(page, "glyph")).some((m) => m.sp === heading && m.verdict === "no"), {
        message: `${J} row: × on the heading`,
      })
      .toBe(true);
    // The wash reaches every block of the section, down to its last bullet
    for (const bullet of ["#482 — extract UploadClient", "#479 — add a storage proxy"]) {
      const box = (await blockOfText(page, bullet))!;
      await expect
        .poll(async () => (await laneMarks(page, "wash")).some((w) => w.top <= box.top + 1 && w.bottom >= box.bottom - 1), {
          message: `${J} row: the section is washed down to ${bullet}`,
        })
        .toBe(true);
    }
    await moveTo(page, J, "j", "Ran the integration suite");
    await page.keyboard.press("q");
    await expect(composer(page), `${J} composer opens`).toBeFocused();
    await expectLabel(page, J, "question on - [ ] Ran the integration suite (list item)");
    await expectComposerInView(page, J);
    await page.keyboard.type("is 5 retries enough?");
    await page.keyboard.press("Enter");
    await expectMessage(page, J, "question saved · z to undo");
    await expectFocusOn(page, J, "Ran the integration suite");
    await expectInView(page, J);
    await expectPhrase(page, J, "drafted by Claude · your turn");
    await expectRows(page, J, [["note", "you · is 5 retries enough?"]]);
    await expectButton(page, J, "Hand back · 4 ⌘⏎");
    await expect.poll(() => brief(named(root, "comment")), { message: `${J} log: the four notes, as written` }).toEqual([
      expect.stringMatching(/^\d+ comment "way too verbose"$/),
      expect.stringMatching(/^\d+ comment "say why"$/),
      expect.stringMatching(/^\d+ comment "no"$/),
      expect.stringMatching(/^\d+ comment "ask"$/),
    ]);
  });

  const round1 = waitForHandBack(root);

  await test.step("J-05 · hand back", async () => {
    const J = "J-05";
    const pressed = Date.now();
    await page.keyboard.press("ControlOrMeta+Enter");
    await expectMessage(page, J, "handing back · 4 notes · z to undo");
    expect(await phraseOf(page), `${J} phrase: unchanged for the hold`).toBe("drafted by Claude · your turn");
    const sentAt = await expectMessage(page, J, "sent · you can keep reading", AFTER_HOLD);
    await expect.poll(() => named(root, "claim").length, { message: `${J}: the waiting agent claims` }).toBe(1);
    const submit = lastNamed(root, "submit");
    const claim = lastNamed(root, "claim");
    expectHeld(J, pressed, submit.not_before, sentAt);
    // A-22: with an agent waiting, `sent` passes in under a second, and never before the hold ends
    expect(claim.ts - submit.not_before, `${J}: picked up within a second of the hold`).toBeLessThan(1_000);
    expect(claim.ts, `${J}: not picked up during the hold`).toBeGreaterThanOrEqual(submit.not_before);
    await expectPhrase(page, J, `Claude picked up ${hhmm(claim.ts)}`);
    await expectButton(page, J, "With Claude");
    await expectFocusOn(page, J, "Ran the integration suite");
    await expectInView(page, J);
    expect(await docText(page), `${J} row: the doc as handed back`).toContain(ONLY_HANDED);
  });

  await test.step("J-06 · Claude works", async () => {
    const J = "J-06";
    // The agent takes the notes and writes its edit; birta records it as one external write
    const picked = await round1;
    expect(picked.status, `${J} agent review --wait: ${picked.stderr}`).toBe(0);
    served.write(NAME, ROUND1);
    await expect
      .poll(() => named(root, "external").map((l) => l.to_rev), { message: `${J}: the agent's write lands as one external line` })
      .toEqual([revOf(ROUND1)]);
    // The write reaches the server now; the page must hold the handed-back text through it
    for (let i = 0; i < 15; i++) {
      await expectHandedBack(page, J, `${i * 100} ms after Claude's write`);
      await page.waitForTimeout(100);
    }
    await step(page, J, "k");
    await expectFocusOn(page, J, "Unit tests for retryable vs. non-retryable status codes");
    await page.keyboard.press("Alt+KeyU");
    await expectStrip(page, J, "source view · esc back");
    // A review key leaves source view, then acts
    await page.keyboard.press("k");
    await expectFocusOn(page, J, "Unit tests for backoff timing and jitter bounds");
    await expectStrip(page, J, "## Testing › list item · with Claude · c note for next hand-back · ? keys");
    const held = await focusKey(page);
    await page.keyboard.press("Alt+KeyR");
    await expectStrip(page, J, "reading mode · esc back");
    await expect
      .poll(async () => (await snap(page)).strip?.dim ?? null, { message: `${J} strip dimmed in reading mode` })
      .toBe(true);
    await page.keyboard.press("Escape");
    await expect
      .poll(async () => (await snap(page)).strip?.dim ?? null, { message: `${J} Esc leaves reading mode in one press` })
      .toBe(false);
    expect(await focusKey(page), `${J} focus unchanged by Esc`).toBe(held);
    await expectHandedBack(page, J, "back from reading mode");
    await expectInView(page, J);
    await expectPhrase(page, J, `Claude picked up ${hhmm(lastNamed(root, "claim").ts)}`);
    await expectNoMessage(page, J);
    await expectStrip(page, J, "## Testing › list item · with Claude · c note for next hand-back · ? keys");
    // The doc as handed back, though the log already has Claude's write
    const s = await snap(page);
    expect(s.content, `${J} row: the handed-back text still shows ## Stacked on`).toContain("Stacked on");
    expect(s.content, `${J} row: the handed-back Why paragraph still shows`).toContain(
      "Uploads to the storage service occasionally fail with transient errors",
    );
    expect(await laneMarks(page, "bar"), `${J} row: Claude's edits don't show yet`).toEqual([]);
    expect(s.receipts, `${J} row: no change rows yet`).toEqual([]);
    await expectHandedBack(page, J, "at the end of Claude's turn");
    expect(docFile(root), `${J} file: Claude's write, untouched by the page`).toBe(ROUND1);
    expect(named(root, "edit"), `${J} log: the page writes nothing mid-turn`).toEqual([]);
    await expectButton(page, J, "With Claude");
  });

  let round2!: Promise<AgentRun>;
  let yield1!: State;

  await test.step("J-07 · reply lands", async () => {
    const J = "J-07";
    const verbose = noteNumber(root, "way too verbose", (l) => l.body === "way too verbose");
    const sayWhy = noteNumber(root, "say why", (l) => l.body === "say why");
    const stacked = noteNumber(root, "the no on ## Stacked on", (l) => l.verdict === "no");
    const question = noteNumber(root, "the question", (l) => l.verdict === "ask");
    const before = await focusOne(page, J);
    const replied1 = agent(
      root,
      ["respond", NAME, "--as", "claude"],
      [
        `#${sayWhy} done`,
        `#${verbose} done`,
        `#${verbose} covers "Most would succeed moments later."`,
        `#${verbose} covers "Retry on network errors, 429 and 502–504 with exponential backoff and jitter"`,
        `#${verbose} covers "defaults: 4 attempts, 200 ms base, 5 s cap"`,
        `#${verbose} covers "Unit tests for backoff timing and retryable status codes"`,
        `#${stacked} done`,
        'unasked "Respect `Retry-After` when the server sends it": folded into the defaults bullet',
        'unasked "Log each retry at `debug` with attempt number and delay": too much detail for a PR description',
        `#${question} answer: ${ANSWER}`,
        `@: ${TURN_NOTE_1}`,
        "",
      ].join("\n"),
    );
    expect(replied1.status, `${J} agent respond, round 1: ${replied1.stdout}${replied1.stderr}`).toBe(0);
    round2 = waitForHandBack(root);
    yield1 = lastNamed(root, "yield");
    const t0 = await expectMessage(page, J, "Claude replied · 9 to check · n starts at the top");
    await expectLasts(page, J, "Claude replied · 9 to check · n starts at the top", t0, 4_000, 8_000);
    await expectPhrase(page, J, `Claude replied ${hhmm(yield1.ts)} · 9 to check`);
    // The merged unit-test bullet takes the focused bullet's place, at the same height
    await expectFocusOn(page, J, "Unit tests for backoff timing");
    const after = await focusOne(page, J);
    // Same screen position, unless the scroll that would keep it is past the page's top or bottom:
    // then the page sits at that end and the gap is exactly what the clamp took
    const scroll = await page.evaluate(() => ({ y: scrollY, max: document.documentElement.scrollHeight - innerHeight }));
    const keep = scroll.y + after.top - before.top;
    const clamped = Math.min(Math.max(keep, 0), scroll.max);
    expect(Math.abs(scroll.y - clamped), `${J} focus: same screen position, but for the clamp`).toBeLessThanOrEqual(1);
    await expectInView(page, J);
    const text = await docText(page);
    expect(text, `${J} row: Claude's text arrives`).toContain(ONLY_ROUND1);
    expect(text, `${J} row: the handed-back text goes`).not.toContain(ONLY_HANDED);
    expect(
      (await laneMarks(page, "bar")).some((b) => b.check === "new"),
      `${J} row: Claude's new bars arrive with its text`,
    ).toBe(true);
    await expectButton(page, J, "Approve ⌘⏎");
  });

  await test.step("J-08 · first walk stop: the whole doc", async () => {
    const J = "J-08";
    const from = logEnd(root);
    await step(page, J, "n");
    await expectFocusDoc(page, J);
    await expectInView(page, J);
    await expectStrip(page, J, "1 of 9 · whole doc · Claude replied · y ok · c reply · n next");
    await expectPhrase(page, J, `Claude replied ${hhmm(yield1.ts)} · 9 to check`);
    await expectRows(page, J, [
      ["note", "you · way too verbose"],
      ["reply", "↳ Claude · done · 4 changes · n walks them"],
      ["turn_note", `↳ Claude · ${TURN_NOTE_1}`],
    ]);
    await expectLandingQuiet(page, root, J, from);
    await page.keyboard.press("y");
    await expectMessage(page, J, "ok · Claude's replies");
    await expectChecked(root, J, from);
    await expectStrip(page, J, "1 of 9 · whole doc · Claude replied · ok · Claude's replies");
    await expectPhrase(page, J, `Claude replied ${hhmm(yield1.ts)} · 8 to check`);
    await expectFocusDoc(page, J);
    await expectButton(page, J, "Approve ⌘⏎");
  });

  await test.step("J-09 · the say why change", async () => {
    const J = "J-09";
    const from = logEnd(root);
    await step(page, J, "n");
    await expectFocusOn(page, J, "UploadClient.put makes a single attempt");
    await expectRows(page, J, [
      ["note", "you · say why"],
      ["receipt", '↳ Claude · done · also for "way too verbose"'],
    ]);
    await expectRowLead(page, J, '↳ Claude · done · also for "way too verbose"');
    const row = await openRow(page, J);
    expect(joined(row.ins), `${J} row: the new why sentence is underlined`).toContain(
      "UploadClient.put makes a single attempt with no retry logic",
    );
    expect(row.del.length, `${J} row: the trimmed sentences are struck`).toBeGreaterThan(0);
    expect(joined(row.del), `${J} row: the old opening is struck`).toContain(
      "Uploads to the storage service occasionally fail with transient errors",
    );
    expect(joined(row.del), `${J} row: the new sentence isn't struck`).not.toContain("makes a single attempt");
    expect(joined(row.ins), `${J} row: the old opening isn't underlined`).not.toContain("Uploads to the storage service");
    await expectLandingQuiet(page, root, J, from);
    await page.keyboard.press("y");
    await expectMessage(page, J, "kept · z to undo");
    await expectChecked(root, J, from);
    await expectStrip(page, J, "2 of 9 · ## Why › paragraph · Claude's change · kept · z to undo");
    await expectRowTail(page, J, "kept");
    await expectPhrase(page, J, `Claude replied ${hhmm(yield1.ts)} · 7 to check`);
    await expectInView(page, J);
    await expectButton(page, J, "Approve ⌘⏎");
  });

  await test.step("J-10 · merged bullets, reverted and restored", async () => {
    const J = "J-10";
    const from = logEnd(root);
    await step(page, J, "n");
    await expectFocusOn(page, J, "Retry on network errors, 429 and 502–504");
    await expectWhere(page, J, "3 of 9 · ## What › list item · Claude's change");
    const top = (await focusOne(page, J)).top;
    await expectLandingQuiet(page, root, J, from);
    const writes: [message: string, first: string, file: string][] = [
      ["reverted · u restores Claude's text", "Wrap UploadClient.put in a retry loop", REVERTED],
      ["restored Claude's text · u reverts again", "Retry on network errors, 429 and 502–504", ROUND1],
      ["reverted · u restores Claude's text", "Wrap UploadClient.put in a retry loop", REVERTED],
    ];
    for (const [i, [message, first, file]] of writes.entries()) {
      await page.keyboard.press("u");
      await expectFocusOn(page, J, first);
      await expectMessage(page, J, message);
      expect(Math.abs((await focusOne(page, J)).top - top), `${J} focus stays through the write`).toBeLessThanOrEqual(1);
      // Each u is one write of exactly that change, recorded as birta's own
      await expectFile(root, J, file, `after u ×${i + 1}`);
      await expect
        .poll(() => named(root, "edit").length, { message: `${J} log: u ×${i + 1} is one edit line` })
        .toBe(i + 1);
      expect(lastNamed(root, "edit").to_rev, `${J} log: u ×${i + 1} records the bytes written`).toBe(revOf(file));
    }
    expect(named(root, "external").length, `${J} log: no write of the page's reads as Claude's`).toBe(1);
    await expectInView(page, J);
    await expectPhrase(page, J, `Claude replied ${hhmm(yield1.ts)} · 6 to check`);
    await expectRows(page, J, [["receipt", 'Claude · because you said "way too verbose"']]);
    await expectRowLead(page, J, 'Claude · because you said "way too verbose"');
    const row = await openRow(page, J);
    const struck = joined(row.del);
    expect(struck, `${J} row: the first old bullet struck`).toContain("Wrap UploadClient.put in a retry loop");
    expect(struck, `${J} row: the second old bullet struck`).toContain("Retry only on idempotent failure modes");
    expect(joined(row.ins), `${J} row: the new bullet underlined`).toContain(
      "Retry on network errors, 429 and 502–504 with exponential backoff and jitter",
    );
    expect(row.tail ?? "", `${J} row tail`).toMatch(/^reverted by you/);
    expect((await laneMarks(page, "revert")).length, `${J} lane: ↺`).toBeGreaterThan(0);
    await expectButton(page, J, "Approve ⌘⏎");
  });

  await test.step("J-11 · two cut bullets, the reworded bullet, the section cut", async () => {
    const J = "J-11";
    const stops: { lead: string; block?: string; removed?: string; left: number }[] = [
      { lead: "Claude · not asked for: folded into the defaults bullet", removed: "Respect `Retry-After`", left: 5 },
      { lead: 'Claude · because you said "way too verbose"', block: "Honor Retry-After; defaults: 4 attempts", left: 4 },
      { lead: "Claude · not asked for: too much detail for a PR description", removed: "Log each retry", left: 3 },
      { lead: "Claude · because you said no to ## Stacked on", removed: "## Stacked on", left: 2 },
    ];
    const cuts = stops.filter((x) => x.removed).map((x) => removalId(frames.got, x.removed!));
    // Derived from the walk order: whole doc 1, Why 2, the merged bullet 3, then these
    const wheres = [
      "4 of 9 · ## What › removed list item",
      "5 of 9 · ## What › list item · Claude's change",
      "6 of 9 · ## What › removed list item",
      null,
    ];
    for (const [i, { lead, block, removed, left }] of stops.entries()) {
      const from = logEnd(root);
      await step(page, J, "n");
      const id = removed ? removalId(frames.got, removed) : null;
      if (block) await expectFocusOn(page, J, block);
      else await expectFocusRemoval(page, J, id!);
      // The strip table's removal row, word for word
      if (i === 0) await expectStrip(page, J, `${wheres[i]} · y keep the cut · u put it back · c reply · n next`, 8_000);
      else if (wheres[i]) await expectWhere(page, J, wheres[i]!);
      await expectRowLead(page, J, lead);
      await expectLandingQuiet(page, root, J, from);
      await page.keyboard.press("y");
      await expectPhrase(page, J, `Claude replied ${hhmm(yield1.ts)} · ${left} to check`);
      await expectMessage(page, J, "kept · z to undo");
      const checked = await expectChecked(root, J, from);
      if (id) {
        // Two removals share this gap; y checks the focused one only
        expect(checked, `${J} log: y checks this removal`).toContain(id);
        for (const other of cuts.filter((c) => c !== id))
          expect(checked, `${J} log: y leaves the other removals unchecked`).not.toContain(other);
      }
    }
    await expectRows(page, J, [["receipt", "Claude · because you said no to ## Stacked on"]]);
    const row = await openRow(page, J);
    expect(row.substance ?? "", `${J} row: removed: then the heading`).toContain("removed: ## Stacked on");
    expect(joined(row.del), `${J} row: the heading struck`).toContain("## Stacked on");
    expect(joined(row.del), `${J} row: its first block struck`).toContain("#482 — extract UploadClient from the sync worker");
    expect(row.substance ?? "", `${J} row: and 1 more block`).toContain("and 1 more block");
    expect(row.substance ?? "", `${J} row: the second block stays folded`).not.toContain(ONLY_HANDED);
    expect(joined(row.del), `${J} row: the bullet cuts stay apart from the section cut`).not.toMatch(
      /Log each retry|Respect Retry-After/,
    );
    await expectRowTail(page, J, "kept");
    await expectInView(page, J);
    await expectButton(page, J, "Approve ⌘⏎");
  });

  await test.step("J-12 · the last change, then Claude's answer", async () => {
    const J = "J-12";
    let from = logEnd(root);
    await step(page, J, "n");
    await expectFocusOn(page, J, "Unit tests for backoff timing and retryable status codes");
    await expectWhere(page, J, "8 of 9 · ## Testing › list item · Claude's change");
    await expectLandingQuiet(page, root, J, from);
    await page.keyboard.press("y");
    await expectPhrase(page, J, `Claude replied ${hhmm(yield1.ts)} · 1 to check`);
    await expectMessage(page, J, "kept · z to undo");
    await expectChecked(root, J, from);
    from = logEnd(root);
    await step(page, J, "n");
    await expectFocusOn(page, J, "Ran the integration suite");
    await expectStrip(page, J, "9 of 9 · ## Testing › list item · Claude replied · y ok · c reply · n next", 8_000);
    await expectRows(page, J, [
      ["note", "you · is 5 retries enough?"],
      ["reply", `↳ Claude · ${ANSWER}`],
    ]);
    await expectLandingQuiet(page, root, J, from);
    await page.keyboard.press("y");
    await expectMessage(page, J, "ok · Claude's answer");
    await expectChecked(root, J, from);
    await expectPhrase(page, J, `Claude replied ${hhmm(yield1.ts)} · all checked`);
    await expectInView(page, J);
    await expectButton(page, J, "Approve ⌘⏎");
  });

  await test.step("J-13 · reply on the reverted bullets, hand back", async () => {
    const J = "J-13";
    for (let i = 0; i < 6; i++) await step(page, J, "p");
    await expectFocusOn(page, J, "Wrap UploadClient.put in a retry loop");
    await page.keyboard.press("c");
    await expect(composer(page), `${J} composer opens`).toBeFocused();
    await expectLabel(page, J, "reply to Claude's change on ## What › list item");
    await expectComposerInView(page, J);
    await expect
      .poll(async () => (await snap(page)).strip?.keys ?? null, { message: `${J} hint` })
      .toBe("⏎ save · ⇧⏎ new line · ⌘⏎ save & hand back · esc save & close");
    await page.keyboard.type("keep these two separate");
    const pressed = Date.now();
    await page.keyboard.press("ControlOrMeta+Enter");
    await expectMessage(page, J, "handing back · 1 note · z to undo");
    expect(await phraseOf(page), `${J} phrase: unchanged for the hold`).toBe(
      `Claude replied ${hhmm(yield1.ts)} · all checked`,
    );
    await expectRows(page, J, [["note", "you · keep these two separate"]]);
    const sentAt = await expectMessage(page, J, "sent · you can keep reading", AFTER_HOLD);
    await expect.poll(() => named(root, "claim").length, { message: `${J}: the waiting agent claims` }).toBe(2);
    const submit = lastNamed(root, "submit");
    const claim = lastNamed(root, "claim");
    expectHeld(J, pressed, submit.not_before, sentAt);
    expect(claim.ts - submit.not_before, `${J}: picked up within a second of the hold`).toBeLessThan(1_000);
    expect(claim.ts, `${J}: not picked up during the hold`).toBeGreaterThanOrEqual(submit.not_before);
    // Every y of the round told Claude nothing: the hand-back carries the one reply
    expect(
      brief(since(root, yield1.n).filter((l) => toClaude(l) && l.n < submit.n)),
      `${J} log: the hand-back holds only the reply`,
    ).toEqual([expect.stringMatching(/ "keep these two separate"$/)]);
    expect(docFile(root), `${J} file: still reverted`).toBe(REVERTED);
    await expectPhrase(page, J, `Claude picked up ${hhmm(claim.ts)}`);
    await expectFocusOn(page, J, "Wrap UploadClient.put in a retry loop");
    await expectInView(page, J);
    await expectButton(page, J, "With Claude");
  });

  let approval!: Promise<AgentRun>;
  let yield2!: State;

  await test.step("J-14 · second reply", async () => {
    const J = "J-14";
    const picked2 = await round2;
    expect(picked2.status, `${J} agent review --wait, round 2: ${picked2.stderr}`).toBe(0);
    const separate = noteNumber(root, "keep these two separate", (l) => l.body === "keep these two separate");
    const replied2 = agent(
      root,
      ["respond", NAME, "--as", "claude"],
      [`#${separate} answer: ${SEPARATE}`, `@: ${TURN_NOTE_2}`, ""].join("\n"),
    );
    expect(replied2.status, `${J} agent respond, round 2: ${replied2.stdout}${replied2.stderr}`).toBe(0);
    approval = waitForHandBack(root);
    yield2 = lastNamed(root, "yield");
    await expectMessage(page, J, "Claude replied · 2 to check · n starts at the top");
    await expectPhrase(page, J, `Claude replied ${hhmm(yield2.ts)} · 2 to check`);
    const from = logEnd(root);
    await step(page, J, "n");
    await expectFocusDoc(page, J);
    await expectInView(page, J);
    await expectWhere(page, J, "1 of 2 · whole doc · Claude replied");
    await expectRows(page, J, [["turn_note", `↳ Claude · ${TURN_NOTE_2}`]]);
    await expectLandingQuiet(page, root, J, from);
    await page.keyboard.press("y");
    await expectMessage(page, J, "ok · Claude's replies");
    await expectChecked(root, J, from);
    await expectPhrase(page, J, `Claude replied ${hhmm(yield2.ts)} · 1 to check`);
    await expectButton(page, J, "Approve ⌘⏎");
  });

  await test.step("J-15 · the last reply, then approve", async () => {
    const J = "J-15";
    const from = logEnd(root);
    await step(page, J, "n");
    await expectFocusOn(page, J, "Wrap UploadClient.put in a retry loop");
    await expectWhere(page, J, "2 of 2 · ## What › list item · Claude replied");
    await expectRows(page, J, [
      ["note", "you · keep these two separate"],
      ["reply", `↳ Claude · ${SEPARATE}`],
    ]);
    await expectLandingQuiet(page, root, J, from);
    await page.keyboard.press("y");
    await expectMessage(page, J, "ok · Claude's reply");
    await expectChecked(root, J, from);
    await expectPhrase(page, J, `Claude replied ${hhmm(yield2.ts)} · all checked`);
    // The round-1 revert is settled: its ↺ stays and u has nothing to do
    expect((await laneMarks(page, "revert")).length, `${J} lane: ↺ stays`).toBeGreaterThan(0);
    const edits = named(root, "edit").length;
    await page.keyboard.press("u");
    await expectMessage(page, J, "nothing here to revert");
    expect(docFile(root), `${J} file: u leaves the settled revert`).toBe(REVERTED);
    expect(named(root, "edit").length, `${J} log: u writes nothing`).toBe(edits);
    const pressed = Date.now();
    await page.keyboard.press("ControlOrMeta+Enter");
    await expectMessage(page, J, "approving · z to undo");
    const approvedMsg = "approved ✓ · this text is final · nothing is published without you";
    const t0 = await expectMessage(page, J, approvedMsg, AFTER_HOLD);
    await expectLasts(page, J, approvedMsg, t0, 4_000, 8_000);
    const approve = lastNamed(root, "approve");
    expectHeld(J, pressed, approve.not_before, t0);
    // What's approved is the text on disk: Claude's round 1 with your revert
    expect(docFile(root), `${J} file: the approved text`).toBe(REVERTED);
    expect(approve.rev, `${J} log: approve names the text on disk`).toBe(revOf(REVERTED));
    expect(brief(since(root, yield2.n).filter(toClaude)), `${J} log: round 2's y told Claude nothing`).toEqual([]);
    await expectPhrase(page, J, `approved ${hhmm(approve.not_before)} · c to reopen`);
    await expectButton(page, J, "Approved ✓");
    expect(await laneMarks(page, "bar"), `${J} row: dots only, no bar drawn`).toEqual([]);
    await expectFocusOn(page, J, "Wrap UploadClient.put in a retry loop");
    await expectInView(page, J);
    const done = await approval;
    expect(done.status, `agent review --wait after approve: ${done.stderr}`).toBe(0);
    // The CLI names the approved rev by a short prefix of it
    const shown = new RegExp(`^next: birta show ${NAME.replace(".", "\\.")}@([0-9a-f]{7,})\\b`, "m").exec(done.stdout);
    expect(shown, `${J}: the agent's next: names the approved text`).not.toBeNull();
    expect(approve.rev.startsWith(shown![1]), `${J}: ${shown![1]} is a prefix of ${approve.rev}`).toBe(true);
  });

  await test.step("J-16 · one more thought", async () => {
    const J = "J-16";
    await page.keyboard.press("c");
    await expect(composer(page), `${J} composer opens`).toBeFocused();
    await expectLabel(page, J, "new round · note on - Wrap UploadClient.put… (list item)");
    await expectComposerInView(page, J);
    await page.keyboard.type("link the docs PR");
    await page.keyboard.press("Enter");
    await expectMessage(page, J, "reopened · hand back when ready · z keeps it approved");
    await expectPhrase(page, J, `approved ${hhmm(lastNamed(root, "approve").not_before)} · reopened`);
    await expectFocusOn(page, J, "Wrap UploadClient.put in a retry loop");
    await expectInView(page, J);
    await expectButton(page, J, "Hand back · 1 ⌘⏎");
    expect(docFile(root), `${J} file: a note leaves the approved text`).toBe(REVERTED);
  });
});
