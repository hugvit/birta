import { Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  Box,
  GATE_ON,
  STOPS_DOC,
  State,
  blockBox,
  booted,
  draftFiles,
  fixtureDoc,
  fixtureState,
  focused,
  frameBox,
  hex,
  highlighted,
  ink,
  longDoc,
  marks,
  misaligned,
  mockReview,
  near,
  pixelsOf,
  revOf,
  reviewLog,
  shot,
  stopSp,
  stops,
  watchSent,
  withRev,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const NARROW = { width: 500, height: 900 };
const SETTLE_MS = 400;
/** Moves the composer's pick to the next or previous candidate; j and k stay text there. */
const PICK_NEXT = "ArrowDown";
const PICK_PREV = "ArrowUp";
/** The receipt keys as the copy writes them, with any run of spaces between the groups. */
const KEYS = /y keep\s+x undo\s+c instead/;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

interface OpenOpts {
  state?: State;
  states?: State[];
  acks?: "auto" | "manual";
  viewport?: { width: number; height: number };
}

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, opts: OpenOpts = {}) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(opts.viewport ?? WIDE);
  const mock = await mockReview(page, { state: opts.state, states: opts.states, acks: opts.acks });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

/** Serves `doc` with the real socket, so ops land in the review file. */
async function openReal(page: Page, birta: Start, doc: string, env: Record<string, string> = {}) {
  const served = await birta({ "pr.md": doc }, { ...GATE_ON, ...env });
  const sent = watchSent(page);
  const received = watchReceived(page);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, sent, received };
}

/** Every frame the page receives, for pages whose socket is not mocked. Call before `goto`. */
function watchReceived(page: Page): State[] {
  const got: State[] = [];
  page.on("websocket", (ws) => {
    ws.on("framereceived", (f) => {
      try {
        got.push(JSON.parse(typeof f.payload === "string" ? f.payload : f.payload.toString()));
      } catch {
        // not a JSON frame
      }
    });
  });
  return got;
}

/** Presses j until `sp` has focus, failing if the walk passes the last stop first. */
async function focusOn(page: Page, sp: string) {
  const all = await stops(page);
  const at = all.findIndex((s) => s.sp === sp);
  if (at < 0) throw new Error(`${sp} is not a stop`);
  while ((await focused(page)) !== null) await page.keyboard.press("Escape");
  // Blanks are stops too, so walk until the block has focus rather than count
  for (let i = 0; i <= at + 8 && (await focused(page)) !== sp; i++) await page.keyboard.press("j");
  await expect.poll(() => focused(page)).toBe(sp);
}

/** Presses j from nothing until the blank asking `question` holds the keyboard. */
async function focusBlank(page: Page, question: string) {
  while ((await focused(page)) !== null) await page.keyboard.press("Escape");
  const has = () => page.evaluate((q) => {
    const a = document.activeElement;
    return !!a && a.getAttribute("data-kind") === "blank" && (a.shadowRoot?.textContent ?? "").includes(q);
  }, question);
  for (let i = 0; i < 40 && !(await has()); i++) await page.keyboard.press("j");
  await expect.poll(has).toBe(true);
}

/** Selects `word` in the first text node of the block `sp` that holds it. */
async function selectWord(page: Page, sp: string, word: string) {
  await page.evaluate(
    ({ sp, word }) => {
      const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
      const el = all[all.length - 1]!;
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode() as Text | null; n; n = walker.nextNode() as Text | null) {
        const at = n.data.indexOf(word);
        if (at < 0) continue;
        const r = document.createRange();
        r.setStart(n, at);
        r.setEnd(n, at + word.length);
        getSelection()!.removeAllRanges();
        getSelection()!.addRange(r);
        return;
      }
      throw new Error(`no text node in ${sp} holds ${word}`);
    },
    { sp, word },
  );
}

/** The composer's text field, wherever it is open. */
const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"] textarea');

interface Host {
  kind: string | null;
  /** The host's rendered text, style sheets left out. */
  text: string;
  box: Box;
  /** Where the host sits: inside an item, after a block, or in the doc slot above the doc. */
  slot: { where: "in" | "after" | "doc"; sp: string | null; tag: string | null };
  /** Whether nothing but other flow hosts follows it inside its parent. */
  last: boolean;
}

/** Every `birta-flow` host in document order, read through its (forced open) root. */
function hosts(page: Page): Promise<Host[]> {
  return page.evaluate(() => {
    const content = document.getElementById("content")!;
    const isFlow = (e: Element | null) => !!e && e.localName === "birta-flow";
    return Array.from(document.querySelectorAll("birta-flow")).map((h) => {
      const root = h.shadowRoot;
      const text = root
        ? Array.from(root.children)
            .filter((c) => c.localName !== "style")
            .map((c) => (c as HTMLElement).innerText ?? c.textContent ?? "")
            .join("\n")
        : "";
      const r = h.getBoundingClientRect();
      let slot: { where: "in" | "after" | "doc"; sp: string | null; tag: string | null };
      const parent = h.parentElement!;
      if (parent.localName === "li" && parent.hasAttribute("data-sourcepos")) {
        slot = { where: "in", sp: parent.getAttribute("data-sourcepos"), tag: "li" };
      } else {
        let prev = h.previousElementSibling;
        while (prev && isFlow(prev)) prev = prev.previousElementSibling;
        slot =
          prev && content.contains(prev)
            ? { where: "after", sp: prev.getAttribute("data-sourcepos"), tag: prev.localName }
            : { where: "doc", sp: null, tag: null };
      }
      let next = h.nextElementSibling;
      while (next && isFlow(next)) next = next.nextElementSibling;
      return {
        kind: h.getAttribute("data-kind"),
        text: text.replace(/[ \t]+/g, " ").trim(),
        box: { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height },
        slot,
        last: next === null,
      };
    });
  });
}

/** The hosts that belong to the block `sp`: after it, or inside it for an item. */
async function hostsAt(page: Page, sp: string): Promise<Host[]> {
  return (await hosts(page)).filter((h) => h.slot.sp === sp);
}

async function docSlot(page: Page): Promise<Host[]> {
  return (await hosts(page)).filter((h) => h.slot.where === "doc");
}

const texts = (hs: Host[]) => hs.map((h) => h.text);

/** A rule drawn by the overlay: `data-mark="rule"` in `birta-over`. */
interface Rule extends Box {
  style: string | null;
  role: string | null;
}

function rules(page: Page): Promise<Rule[]> {
  return page.evaluate(() => {
    const root = document.querySelector("#lane-frame > birta-over")?.shadowRoot;
    if (!root) return [];
    return Array.from(root.querySelectorAll('[data-mark="rule"]')).map((el) => {
      const r = el.getBoundingClientRect();
      return {
        style: el.getAttribute("data-style"),
        role: el.getAttribute("data-role"),
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
        width: r.width,
        height: r.height,
      };
    });
  });
}

/** The rule that runs down the side of `host`, if any. */
async function ruleOf(page: Page, host: Host): Promise<Rule | null> {
  const all = await rules(page);
  return all.find((r) => Math.abs(r.top - host.box.top) <= 1 && Math.abs(r.bottom - host.box.bottom) <= 1) ?? null;
}

/** The rule beside the host whose text holds `text`, as style and role; drawn a frame after the host. */
async function ruleFor(page: Page, text: string): Promise<{ style: string | null; role: string | null } | null> {
  const host = (await hosts(page)).find((h) => h.text.includes(text));
  const rule = host ? await ruleOf(page, host) : null;
  return rule && { style: rule.style, role: rule.role };
}

/** The left edge of the first line of text inside the host whose text holds `text`. */
function textLeft(page: Page, text: string): Promise<number | null> {
  return page.evaluate((text) => {
    for (const h of document.querySelectorAll("birta-flow")) {
      const root = h.shadowRoot;
      if (!root) continue;
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
        acceptNode: (n) =>
          n.parentElement?.closest("style") || !(n.textContent ?? "").trim()
            ? NodeFilter.FILTER_SKIP
            : NodeFilter.FILTER_ACCEPT,
      });
      const first = walker.nextNode();
      if (!first || !(root.textContent ?? "").includes(text)) continue;
      const r = document.createRange();
      r.selectNodeContents(first);
      return r.getClientRects()[0]?.left ?? null;
    }
    return null;
  }, text);
}

/** `HH:MM` for an epoch-ms time, in the page's own time zone. */
function hhmm(page: Page, ts: number): Promise<string> {
  return page.evaluate((ts) => {
    const d = new Date(ts);
    return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  }, ts);
}

/** A state for `doc` holding only the given notes, each cloned from a drafts fixture item. */
function notesState(
  doc: string,
  notes: { sp: string | null; body: string; n?: number; status?: string; verdict?: string; scope?: string }[],
): State {
  const s = fixtureState("drafts");
  const block = s.items.find((i: State) => i.at.scope === "block");
  const whole = s.items.find((i: State) => i.at.kind === "doc");
  s.rev = revOf(doc);
  s.items = notes.map((note, i) => {
    const base = JSON.parse(JSON.stringify(note.sp === null ? whole : block));
    const n = note.n ?? 40 + i;
    const item: State = { ...base, n, cid: `c-${n}`, body: note.body, status: note.status ?? "draft" };
    if (note.verdict) item.verdict = note.verdict;
    else delete item.verdict;
    if (note.sp !== null) item.at = { ...base.at, scope: note.scope ?? "block", loc: { sp: note.sp } };
    return item;
  });
  return s;
}

const FLOW_DOC = fixtureDoc("flow");

/** What the viewer's own keys (r d t u) toggle. */
function viewerState(page: Page) {
  return page.evaluate(() => ({
    reading: document.body.classList.contains("reading-mode"),
    theme: document.documentElement.getAttribute("data-theme"),
    menu: document.getElementById("theme-dropdown")!.classList.contains("open"),
    raw: getComputedStyle(document.getElementById("source-view")!).display !== "none",
  }));
}

// --- the composer ----------------------------------------------------------------------------

test.describe("composer", () => {
  test("c keeps a note on the doc, a block, a heading's section and a selection with ⏎", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    const heading = await stopSp(page, "What changes");
    const keep = async (body: string) => {
      await page.keyboard.press("c");
      await expect(composer(page)).toBeFocused();
      await page.keyboard.type(body);
      await page.keyboard.press("Enter");
      await expect(composer(page)).toHaveCount(0);
    };
    await keep("whole doc");
    await focusOn(page, para);
    await keep("on the paragraph");
    await focusOn(page, heading);
    await keep("on the section");
    await selectWord(page, para, "upload");
    await keep("on the word");
    await expect.poll(() => mock.ops().length).toBe(4);
    const ops = mock.ops();
    expect(ops.map((o) => [o.op, o.body])).toEqual([
      ["comment", "whole doc"],
      ["comment", "on the paragraph"],
      ["comment", "on the section"],
      ["comment", "on the word"],
    ]);
    expect(ops[0].at).toStrictEqual({ k: "doc" });
    expect(ops[1].at).toStrictEqual({ k: "block", sp: para, scope: "block" });
    expect(ops[2].at).toStrictEqual({ k: "block", sp: heading, scope: "section" });
    expect(ops[3].at).toMatchObject({ k: "quote", sp: para, q: "upload", off: 9 });
    for (const op of ops) expect(op.verdict).toBeUndefined();
  });

  test("after ⏎ focus rests on the note's block, and on the doc for a whole-doc note", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "Detail words.");
    await focusOn(page, para);
    await page.keyboard.press("c");
    await page.keyboard.type("on the block");
    await page.keyboard.press("Enter");
    await expect(composer(page)).toHaveCount(0);
    expect(await focused(page)).toBe(para);
    await page.keyboard.press("Escape");
    await expect.poll(() => focused(page)).toBeNull();
    await page.keyboard.press("c");
    await page.keyboard.type("on the doc");
    await page.keyboard.press("Enter");
    await expect(composer(page)).toHaveCount(0);
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBeNull();
  });

  test("q opens a one-line composer and keeps an ask", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const item = await stopSp(page, "backoff of 1, 2 and 4 seconds");
    await focusOn(page, item);
    await page.keyboard.press("q");
    await expect(composer(page)).toBeFocused();
    await expect(composer(page)).toHaveAttribute("placeholder", "explain (optional words)");
    await page.keyboard.type("why 4 seconds");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({
      op: "comment",
      verdict: "ask",
      body: "why 4 seconds",
      at: { k: "block", sp: item, scope: "block" },
    });
  });

  test("each composer says how to keep and close it, and only c's says how to hand back", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const host = page.locator('birta-flow[data-kind="composer"]');
    const mac = await page.evaluate(() => /Mac/.test(navigator.platform));
    await page.keyboard.press("c");
    await expect(host.getByText(`⏎ keep · esc close · ${mac ? "⌘⏎" : "Ctrl⏎"} hand back`, { exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(composer(page)).toHaveCount(0);
    await page.keyboard.press("q");
    await expect(host.getByText(/^⏎ keep · esc close$/)).toBeVisible();
    await expect(host.getByText(/hand back/)).toHaveCount(0);
  });

  test("review and viewer keys typed in the composer are text, and ⇧⏎ is a new line", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    await focusOn(page, para);
    const page0 = await viewerState(page);
    await page.keyboard.press("c");
    await expect(composer(page)).toBeFocused();
    const typed = "x y l . j k c q z m n p r d t u";
    await page.keyboard.type(typed);
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type("second line");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBe(para);
    expect(await viewerState(page)).toEqual(page0);
    await expect(composer(page)).toHaveValue(`${typed}\nsecond line`);
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "comment", body: `${typed}\nsecond line` });
    expect(mock.ops()[0].verdict).toBeUndefined();
  });

  test("Esc keeps a note that has text, and closes an empty one", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("keep me");
    await page.keyboard.press("Escape");
    await expect(composer(page)).toHaveCount(0);
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "comment", at: { k: "doc" }, body: "keep me" });
    for (const key of ["c", "q"]) {
      await page.keyboard.press(key);
      await expect(composer(page)).toBeFocused();
      await page.keyboard.press("Escape");
      await expect(composer(page)).toHaveCount(0);
    }
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });

  test("⌘⏎ in the composer keeps the note, then hands back a round that holds it", async ({ page, birta }) => {
    const { served, received } = await openReal(page, birta, STOPS_DOC, { BIRTA_REVIEW_HOLD_MS: "300" });
    await page.keyboard.press("c");
    await page.keyboard.type("hand this back");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect(composer(page)).toHaveCount(0);
    await expect.poll(() => (reviewLog(served.root) ?? []).map((l) => l.op).filter(Boolean)).toEqual([
      "snap",
      "owner",
      "comment",
      "submit",
    ]);
    const log = reviewLog(served.root)!;
    const note = log.findIndex((l) => l.op === "comment");
    expect(log[note]).toMatchObject({ body: "hand this back", at: "doc" });
    // Line numbers count from 1 and the header is line 1
    expect(log.find((l) => l.op === "submit").upto).toBeGreaterThanOrEqual(note + 1);
    expect(received.filter((f) => f.type === "nack")).toEqual([]);
  });

  test("a kept note shows at once as a dashed draft, and the snapshot takes its place", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC, { acks: "manual" });
    const para = await stopSp(page, "Detail words.");
    await focusOn(page, para);
    await page.keyboard.press("c");
    await page.keyboard.type("draft me");
    await page.keyboard.press("Enter");
    // Before any ack or state
    await expect.poll(async () => texts(await hostsAt(page, para))).toEqual([expect.stringContaining("draft me")]);
    const draft = (await hostsAt(page, para))[0];
    expect(draft.kind).toBe("note");
    await expect.poll(() => ruleFor(page, "draft me")).toEqual({ style: "dashed", role: "you" });

    const cid = mock.ops()[0].cid;
    mock.ack(cid);
    const state = notesState(STOPS_DOC, [{ sp: para, body: "draft me", n: 101 }]);
    state.items[0].cid = cid;
    mock.pushState(state);
    await page.waitForTimeout(SETTLE_MS);
    expect(texts(await hostsAt(page, para))).toEqual([expect.stringContaining("draft me")]);

    // Once sent, the rule is solid in your colour
    state.items[0].status = "sent";
    mock.pushState(withRev(state, state.rev));
    await expect.poll(() => ruleFor(page, "draft me")).toEqual({ style: "solid", role: "you" });
    expect(texts(await hostsAt(page, para))).toEqual([expect.stringContaining("draft me")]);
  });

  test("a nacked note comes back as unsent text, and the pill says why", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC, { acks: "manual" });
    const para = await stopSp(page, "Detail words.");
    await focusOn(page, para);
    await page.keyboard.press("c");
    await page.keyboard.type("too late");
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await hostsAt(page, para)).length).toBe(1);
    mock.nack(mock.ops()[0].cid, "changed_since");
    await expect(page.getByText("changed since · try again")).toBeVisible();
    await expect.poll(async () => texts(await hostsAt(page, para))).toEqual([
      expect.stringMatching(/too late[\s\S]*unsent · ⏎ to keep/),
    ]);
    await expect.poll(() => ruleFor(page, "too late")).toEqual({ style: "dashed", role: "you" });
    // Kept as a half-typed note too, so a reload does not lose it
    await expect.poll(() => mock.frames.map((f) => f.msg).filter((m) => m.type === "compose").at(-1)?.text).toBe("too late");
    // ⏎ on its block keeps it again
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1]).toMatchObject({ op: "comment", body: "too late", at: { k: "block", sp: para } });
  });

  test('a typed "quote" previews where it lands and keeps the note there', async ({ page, birta }) => {
    const { served, sent } = await openReal(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type('"Stacked on" remove');
    await expect(page.locator("birta-flow").getByText("→ ## Stacked on")).toBeVisible();
    expect(sent.filter((f) => f.type === "resolve").at(-1)).toMatchObject({
      path: "pr.md",
      rev: revOf(STOPS_DOC),
      text: "Stacked on",
    });
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    const note = reviewLog(served.root)![3];
    expect(note).toMatchObject({ op: "comment", body: "remove" });
    expect(note.at.lines).toEqual([31, 31]);
  });

  test("with several candidates the preview shows the first, and the pick key moves along them", async ({
    page,
    birta,
  }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    const code = await stopSp(page, "retry(upload, 3);");
    await page.keyboard.press("c");
    await page.keyboard.type('"upload" do it');
    await expect.poll(() => mock.frames.filter((f) => f.msg.type === "resolve").length).toBeGreaterThan(0);
    const ask = mock.frames.filter((f) => f.msg.type === "resolve").at(-1)!.msg;
    expect(ask).toMatchObject({ path: "pr.md", rev: revOf(STOPS_DOC), text: "upload" });
    // The shapes the server's resolve gives
    const first = { k: "quote", q: "upload", pre: "", suf: "", sp: para, off: 9 };
    const second = { k: "quote", q: "upload", pre: "", suf: "", sp: code, off: 6 };
    mock.push({
      type: "resolved",
      cid: ask.cid,
      candidates: [
        { at: first, preview: '→ "upload"' },
        { at: second, preview: '→ "retry(upload, 3);"' },
      ],
    });
    const flow = page.locator("birta-flow");
    await expect(flow.getByText('→ "upload"', { exact: true })).toBeVisible();
    await page.keyboard.press(PICK_NEXT);
    await expect(flow.getByText('→ "retry(upload, 3);"')).toBeVisible();
    await page.keyboard.press(PICK_PREV);
    await expect(flow.getByText('→ "upload"', { exact: true })).toBeVisible();
    await page.keyboard.press(PICK_NEXT);
    await expect(flow.getByText('→ "retry(upload, 3);"')).toBeVisible();
    await expect(composer(page)).toHaveValue('"upload" do it');
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "comment", body: "do it" });
    expect(mock.ops()[0].at).toStrictEqual(second);
  });

  test("a quote with no match previews the whole doc and keeps every word there", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type('"nowhere" do it');
    await expect.poll(() => mock.frames.filter((f) => f.msg.type === "resolve").length).toBeGreaterThan(0);
    const ask = mock.frames.filter((f) => f.msg.type === "resolve").at(-1)!.msg;
    mock.push({ type: "resolved", cid: ask.cid, candidates: [] });
    await expect(page.locator("birta-flow").getByText("→ no match · whole doc")).toBeVisible();
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "comment", at: { k: "doc" }, body: '"nowhere" do it' });
  });

  test("half-typed text goes out as one compose frame once typing pauses", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("half typed note", { delay: 30 });
    await page.waitForTimeout(800);
    const compose = mock.frames.filter((f) => f.msg.type === "compose").map((f) => f.msg);
    expect(compose).toHaveLength(1);
    expect(compose[0]).toMatchObject({ path: "pr.md", text: "half typed note" });
    expect(typeof compose[0].key).toBe("string");
    expect(compose[0].key.length).toBeGreaterThan(0);
  });

  test("half-typed text comes back after a reload as an unsent note, and ⏎ keeps it once", async ({
    page,
    birta,
  }) => {
    const { served } = await openReal(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("unsent words");
    await expect.poll(() => draftFiles(served.root).length).toBe(1);
    await page.reload();
    await booted(page);
    await expect.poll(async () => texts(await docSlot(page)).join("\n")).toContain("unsent words");
    const unsent = (await docSlot(page)).find((h) => h.text.includes("unsent words"))!;
    expect(unsent.text).toContain("unsent · ⏎ to keep");
    await expect.poll(() => ruleFor(page, "unsent words")).toEqual({ style: "dashed", role: "you" });
    expect(reviewLog(served.root)).toBeNull();

    await page.keyboard.press("c");
    await expect(composer(page)).toHaveCount(1);
    await expect(composer(page)).toHaveValue("unsent words");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    expect(reviewLog(served.root)![3]).toMatchObject({ op: "comment", at: "doc", body: "unsent words" });
    await expect.poll(() => draftFiles(served.root)).toEqual([]);
    await expect(page.getByText("unsent · ⏎ to keep")).toHaveCount(0);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)).toHaveLength(4);
  });

  test("a restored unsent note keeps on a lone ⏎ once it has focus", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("unsent words");
    await expect.poll(() => draftFiles(served.root).length).toBe(1);
    await page.reload();
    await booted(page);
    const unsent = page.locator("birta-flow", { hasText: "unsent · ⏎ to keep" });
    await expect(unsent).toBeVisible();
    await unsent.click();
    await expect.poll(() => page.evaluate(() => document.activeElement?.localName ?? null)).toBe("birta-flow");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    expect(reviewLog(served.root)![3]).toMatchObject({ op: "comment", at: "doc", body: "unsent words" });
    await expect.poll(() => draftFiles(served.root)).toEqual([]);
    await expect(page.getByText("unsent · ⏎ to keep")).toHaveCount(0);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)).toHaveLength(4);
  });

  test("half-typed text on a block comes back under that block after a reload", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, STOPS_DOC);
    const para = await stopSp(page, "Detail words.");
    await focusOn(page, para);
    await page.keyboard.press("c");
    await page.keyboard.type("about the detail");
    await expect.poll(() => draftFiles(served.root).length).toBe(1);
    await page.reload();
    await booted(page);
    await expect.poll(async () => texts(await hostsAt(page, para)).join("\n")).toContain("about the detail");
    const unsent = (await hostsAt(page, para)).find((h) => h.text.includes("about the detail"))!;
    expect(unsent.slot.where).toBe("after");
    expect(unsent.text).toContain("unsent · ⏎ to keep");
    expect(texts(await docSlot(page)).join("\n")).not.toContain("about the detail");
    expect(reviewLog(served.root)).toBeNull();
  });
});

// --- where flow elements go ------------------------------------------------------------------

test.describe("placement", () => {
  test("a note on a list item goes last in the item, after its nested list", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const item = await stopSp(page, "backoff of 1, 2 and 4 seconds");
    mock.pushState(notesState(STOPS_DOC, [{ sp: item, body: "on the item" }]));
    await expect.poll(async () => texts(await hostsAt(page, item))).toEqual([expect.stringContaining("on the item")]);
    const host = (await hostsAt(page, item))[0];
    expect(host.slot.where).toBe("in");
    expect(host.last).toBe(true);
    const nested = await page.evaluate((sp) => {
      const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
      return all[all.length - 1]!.querySelector(":scope > ul")!.getBoundingClientRect().bottom;
    }, item);
    expect(host.box.top).toBeGreaterThanOrEqual(nested - 1);
  });

  test("a note on a table row goes after the table, not inside it", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const row = await stopSp(page, "404");
    mock.pushState(notesState(STOPS_DOC, [{ sp: row, body: "on the row" }]));
    await expect
      .poll(async () => (await hosts(page)).filter((h) => h.text.includes("on the row")).map((h) => h.slot.tag))
      .toEqual(["table"]);
    expect(await page.evaluate(() => document.querySelectorAll("#content table birta-flow").length)).toBe(0);
  });

  test("a list item's bar, wash and focus end at its own lines, above the notes inside it", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    const item = "11:1-11:44";
    await expect.poll(async () => (await hostsAt(page, item)).length).toBe(2);
    const noteTop = Math.min(...(await hostsAt(page, item)).map((h) => h.box.top));
    await expect.poll(async () => (await marks(page)).filter((m) => m.sp === item).map((m) => m.mark).sort()).toEqual([
      "bar",
      "glyph",
      "wash",
    ]);
    for (const m of (await marks(page)).filter((m) => m.sp === item && m.mark !== "glyph"))
      expect(m.bottom, `${m.mark} bottom`).toBeLessThanOrEqual(noteTop);
    expect(await misaligned(page)).toEqual([]);
    await focusOn(page, item);
    const focus = (await marks(page)).find((m) => m.mark === "focus")!;
    expect(focus.bottom).toBeLessThanOrEqual(noteTop);
  });

  test("a whole-doc note sits above the doc's first block", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    mock.pushState(notesState(STOPS_DOC, [{ sp: null, body: "on the whole doc" }]));
    await expect.poll(async () => texts(await docSlot(page))).toEqual([expect.stringContaining("on the whole doc")]);
    const host = (await docSlot(page))[0];
    const first = await page.evaluate(() => document.querySelector("#content [data-sourcepos]")!.getBoundingClientRect().top);
    expect(host.box.bottom).toBeLessThanOrEqual(first + 1);
  });

  for (const variant of ["light", "dark"]) {
    test(`${variant}: notes in the doc slot use the page's font, as notes under a block do`, async ({
      page,
      birta,
    }) => {
      await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
      const html = page.locator("html");
      if ((await html.getAttribute("data-theme")) !== variant) await page.locator("#theme-toggle").click();
      await expect(html).toHaveAttribute("data-theme", variant);
      await expect.poll(async () => (await docSlot(page)).length).toBeGreaterThanOrEqual(3);
      const fonts = await page.evaluate(() =>
        Array.from(document.querySelectorAll("birta-flow")).map((h) => {
          const root = h.shadowRoot!;
          const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
            acceptNode: (n) =>
              n.parentElement?.closest("style") || !(n.textContent ?? "").trim()
                ? NodeFilter.FILTER_SKIP
                : NodeFilter.FILTER_ACCEPT,
          });
          const first = walker.nextNode();
          return {
            doc: h.parentElement!.id === "lane-frame",
            font: first ? getComputedStyle(first.parentElement!).fontFamily : null,
          };
        }),
      );
      const body = await page.evaluate(() => getComputedStyle(document.querySelector("#content p")!).fontFamily);
      const inBlock = fonts.filter((f) => !f.doc).map((f) => f.font);
      const doc = fonts.filter((f) => f.doc).map((f) => f.font);
      expect(inBlock.length).toBeGreaterThan(0);
      expect(new Set(inBlock)).toEqual(new Set([body]));
      expect(doc).toEqual(doc.map(() => body));
    });
  }

  test("the doc slot leaves the first heading the gap a note group leaves above a heading", async ({
    page,
    birta,
  }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => (await docSlot(page)).length).toBeGreaterThanOrEqual(3);
    const gaps = await page.evaluate(() => {
      const isHeading = (e: Element | null) => !!e && /^h[1-6]$/.test(e.localName);
      const flows = Array.from(document.querySelectorAll("birta-flow"));
      const lastDoc = flows.filter((h) => h.parentElement!.id === "lane-frame").pop()!;
      const first = document.querySelector("#content > [data-sourcepos]")!;
      const beforeHeading = flows.find((h) => h.parentElement!.id === "content" && isHeading(h.nextElementSibling))!;
      return {
        firstIsHeading: isHeading(first),
        doc: first.getBoundingClientRect().top - lastDoc.getBoundingClientRect().bottom,
        inBlock: beforeHeading.nextElementSibling!.getBoundingClientRect().top - beforeHeading.getBoundingClientRect().bottom,
      };
    });
    expect(gaps.firstIsHeading).toBe(true);
    expect(gaps.inBlock).toBeGreaterThan(8);
    expect(Math.abs(gaps.doc - gaps.inBlock)).toBeLessThanOrEqual(0.5);
  });

  test("notes follow their paragraph in note order, and a section note follows its heading", async ({
    page,
    birta,
  }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    const heading = await stopSp(page, "What changes");
    mock.pushState(
      notesState(STOPS_DOC, [
        { sp: para, body: "second on the paragraph", n: 45 },
        { sp: heading, body: "on the section", scope: "section", verdict: "yes", n: 46 },
        { sp: para, body: "first on the paragraph", n: 44 },
      ]),
    );
    await expect
      .poll(async () => texts(await hostsAt(page, para)))
      .toEqual([expect.stringContaining("first on the paragraph"), expect.stringContaining("second on the paragraph")]);
    expect((await hostsAt(page, para)).every((h) => h.slot.where === "after")).toBe(true);
    const section = await hostsAt(page, heading);
    expect(texts(section)).toEqual([expect.stringContaining("on the section")]);
    expect(section[0].slot.tag).toBe("h2");
  });

  test("a state push keeps each note's host and moves it, rather than making a new one", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "Detail words.");
    const heading = await stopSp(page, "Stacked on");
    mock.pushState(notesState(STOPS_DOC, [{ sp: para, body: "the kept host", n: 44 }]));
    await expect.poll(async () => texts(await hostsAt(page, para))).toEqual([expect.stringContaining("the kept host")]);
    const probe = (mark: boolean) =>
      page.evaluate((mark) => {
        const host = Array.from(document.querySelectorAll("birta-flow")).find((h) =>
          (h.shadowRoot?.textContent ?? "").includes("the kept host"),
        ) as (Element & { probe?: number }) | undefined;
        if (!host) return null;
        if (mark) host.probe = 7;
        return host.probe ?? 0;
      }, mark);
    expect(await probe(true)).toBe(7);

    // A note before it on the same block, then the note moves to another block
    mock.pushState(
      notesState(STOPS_DOC, [
        { sp: para, body: "a newer one above", n: 43 },
        { sp: para, body: "the kept host", n: 44 },
      ]),
    );
    await expect
      .poll(async () => texts(await hostsAt(page, para)))
      .toEqual([expect.stringContaining("a newer one above"), expect.stringContaining("the kept host")]);
    expect(await probe(false)).toBe(7);
    mock.pushState(notesState(STOPS_DOC, [{ sp: heading, body: "the kept host", n: 44 }]));
    await expect.poll(async () => texts(await hostsAt(page, heading))).toEqual([expect.stringContaining("the kept host")]);
    expect(await hostsAt(page, para)).toEqual([]);
    expect(await probe(false)).toBe(7);
  });

  test("under one block the note comes first, then its reply, then a blank", async ({ page, birta }) => {
    const state = fixtureState("flow");
    const blank = state.items.find((i: State) => i.status === "open");
    blank.at.loc = { sp: "11:1-11:44" };
    await open(page, birta, FLOW_DOC, { state });
    await expect.poll(async () => (await hostsAt(page, "11:1-11:44")).length).toBe(3);
    const at = await hostsAt(page, "11:1-11:44");
    expect(at.map((h) => h.kind)).toEqual(["note", "reply", "blank"]);
    expect(at[0].text).toContain("cap it");
    expect(at[2].text).toContain("is 503 the only code?");
  });

  test("a note on a selection inside raw HTML says what it points at, and underlines nothing", async ({
    page,
    birta,
  }) => {
    const { served } = await openReal(page, birta, STOPS_DOC);
    await page.evaluate(() => {
      const div = Array.from(document.querySelectorAll("#content div")).find((d) => d.textContent === "raw html block")!;
      const t = div.firstChild as Text;
      const r = document.createRange();
      r.setStart(t, 0);
      r.setEnd(t, "raw html".length);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    });
    await page.keyboard.press("c");
    await page.keyboard.type("why raw");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    await expect
      .poll(async () => (await hosts(page)).filter((h) => h.text.includes("why raw")).map((h) => h.text))
      .toEqual([expect.stringContaining('→ "raw html')]);
    expect(await page.getByText(/lost (its|their) place/).count()).toBe(0);
    expect(await highlighted(page)).toEqual([]);
  });
});

// --- replies, receipts and facts -------------------------------------------------------------

test.describe("replies and receipts", () => {
  test("a fix under your note reads as the agent's reply with the receipt keys, and no separate receipt", async ({
    page,
    birta,
  }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => (await hostsAt(page, "11:1-11:44")).length).toBe(2);
    const [note, reply] = await hostsAt(page, "11:1-11:44");
    expect(note.text).toContain("cap it");
    expect(reply.text).toMatch(/^↳ Claude · done\b/);
    expect(reply.text).toMatch(KEYS);
    expect(texts(await hosts(page)).filter((t) => t.includes('because you said "cap it"'))).toEqual([]);
  });

  test("a fix cited from a note elsewhere carries a receipt on its own block", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => (await hostsAt(page, "5:1-6:68")).length).toBe(1);
    const [receipt] = await hostsAt(page, "5:1-6:68");
    expect(receipt.kind).toBe("receipt");
    expect(receipt.text).toMatch(/^Claude · because you said "say why it fails" · 1 line → 1\s+·?\s*y keep\s+x undo\s+c instead$/);
  });

  test("a note the agent's turn left alone says so, with the time the turn ended", async ({ page, birta }) => {
    const state = fixtureState("flow");
    // Hours apart, so the send, the pickup and the reply each give their own HH:MM
    const HOUR = 3_600_000;
    state.phrase.sent.ts -= 3 * HOUR;
    state.rounds[0].ts -= 3 * HOUR;
    state.phrase.picked_up.ts -= 2 * HOUR;
    state.phrase.picked_up.silence.since_ts -= 2 * HOUR;
    for (const item of state.items) item.ts -= 3 * HOUR;
    await open(page, birta, FLOW_DOC, { state });
    const at = await hhmm(page, state.phrase.replied.ts);
    for (const other of [state.phrase.sent.ts, state.phrase.picked_up.ts]) expect(await hhmm(page, other)).not.toBe(at);
    await expect
      .poll(async () => texts(await hostsAt(page, "10:1-10:16")).join("\n"))
      .toContain(`↳ not handled in Claude's ${at} turn`);
  });

  test("a note an earlier turn left alone names that turn without a time", async ({ page, birta }) => {
    const state = fixtureState("flow");
    // The note went out in an earlier round, whose turn the state gives no time for
    state.rounds.unshift({ submit: 2, ts: state.rounds[0].ts, by: "", to: "*", items: [7], claim: 3, claimed_by: "claude", yield: 4, released: false });
    state.items.find((i: State) => i.n === 7).turn = 3;
    await open(page, birta, FLOW_DOC, { state });
    await expect
      .poll(async () => texts(await hostsAt(page, "10:1-10:16")).join("\n"))
      .toContain("↳ not handled in Claude's earlier turn");
    expect(texts(await hostsAt(page, "10:1-10:16")).join("\n")).not.toMatch(/\d\d:\d\d/);
  });

  test("a note done with no change says so", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => texts(await hostsAt(page, "12:1-13:0")).join("\n")).toContain("done · no change in text");
  });

  test("writes from outside birta say so, and name git when git made them", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => texts(await hostsAt(page, "8:1-8:15")).join("\n")).toContain("changed outside birta (git)");
    const plain = texts(await hostsAt(page, "16:1-16:19")).join("\n");
    expect(plain).toContain("changed outside birta");
    expect(plain).not.toContain("(git)");
  });

  test("notes that lost their place are one row in the doc slot", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => texts(await docSlot(page)).join("\n")).toContain("2 notes lost their place ▸");
    await expect(page.getByText("cut this")).toBeHidden();
    await expect(page.getByText("and this")).toBeHidden();
  });

  test("the agent's note on its turn sits in the doc slot", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect
      .poll(async () => (await docSlot(page)).filter((h) => h.kind === "turn_note").map((h) => h.text))
      .toEqual([expect.stringContaining("Capped the backoff.")]);
    expect(texts(await hosts(page)).filter((t) => t.includes("Capped the backoff."))).toHaveLength(1);
  });

  test("an answer to your doc note sits under it in the doc slot", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect
      .poll(async () => {
        const slot = await docSlot(page);
        const note = slot.findIndex((h) => h.text.includes("is the test ladder still needed?"));
        return note < 0 ? null : slot[note + 1]?.text ?? null;
      })
      .toMatch(/^↳ Claude\b.*Yes: CI runs it on every push\./s);
  });

  test("m hides the lane's marks and leaves the notes in the flow", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    const note = page.locator('birta-flow[data-kind="note"]', { hasText: "cap it" });
    await expect(note).toBeVisible();
    const bar = async () => (await marks(page)).find((m) => m.mark === "bar" && m.sp === "11:1-11:44")?.width ?? null;
    await expect.poll(bar).toBeGreaterThan(0);
    const before = await note.boundingBox();
    await page.keyboard.press("m");
    // Control: the lane really hid
    await expect.poll(bar).toBe(0);
    await expect(note).toBeVisible();
    await expect(page.locator('birta-flow[data-kind="reply"]', { hasText: "no change in text" })).toBeVisible();
    await expect(page.locator('birta-flow[data-kind="blank"]', { hasText: "is 503 the only code?" })).toBeVisible();
    expect(await note.boundingBox()).toEqual(before);
  });

  test("a reply longer than two lines folds, and ▸ more opens it", async ({ page, birta }) => {
    const state = fixtureState("flow");
    const long = Array.from({ length: 30 }, (_, i) => `Sentence ${i + 1} of a long answer about the ladder.`).join(" ");
    state.items.find((i: State) => i.n === 8).replies[0].body = long;
    await open(page, birta, FLOW_DOC, { state });
    const reply = page.locator('birta-flow[data-kind="reply"]', { hasText: "Sentence 1 of" });
    await expect(reply.getByText("▸ more")).toBeVisible();
    const folded = (await reply.boundingBox())!.height;
    const line = await page.evaluate(() => parseFloat(getComputedStyle(document.body).lineHeight) || 24);
    expect(folded).toBeLessThanOrEqual(3 * line);
    await reply.getByText("▸ more").click();
    await expect.poll(async () => (await reply.boundingBox())!.height).toBeGreaterThan(folded + line);
  });

  test("a reply from the page with no name is yours, and nothing shows an empty name", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => texts(await hostsAt(page, "1:1-1:16")).join("\n")).toMatch(/↳ you\b.*yes, keep it/s);
    const all = texts(await hosts(page));
    expect(all.filter((t) => /undefined|null|someone|↳ ·|^·/.test(t))).toEqual([]);
  });
});

// --- blanks ----------------------------------------------------------------------------------

test.describe("blanks", () => {
  const BLANK = 21;

  test("an open blank shows the agent's question and how to answer it", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => (await hostsAt(page, "3:1-3:6")).map((h) => h.kind)).toEqual(["blank"]);
    const [blank] = await hostsAt(page, "3:1-3:6");
    expect(blank.text).toContain("is 503 the only code?");
    expect(blank.text).toContain("type an answer · y yes · x no · . your call · l later");
  });

  test("opening the lost row redraws that row alone, and an answer being typed is saved as the field is left", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await page.locator('birta-flow[data-kind="blank"]', { hasText: "is 503 the only code?" }).click();
    await page.keyboard.type("half an answer");
    const answer = page.locator('birta-flow[data-kind="blank"] textarea');
    await expect(answer).toHaveValue("half an answer");
    // A mark on every other host's drawn line: a redraw would replace it
    const marked = await page.evaluate(() =>
      Array.from(document.querySelectorAll("birta-flow:not([data-kind=lost])")).filter((h) => {
        const line = h.shadowRoot!.querySelector(".flow > div");
        if (line) (line as HTMLElement).dataset.probe = "1";
        return !!line;
      }).length,
    );
    expect(marked).toBeGreaterThan(1);
    await page.locator('birta-flow[data-kind="lost"] .more').click();
    await expect(page.getByText("cut this")).toBeVisible();
    // Leaving the field keeps its words, as Esc does
    await expect.poll(() => mock.ops().map((o) => [o.op, o.body])).toEqual([["reply", "half an answer"]]);
    const kept = await page.evaluate(
      () => Array.from(document.querySelectorAll("birta-flow")).filter((h) => h.shadowRoot!.querySelector('[data-probe="1"]')).length,
    );
    expect(kept).toBe(marked);
  });

  for (const [key, verdict] of [
    ["y", "yes"],
    ["x", "no"],
    [".", "your_call"],
    ["l", "later"],
  ] as const) {
    test(`a focused blank takes ${key} as the answer ${verdict}`, async ({ page, birta }) => {
      const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
      await focusBlank(page, "is 503 the only code?");
      await page.keyboard.press(key);
      await expect.poll(() => mock.ops().length).toBe(1);
      expect(mock.ops()[0]).toMatchObject({ op: "reply", to: BLANK, verdict });
      expect(mock.ops()[0].body).toBeUndefined();
    });
  }

  test("after the fifth open blank the rest show only their question, and the slot comes on focus", async ({
    page,
    birta,
  }) => {
    const state = fixtureState("flow");
    const base = state.items.find((i: State) => i.status === "open");
    const sps = ["1:1-1:16", "3:1-3:6", "5:1-6:68", "8:1-8:15", "10:1-10:16", "11:1-11:44", "14:1-14:21"];
    state.items = sps.map((sp, k) => ({
      ...JSON.parse(JSON.stringify(base)),
      n: 21 + k,
      body: `question ${k + 1}`,
      at: { ...base.at, loc: { sp } },
    }));
    state.changes = [];
    state.deletions = [];
    state.lost = [];
    const { mock } = await open(page, birta, FLOW_DOC, { state });
    const blanks = async () =>
      (await hosts(page)).filter((h) => h.kind === "blank").map((h) => ({ q: h.text.match(/question \d/)?.[0], hint: h.text.includes("type an answer · y yes · x no · . your call · l later") }));
    await expect.poll(blanks).toEqual([
      { q: "question 1", hint: true },
      { q: "question 2", hint: true },
      { q: "question 3", hint: true },
      { q: "question 4", hint: true },
      { q: "question 5", hint: true },
      { q: "question 6", hint: false },
      { q: "question 7", hint: false },
    ]);
    await focusBlank(page, "question 6");
    await expect.poll(async () => (await blanks())[5].hint).toBe(true);
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "reply", to: 26, verdict: "yes" });
  });

  test("an answered blank stays a blank until its answer is handed back, then settles", async ({ page, birta }) => {
    // Item 22 is answered by reply 25, which no hand-back covers yet
    const state = fixtureState("flow");
    const { mock } = await open(page, birta, FLOW_DOC, { state });
    const at = async () => (await hostsAt(page, "1:1-1:16")).map((h) => [h.kind, h.text]);
    await expect.poll(at).toEqual([
      ["blank", expect.stringContaining("keep this title?")],
      ["reply", expect.stringMatching(/↳ you · yes, keep it/)],
    ]);
    // A second answer amends the first, and a key's answer replaces typed words
    await focusBlank(page, "keep this title?");
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(opFields(mock.ops()[0])).toStrictEqual({ op: "amend", to: 25, verdict: "no", body: "" });

    // Handed back: a settled note, where only y (seen) acts. The mock resends its state on
    // any content the server repeats, so the hand-back goes into that same state
    state.rounds.push(handedBack(state, 26));
    mock.pushState(state);
    await expect.poll(async () => (await at()).map((x) => x[0])).toEqual(["note", "reply"]);
    const note = page.locator('birta-flow[data-kind="note"]', { hasText: "keep this title?" });
    await note.click();
    await page.keyboard.press("x");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
    await note.click();
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1]).toMatchObject({ op: "seen", ids: ["#22"] });
  });

  test("on a real server a draft answer can be changed, and once handed back y settles it", async ({ page, birta }) => {
    const T0 = Date.now() - 600_000;
    const human = { type: "human", via: "browser" };
    const agent = { type: "agent", name: "claude", via: "cli" };
    const lines: State[] = [
      { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
      { by: { type: "birta" }, op: "snap", rev: revOf(STOPS_DOC), text: STOPS_DOC },
      { by: { type: "birta" }, op: "owner", drafted_by: null },
      { by: human, cid: "c-4", op: "comment", at: "doc", body: "look it over" },
      { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
      { by: agent, op: "claim", round: 5 },
      {
        by: agent,
        op: "comment",
        at: { quote: "Detail words.", pre: "", suf: "", lines: [29, 29], rev: revOf(STOPS_DOC), scope: "block" },
        verdict: "ask",
        body: "only here?",
        turn: 6,
      },
      { by: agent, op: "done", to: 4, turn: 6 },
      { by: agent, op: "yield", turn: 6 },
    ];
    const log = lines.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join("");
    const served = await birta({ "pr.md": STOPS_DOC, "pr.md.birta.jsonl": log }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const para = await stopSp(page, "Detail words.");
    const blank = page.locator('birta-flow[data-kind="blank"]', { hasText: "only here?" });
    await focusBlank(page, "only here?");
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "reply", to: 7, verdict: "yes" });
    const answer = reviewLog(served.root)!.length;
    // Still a draft answer: still a blank, and another key amends it
    await expect.poll(async () => texts(await hostsAt(page, para)).join("\n")).toMatch(/↳ you · yes/);
    await expect(blank).toBeVisible();
    await page.keyboard.press("x");
    await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "amend", to: answer, verdict: "no" });
    // The latest answer is the one shown
    await expect.poll(async () => texts(await hostsAt(page, para)).join("\n")).toMatch(/↳ you · no/);
    expect(texts(await hostsAt(page, para)).join("\n")).not.toMatch(/↳ you · yes/);

    // The hand-back, as ⌘⏎ would write it
    const n = reviewLog(served.root)!.length;
    const submit = { by: human, op: "submit", upto: n, not_before: Date.now() - 1_000, hold_ms: 0, to: "*", ts: Date.now() - 1_000 };
    fs.appendFileSync(path.join(served.root, "pr.md.birta.jsonl"), JSON.stringify(submit) + "\n");
    const note = page.locator('birta-flow[data-kind="note"]', { hasText: "only here?" });
    await expect(note).toBeVisible();
    await expect(blank).toHaveCount(0);
    await note.click();
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "seen", ids: ["#7"] });
    await expect(note).toHaveCount(0);
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "dot").map((m) => m.sp)).toEqual([para]);
  });

  test("another letter says how to type; ⏎ opens the field, and ⏎ there keeps the words", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await focusBlank(page, "is 503 the only code?");
    await page.keyboard.press("o");
    await expect(page.getByText("⏎ to type an answer")).toBeVisible();
    await page.keyboard.press("Enter");
    await page.keyboard.type("only 503, yes");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "reply", to: BLANK, body: "only 503, yes" });
    expect(mock.ops()[0].verdict).toBeUndefined();
  });
});

// --- verdicts on a noted block, and z --------------------------------------------------------

/** A later hand-back on line `submit`, not yet picked up. */
function handedBack(state: State, submit: number): State {
  const round = { ...JSON.parse(JSON.stringify(state.rounds.at(-1))), submit, ts: state.server_now - 60_000, items: [] };
  for (const k of ["claim", "claimed_by", "yield"]) delete round[k];
  return round;
}

/** A seen op for changes only, which a j walk past changed blocks sends. */
const changeSeen = (o: State) => o.op === "seen" && o.ids.every((id: string) => /^\d+\.\d+$/.test(id));

/** An op frame without the envelope the core adds. */
function opFields(frame: State): State {
  const out = { ...frame };
  for (const k of ["type", "cid", "path", "rev"]) delete out[k];
  return out;
}

/** drafts.json with its item verdict on "- retries on 503" and no body, so a key can clear it. */
function bareVerdict(): State {
  const state = fixtureState("drafts");
  delete state.items.find((i: State) => i.n === 5).body;
  return state;
}

test.describe("verdict keys on a block that has one", () => {
  test("another key replaces the verdict", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: bareVerdict() });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "amend", to: 5, verdict: "yes" });
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });

  test("the same key retracts a verdict with no words", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: bareVerdict() });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "retract", to: 5 });
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });

  test("the same key drops a verdict that has words and keeps them, and z puts it back", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(opFields(mock.ops()[0])).toStrictEqual({ op: "amend", to: 5, drop_verdict: true });
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(opFields(mock.ops()[1])).toStrictEqual({ op: "amend", to: 5, verdict: "no" });
  });

  test("c on a block with a verdict writes the note onto that verdict", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: bareVerdict() });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toBeFocused();
    await page.keyboard.type("why not");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "amend", to: 5, body: "why not" });
    expect(mock.ops()[0].verdict).toBeUndefined();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });
});

test.describe("z", () => {
  test("during a hold z withdraws the hand-back", async ({ page, birta }) => {
    const state = fixtureState("hold");
    state.hold.not_before = state.server_now + 30_000;
    const { mock } = await open(page, birta, fixtureDoc("hold"), { state });
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "withdraw", to: state.hold.n });
  });

  test("once the hold has passed z withdraws nothing", async ({ page, birta }) => {
    const state = fixtureState("hold");
    state.hold.not_before = state.server_now - 1_000;
    const { mock } = await open(page, birta, fixtureDoc("hold"), { state });
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("z takes back a note you just kept", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("take me back");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    // The mock acks with n = 101
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1]).toMatchObject({ op: "retract", to: 101 });
  });

  test("z puts back a verdict you just replaced", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: bareVerdict() });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1]).toMatchObject({ op: "amend", to: 5, verdict: "no" });
  });

  test("z brings back a verdict you just cleared", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: bareVerdict() });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "retract", to: 5 });
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1]).toMatchObject({
      op: "comment",
      verdict: "no",
      at: { k: "block", sp: "10:1-10:16", scope: "block" },
    });
    expect(mock.ops()[1].body).toBeUndefined();
  });
});

// --- the rewrite row -------------------------------------------------------------------------

test.describe("rewrite row", () => {
  const TURN_CHANGES = ["10.0", "10.1", "10.2", "10.3", "9.0"];

  test("a rewrite shows one row at the top, and y there marks the turn's changes seen", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("rewrite"), { state: fixtureState("rewrite") });
    await expect
      .poll(async () => (await docSlot(page)).filter((h) => h.kind === "rewrite").map((h) => h.text))
      .toEqual(["Claude rewrote most of it · y mark seen"]);
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].op).toBe("seen");
    expect([...mock.ops()[0].ids].sort()).toEqual(TURN_CHANGES);
  });

  test("a click on the rewrite row marks them seen too", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("rewrite"), { state: fixtureState("rewrite") });
    await page.locator('birta-flow[data-kind="rewrite"]').click();
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].op).toBe("seen");
    expect([...mock.ops()[0].ids].sort()).toEqual(TURN_CHANGES);
  });
});

// --- note dots -------------------------------------------------------------------------------

test.describe("note dots", () => {
  /** flow.json with `ns` marked seen, and the answer to item 22 handed back. */
  function seenState(...ns: number[]): State {
    const state = fixtureState("flow");
    for (const n of ns) state.items.find((i: State) => i.n === n).seen = true;
    state.rounds.push(handedBack(state, 26));
    return state;
  }

  const dots = async (page: Page) => (await marks(page)).filter((m) => m.mark === "dot");

  test("a seen note that was handled or answered folds to a dot in the right padding", async ({ page, birta }) => {
    // 6: handled, yours. 22: the agent's question, answered. 7: not handled, so it stays open
    await open(page, birta, FLOW_DOC, { state: seenState(6, 22, 7) });
    await expect.poll(async () => (await dots(page)).map((d) => [d.sp, d.role]).sort()).toEqual([
      ["11:1-11:44", "you"],
      ["1:1-1:16", "agent"],
    ]);
    // Folded with its replies, and not shown twice
    expect(await hostsAt(page, "11:1-11:44")).toEqual([]);
    expect(texts(await hostsAt(page, "1:1-1:16")).join("\n")).not.toContain("keep this title?");
    expect(texts(await hostsAt(page, "10:1-10:16")).join("\n")).toContain("drop this bullet");
    const frame = await frameBox(page);
    for (const d of await dots(page)) {
      expect(Math.abs(d.width - 8), `${d.sp} width`).toBeLessThanOrEqual(0.5);
      expect(Math.abs(d.height - 8), `${d.sp} height`).toBeLessThanOrEqual(0.5);
      expect(Math.abs((d.left + d.right) / 2 - (frame.right - 20)), `${d.sp} x`).toBeLessThanOrEqual(1);
      const b = (await blockBox(page, d.sp!))!;
      const cy = (d.top + d.bottom) / 2;
      expect(cy >= b.line.top && cy <= b.line.bottom, `${d.sp} on its first line`).toBe(true);
    }
    // Yours is solid accent, the agent's a hollow grey ring
    const bg = hex("#ffffff");
    const s = await shot(page, frame);
    const mine = (await dots(page)).find((d) => d.role === "you")!;
    const theirs = (await dots(page)).find((d) => d.role === "agent")!;
    expect(near(s.at((mine.left + mine.right) / 2, (mine.top + mine.bottom) / 2), hex("#0969da"), 12)).toBe(true);
    expect(near(s.at((theirs.left + theirs.right) / 2, (theirs.top + theirs.bottom) / 2), bg, 12)).toBe(true);
    expect(ink(pixelsOf(s, theirs), bg)).not.toBeNull();
    // m hides dots with the rest of the lane
    await page.keyboard.press("m");
    await expect.poll(async () => (await dots(page)).map((d) => d.width)).toEqual([0, 0]);
  });

  test("an open note shows no dot", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await expect.poll(async () => (await hostsAt(page, "11:1-11:44")).length).toBe(2);
    expect(await dots(page)).toEqual([]);
  });

  test("y on a focused handled note sends seen with the note's own id", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await page.locator('birta-flow[data-kind="note"]', { hasText: "cap it" }).click();
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "seen", ids: ["#6"] });
  });

  test("⏎ on a folded note's block opens it here and sends nothing", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: seenState(6) });
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    await focusOn(page, "11:1-11:44");
    await page.keyboard.press("Enter");
    await expect.poll(async () => texts(await hostsAt(page, "11:1-11:44")).join("\n")).toContain("cap it");
    await expect.poll(async () => (await dots(page)).length).toBe(0);
    await page.waitForTimeout(SETTLE_MS);
    // The walk there steps past changed blocks, which marks them seen; a note's own seen still counts
    expect(mock.ops().filter((o) => !changeSeen(o))).toEqual([]);
  });

  test("a note folded by y stays folded after a reload, and a new reply opens it again", async ({ page, birta }) => {
    const T0 = Date.now() - 600_000;
    const human = { type: "human", via: "browser" };
    const agent = { type: "agent", name: "claude", via: "cli" };
    const lines: State[] = [
      { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
      { by: { type: "birta" }, op: "snap", rev: revOf(STOPS_DOC), text: STOPS_DOC },
      { by: { type: "birta" }, op: "owner", drafted_by: null },
      {
        by: human,
        cid: "c-4",
        op: "comment",
        at: { quote: "Detail words.", pre: "", suf: "", lines: [29, 29], rev: revOf(STOPS_DOC), scope: "block" },
        body: "tidy this",
      },
      { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
      { by: agent, op: "claim", round: 5 },
      { by: agent, op: "done", to: 4, turn: 6 },
      { by: agent, op: "yield", turn: 6 },
    ];
    const log = lines.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join("");
    const served = await birta({ "pr.md": STOPS_DOC, "pr.md.birta.jsonl": log }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const note = page.locator('birta-flow[data-kind="note"]', { hasText: "tidy this" });
    await expect(note).toBeVisible();
    await note.click();
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "seen", ids: ["#4"] });
    await expect(note).toHaveCount(0);
    await expect.poll(async () => (await dots(page)).length).toBe(1);

    await page.reload();
    await booted(page);
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    await expect(page.locator("birta-flow", { hasText: "tidy this" })).toHaveCount(0);

    // A later reply is news, so the note opens again
    const n = reviewLog(served.root)!.length + 1;
    const reply = { by: agent, op: "reply", to: 4, kind: "note", body: "one more thing", late: true, ts: Date.now() };
    fs.appendFileSync(path.join(served.root, "pr.md.birta.jsonl"), JSON.stringify(reply) + "\n");
    await expect(page.locator("birta-flow", { hasText: "tidy this" })).toBeVisible();
    await expect(page.locator("birta-flow", { hasText: "one more thing" })).toBeVisible();
    await expect.poll(async () => (await dots(page)).length).toBe(0);
    expect(reviewLog(served.root)).toHaveLength(n);
  });
});

// --- rules -----------------------------------------------------------------------------------

test.describe("rules", () => {
  /** flow.json with two more notes of yours, still drafts: one on a paragraph, one in a list item. */
  function withDrafts(): State {
    const state = fixtureState("flow");
    const tidy = state.items.find((i: State) => i.n === 9);
    const draft = (n: number, sp: string, body: string) => {
      const item = { ...JSON.parse(JSON.stringify(tidy)), n, cid: `c-${n}`, body, status: "draft" };
      delete item.outcome;
      item.at.loc = { sp };
      return item;
    };
    state.items.push(draft(30, "5:1-6:68", "still a draft"), draft(31, "12:1-13:0", "a draft in a list"));
    return state;
  }

  /** Rules beside top-level hosts: after a block, or in the doc slot. */
  const TOP: [string, string, string][] = [
    ["still a draft", "dashed", "you"],
    ["is the test ladder still needed?", "solid", "you"],
    ["Yes: CI runs it on every push.", "solid", "agent"],
    ["is 503 the only code?", "dotted", "agent"],
  ];
  /** Rules beside hosts that sit last inside a list item. */
  const IN_ITEM: [string, string, string, string][] = [
    ["a draft in a list", "dashed", "you", "12:1-13:0"],
    ["cap it", "solid", "you", "11:1-11:44"],
    ["↳ Claude · done", "solid", "agent", "11:1-11:44"],
  ];

  /** The left edge of the first line of the item's own text. */
  function itemTextLeft(page: Page, sp: string): Promise<number> {
    return page.evaluate((sp) => {
      const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
      const walker = document.createTreeWalker(all[all.length - 1]!, NodeFilter.SHOW_TEXT, {
        acceptNode: (n) => ((n.textContent ?? "").trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP),
      });
      const r = document.createRange();
      r.selectNodeContents(walker.nextNode()!);
      return r.getClientRects()[0].left;
    }, sp);
  }

  async function checkRules(
    page: Page,
    want: [string, string, string, string?][],
    lane: number,
    textAt: (sp?: string) => Promise<number>,
  ) {
    const frame = await frameBox(page);
    await expect.poll(async () => (await hosts(page)).some((h) => h.text.includes(want[0][0]))).toBe(true);
    for (const [text, style, role, sp] of want) {
      await expect.poll(() => ruleFor(page, text), `${text}: its rule`).toEqual({ style, role });
      const host = (await hosts(page)).find((h) => h.text.includes(text))!;
      const rule = await ruleOf(page, host);
      expect(Math.abs((rule!.left + rule!.right) / 2 - (frame.left + lane)), `${text}: rule x`).toBeLessThanOrEqual(1);
      expect(Math.abs(rule!.width - 2), `${text}: rule width`).toBeLessThanOrEqual(0.5);
      expect(Math.abs((await textLeft(page, text))! - (await textAt(sp))), `${text}: text x`).toBeLessThanOrEqual(1);
    }
  }

  for (const [width, viewport, lane, textX] of [
    ["at 1280px", WIDE, 20, 52],
    ["under 768px", NARROW, 12, 28],
    ["at 768px", { width: 768, height: 900 }, 20, 52],
    ["at 767px", { width: 767, height: 900 }, 12, 28],
  ] as const) {
    test(`${width} a note's rule is at x=${lane} and its text at x=${textX}`, async ({ page, birta }) => {
      await open(page, birta, FLOW_DOC, { state: withDrafts(), viewport });
      const frame = await frameBox(page);
      await checkRules(page, TOP, lane, async () => frame.left + textX);
    });

    // The note is the item's last child, so its text keeps the item's indent, 12px in at both widths
    test(`${width} a note inside a list item has its rule at x=${lane} and its text indented with the item`, async ({
      page,
      birta,
    }) => {
      await open(page, birta, FLOW_DOC, { state: withDrafts(), viewport });
      await checkRules(page, IN_ITEM, lane, async (sp) => (await itemTextLeft(page, sp!)) + 12);
    });
  }
});

// --- live changes while the flow is open -----------------------------------------------------

/** Marks the open composer's textarea and selects part of it, so a recreated one shows. */
function tagComposer(page: Page) {
  return page.evaluate(() => {
    const host = document.querySelector('birta-flow[data-kind="composer"]')!;
    const area = host.shadowRoot!.querySelector("textarea") as HTMLTextAreaElement & { probe?: number };
    area.probe = 7;
    area.setSelectionRange(2, 6);
  });
}

/** The tagged composer's node, text, selection and focus, or null when it is gone. */
function sameComposer(page: Page) {
  return page.evaluate(() => {
    const host = document.querySelector('birta-flow[data-kind="composer"]');
    const area = host?.shadowRoot?.querySelector("textarea") as (HTMLTextAreaElement & { probe?: number }) | null;
    return area
      ? {
          probe: area.probe,
          value: area.value,
          sel: [area.selectionStart, area.selectionEnd],
          focused: host!.shadowRoot!.activeElement === area && document.activeElement === host,
        }
      : null;
  });
}

const COMPOSER_WANT = { probe: 7, value: "half a thought", sel: [2, 6], focused: true };

test.describe("patches", () => {
  test("a patch or a state while composing keeps the composer: node, focus, text and caret", async ({
    page,
    birta,
  }) => {
    const { served, mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "Detail words.");
    await focusOn(page, para);
    await page.keyboard.press("c");
    await page.keyboard.type("half a thought");
    const same = () => sameComposer(page);
    const want = COMPOSER_WANT;
    await tagComposer(page);

    // An edit above the note: the patcher runs while the composer is open
    served.write("pr.md", STOPS_DOC.replace("## Why\n", "## Why\n\nA new first paragraph.\n"));
    await expect(page.locator("#content p", { hasText: "A new first paragraph." })).toBeAttached();
    expect(await same()).toEqual(want);

    // A state push reconciles the flow around the open composer
    mock.pushState(notesState(STOPS_DOC.replace("## Why\n", "## Why\n\nA new first paragraph.\n"), [{ sp: null, body: "another note" }]));
    await expect.poll(async () => texts(await docSlot(page)).join("\n")).toContain("another note");
    expect(await same()).toEqual(want);

    // Mid-composition an edit may wait, but the composer and what it holds survive
    await page.evaluate(() => {
      const area = document.querySelector('birta-flow[data-kind="composer"]')!.shadowRoot!.querySelector("textarea")!;
      area.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, composed: true, data: "" }));
    });
    served.write("pr.md", STOPS_DOC.replace("## Why\n", "## Why\n\nA second new paragraph.\n"));
    await page.waitForTimeout(SETTLE_MS);
    expect(await same()).toEqual(want);
    await page.evaluate(() => {
      const area = document.querySelector('birta-flow[data-kind="composer"]')!.shadowRoot!.querySelector("textarea")!;
      area.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, composed: true, data: "" }));
    });
    await expect(page.locator("#content p", { hasText: "A second new paragraph." })).toBeAttached();
    expect(await same()).toEqual(want);
  });

  test("a note inserted above the viewport does not move what you are reading", async ({ page, birta }) => {
    const doc = longDoc();
    const { mock } = await open(page, birta, doc);
    const reading = page.locator("#content p", { hasText: "Filler paragraph 20." });
    await reading.evaluate((el) => window.scrollBy(0, el.getBoundingClientRect().top - 300));
    await page.waitForTimeout(SETTLE_MS);
    const before = (await reading.boundingBox())!.y;
    const above = await stopSp(page, "Detail words.");
    mock.pushState(
      notesState(doc, [
        { sp: above, body: "a note above the viewport" },
        { sp: above, body: "and another one, so the insert is tall" },
      ]),
    );
    await expect.poll(async () => (await hostsAt(page, above)).length).toBe(2);
    const inserted = await hostsAt(page, above);
    // Controls: the insert is real height, and it is above what is read
    expect(inserted.reduce((h, x) => h + x.box.height, 0)).toBeGreaterThan(40);
    expect(inserted[1].box.bottom).toBeLessThan(0);
    await page.waitForTimeout(SETTLE_MS);
    expect(Math.abs((await reading.boundingBox())!.y - before)).toBeLessThanOrEqual(1);
  });
});

// --- adversary killers and the lead's rulings on them -----------------------------------------

test.describe("names, verdicts and z", () => {
  test("replies, receipts and not-handled rows name the agent that took the turn", async ({ page, birta }) => {
    const state = JSON.parse(JSON.stringify(fixtureState("flow")).replaceAll('"claude"', '"codex"'));
    await open(page, birta, FLOW_DOC, { state });
    await expect.poll(async () => texts(await hostsAt(page, "11:1-11:44")).join("\n")).toMatch(/↳ codex · done/i);
    const all = texts(await hosts(page)).join("\n");
    expect(all).toMatch(/not handled in codex's/i);
    expect(all).toMatch(/codex · because you said/i);
    expect(all).not.toMatch(/claude/i);
  });

  test("a verdict key on a block whose verdict was sent never edits the sent one", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    // Item 7: no, sent, not handled
    await focusOn(page, "10:1-10:16");
    for (const key of ["y", "x"]) {
      await page.keyboard.press(key);
      await page.waitForTimeout(SETTLE_MS);
    }
    expect(mock.ops().filter((o) => o.to === 7)).toEqual([]);
    // The walk there steps past changed blocks, which marks them seen
    expect(mock.ops().filter((o) => !changeSeen(o))[0]).toMatchObject({ op: "comment", verdict: "yes", at: { k: "block", sp: "10:1-10:16" } });
  });

  test("c on a verdict that has words opens them in the composer, and ⏎ sends the edit", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet");
    await page.keyboard.press("End");
    await page.keyboard.type(" now");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(opFields(mock.ops()[0])).toStrictEqual({ op: "amend", to: 5, body: "drop this bullet now" });
  });

  test("z twice takes back your last two notes, newest first", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    for (const body of ["first", "second"]) {
      await page.keyboard.press("c");
      await page.keyboard.type(body);
      await page.keyboard.press("Enter");
      await expect(composer(page)).toHaveCount(0);
    }
    await expect.poll(() => mock.ops().length).toBe(2);
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(3);
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(4);
    expect(mock.ops().slice(2)).toMatchObject([
      { op: "retract", to: 102 },
      { op: "retract", to: 101 },
    ]);
  });

  test("z with no act of yours on this page does nothing, whatever drafts the state holds", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts") });
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("z does not take back a note once it is sent", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("gone out");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    const state = notesState(STOPS_DOC, [{ sp: null, body: "gone out", n: 101, status: "sent" }]);
    state.items[0].cid = mock.ops()[0].cid;
    mock.pushState(state);
    await expect.poll(async () => texts(await docSlot(page)).join("\n")).toContain("gone out");
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });

  test("z before the ack takes the note back once the ack comes", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC, { acks: "manual" });
    await page.keyboard.press("c");
    await page.keyboard.type("quick");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
    mock.ack(mock.ops()[0].cid);
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1]).toMatchObject({ op: "retract", to: 101 });
  });

  test("z before a nack takes back nothing", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC, { acks: "manual" });
    await page.keyboard.press("c");
    await page.keyboard.type("quick");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.keyboard.press("z");
    mock.nack(mock.ops()[0].cid, "changed_since");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });

  test("a blank's answer is on the z stack, so z takes it back", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await focusBlank(page, "is 503 the only code?");
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.locator("#content h1").click();
    await page.keyboard.press("z");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops().map((o) => [o.op, o.to, o.verdict])).toEqual([
      ["reply", 21, "yes"],
      ["retract", 101, undefined],
    ]);
  });

  test("a verdict key on a heading answers the heading, not the blank under it", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await focusOn(page, "3:1-3:6");
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "comment", verdict: "yes", at: { k: "block", sp: "3:1-3:6", scope: "section" } });
  });

  test("Esc keeps a written blank answer, as ⏎ does", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    await page.locator('birta-flow[data-kind="blank"]', { hasText: "is 503 the only code?" }).click();
    await page.keyboard.type("only 503");
    await page.keyboard.press("Escape");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "reply", to: 21, body: "only 503" });
  });
});

test.describe("the composer, harder cases", () => {
  test("⏎ before the quote is resolved sends the quote for the server to find", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type('"upload" do it');
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].at).toStrictEqual({ k: "quote", q: "upload", pre: "", suf: "" });
    expect(mock.ops()[0].body).toBe("do it");
  });

  test("a late answer for a quote since changed is neither previewed nor used", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "When the upload");
    const resolves = () => mock.frames.filter((f) => f.msg.type === "resolve").map((f) => f.msg);
    await page.keyboard.press("c");
    await page.keyboard.type('"upload" x');
    await expect.poll(() => resolves().length).toBe(1);
    const stale = resolves()[0];
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.type('"Detail" x');
    await expect.poll(() => resolves().length).toBe(2);
    const stalePick = { k: "quote", q: "upload", pre: "", suf: "", sp: para, off: 9 };
    // The current answer, then the stale one late
    mock.push({ type: "resolved", cid: resolves()[1].cid, candidates: [] });
    await expect(page.locator("birta-flow").getByText("→ no match · whole doc")).toBeVisible();
    mock.push({ type: "resolved", cid: stale.cid, candidates: [{ at: stalePick, preview: '→ "upload"' }] });
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.locator("birta-flow").getByText("→ no match · whole doc")).toBeVisible();
    await expect(page.locator("birta-flow").getByText('→ "upload"')).toHaveCount(0);
    // With the quote gone, the last answer's candidates are not used either
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.type("plain words");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "comment", at: { k: "doc" }, body: "plain words" });
  });

  test("⏎ and Esc during an IME composition neither keep nor close", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("にほ");
    await page.evaluate(() => {
      const area = document.querySelector('birta-flow[data-kind="composer"]')!.shadowRoot!.querySelector("textarea")!;
      for (const key of ["Enter", "Escape"]) {
        area.dispatchEvent(new KeyboardEvent("keydown", { key, isComposing: true, bubbles: true, composed: true, cancelable: true }));
        const e = new KeyboardEvent("keydown", { key, bubbles: true, composed: true, cancelable: true });
        Object.defineProperty(e, "keyCode", { value: 229 });
        area.dispatchEvent(e);
      }
    });
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await expect(composer(page)).toHaveValue("にほ");
  });

  test("⌘⏎ whose note is nacked hands nothing back", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC, { acks: "manual" });
    await page.keyboard.press("c");
    await page.keyboard.type("hand this back");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().some((o) => o.op === "comment")).toBe(true);
    mock.nack(mock.ops().find((o) => o.op === "comment")!.cid, "changed_since");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().map((o) => o.op)).toEqual(["comment"]);
  });

  test("a note kept straight after typing leaves no half-typed copy behind", async ({ page, birta }) => {
    const { served, sent } = await openReal(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("fast note");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    await page.waitForTimeout(800);
    const kept = sent.findIndex((f) => f.type === "op" && f.op === "comment");
    expect(sent.slice(kept).filter((f) => f.type === "compose" && f.text)).toEqual([]);
    expect(draftFiles(served.root)).toEqual([]);
    await page.reload();
    await booted(page);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.getByText("unsent · ⏎ to keep")).toHaveCount(0);
  });

  test("compose_restore replaces what it restored, and another doc's is ignored", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    // The key is the client's own: take it from a compose frame, then empty and close
    await page.keyboard.press("c");
    await page.keyboard.type("k");
    await expect.poll(() => mock.frames.filter((f) => f.msg.type === "compose").length).toBeGreaterThan(0);
    const key = mock.frames.find((f) => f.msg.type === "compose")!.msg.key;
    await page.keyboard.press("Backspace");
    await page.keyboard.press("Escape");
    await expect(composer(page)).toHaveCount(0);
    const restore = (path: string, text: string) => mock.push({ type: "compose_restore", path, items: [{ key, text }] });
    restore(mock.path, "restored words");
    restore(mock.path, "restored words");
    restore("other.md", "another doc's words");
    await expect.poll(async () => texts(await hosts(page)).filter((t) => t.includes("restored words")).length).toBe(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(texts(await hosts(page)).filter((t) => t.includes("restored words"))).toHaveLength(1);
    expect(texts(await hosts(page)).join("\n")).not.toContain("another doc's words");
  });

  test("a lone ⏎ on the doc keeps its unsent note, with no composer", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, STOPS_DOC);
    await page.keyboard.press("c");
    await page.keyboard.type("unsent words");
    await expect.poll(() => draftFiles(served.root).length).toBe(1);
    await page.reload();
    await booted(page);
    await expect.poll(async () => texts(await docSlot(page)).join("\n")).toContain("unsent words");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    expect(reviewLog(served.root)![3]).toMatchObject({ op: "comment", at: "doc", body: "unsent words" });
    await expect(composer(page)).toHaveCount(0);
    await expect.poll(() => draftFiles(served.root)).toEqual([]);
  });

  test("a half-typed ask comes back as an ask", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, STOPS_DOC);
    const para = await stopSp(page, "Detail words.");
    await focusOn(page, para);
    await page.keyboard.press("q");
    await page.keyboard.type("why here");
    await expect.poll(() => draftFiles(served.root).length).toBe(1);
    await page.reload();
    await booted(page);
    await expect.poll(async () => texts(await hostsAt(page, para)).join("\n")).toContain("why here");
    await focusOn(page, para);
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    expect(reviewLog(served.root)![3]).toMatchObject({ op: "comment", verdict: "ask", body: "why here" });
  });

  test("a note added to the composer's own block keeps the composer", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "Detail words.");
    await focusOn(page, para);
    await page.keyboard.press("c");
    await page.keyboard.type("half a thought");
    await tagComposer(page);
    mock.pushState(notesState(STOPS_DOC, [{ sp: para, body: "a note beside the composer" }]));
    await expect.poll(async () => texts(await hostsAt(page, para)).join("\n")).toContain("a note beside the composer");
    expect(await sameComposer(page)).toEqual(COMPOSER_WANT);
  });

  test("a patch to the list item a composer sits in keeps the composer, in that item", async ({ page, birta }) => {
    const { served } = await open(page, birta, STOPS_DOC);
    await focusOn(page, await stopSp(page, "metrics dashboard"));
    await page.keyboard.press("c");
    await page.keyboard.type("half a thought");
    await tagComposer(page);
    served.write("pr.md", STOPS_DOC.replace("- metrics dashboard", "- metrics dashboard, per bucket"));
    await expect(page.locator("#content li", { hasText: "per bucket" })).toBeAttached();
    await expect.poll(() => sameComposer(page)).toEqual(COMPOSER_WANT);
    expect(
      await page.evaluate(
        () =>
          document.querySelector('birta-flow[data-kind="composer"]')!.closest("li")?.textContent?.includes("per bucket") ??
          false,
      ),
    ).toBe(true);
  });
});

test.describe("a note kept after the doc moved under it", () => {
  const PARAS = "# Title\n\nAlpha has the word fix.\n\nBravo has the word fix.\n\nCharlie para.\n";
  // The new paragraph takes Alpha's lines, and Alpha takes Bravo's sp
  const PARAS_GREW = PARAS.replace("Alpha", "Zerox para.\n\nAlpha");
  const LIST = "# Title\n\n- [ ] task alpha\n- [ ] task bravo\n- [ ] task charl\n\nDetail words.\n";
  // The list is one block, so its items are all replaced, and alpha takes bravo's sp
  const LIST_GREW = LIST.replace("- [ ] task alpha", "- [ ] task zerox\n- [ ] task alpha");
  // As LIST_GREW, and bravo's own words change, so nothing in the doc is bravo any more
  const LIST_MOVED = LIST_GREW.replace("task bravo", "task bravo, reworded");
  const MOVED = "its block changed · c to re-point";

  const pill = (page: Page) => page.locator("birta-pop .pill");
  const comments = (root: string) => (reviewLog(root) ?? []).filter((l) => l.op === "comment");
  const sentComments = (sent: State[]) => sent.filter((f) => f.type === "op" && f.op === "comment");

  /** The sp the doc shows now for the innermost block holding `text`. */
  function spNow(page: Page, text: string): Promise<string | null> {
    return page.evaluate((text) => {
      const all = Array.from(document.querySelectorAll("#content [data-sourcepos]")).filter((e) =>
        (e.textContent ?? "").includes(text),
      );
      return all.at(-1)?.getAttribute("data-sourcepos") ?? null;
    }, text);
  }

  /** Walks to the block holding `text` as the doc shows it now, whatever the boot data said. */
  async function focusText(page: Page, text: string) {
    const sp = await spNow(page, text);
    while ((await focused(page)) !== null) await page.keyboard.press("Escape");
    for (let i = 0; i < 20 && (await focused(page)) !== sp; i++) await page.keyboard.press("j");
    expect(await focused(page)).toBe(sp);
  }

  test("a block note kept after a paragraph lands above it goes on its own block", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, PARAS);
    await focusOn(page, await stopSp(page, "Bravo has"));
    await page.keyboard.press("c");
    await page.keyboard.type("note meant for bravo");
    served.write("pr.md", PARAS_GREW);
    await expect(page.locator("#content")).toContainText("Zerox para.");
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    expect(comments(served.root)[0].at.quote).toContain("Bravo has");
  });

  test("a selection note kept after a paragraph lands above it goes on its own words", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, PARAS);
    await selectWord(page, await stopSp(page, "Bravo has"), "fix");
    await page.keyboard.press("c");
    await page.keyboard.type("this fix");
    served.write("pr.md", PARAS_GREW);
    await expect(page.locator("#content")).toContainText("Zerox para.");
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    const at = comments(served.root)[0].at;
    expect(at.quote).toBe("fix");
    expect(at.pre).toContain("Bravo has");
  });

  test("a picked typed quote kept after a paragraph lands above it goes where it was picked", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, PARAS);
    await page.keyboard.press("c");
    await page.keyboard.type('"fix" the second one');
    const target = page.locator('birta-flow[data-kind="composer"] .target');
    await expect(target).not.toBeEmpty();
    await page.keyboard.press(PICK_NEXT);
    await expect(target).toContainText("vo has the word");
    served.write("pr.md", PARAS_GREW);
    await expect(page.locator("#content")).toContainText("Zerox para.");
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    const note = comments(served.root)[0];
    expect(note.body).toBe("the second one");
    expect(note.at.pre).toContain("Bravo has");
  });

  test("a block note on a list item keeps to its item when a write edits a sibling", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, LIST);
    await focusOn(page, await stopSp(page, "task bravo"));
    await page.keyboard.press("c");
    await page.keyboard.type("note meant for bravo");
    served.write("pr.md", LIST.replace("task charl", "task charlie"));
    await expect(page.locator("#content")).toContainText("task charlie");
    // The list was replaced whole, and the composer is still drawn under bravo's new item
    await expect.poll(async () => (await hostsAt(page, (await spNow(page, "task bravo"))!)).map((h) => h.kind)).toEqual([
      "composer",
    ]);
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    expect(comments(served.root)[0].at.quote).toContain("task bravo");
    await expect(page.locator("birta-flow", { hasText: MOVED })).toHaveCount(0);
  });

  test("a block note on a list item keeps to its item when an item lands above it", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, LIST);
    await focusOn(page, await stopSp(page, "task bravo"));
    await page.keyboard.press("c");
    await page.keyboard.type("note meant for bravo");
    served.write("pr.md", LIST_GREW);
    await expect(page.locator("#content")).toContainText("task zerox");
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    expect(comments(served.root)[0].at.quote).toContain("task bravo");
  });

  test("a block note whose item has a twin after a write stays unsent rather than guess", async ({ page, birta }) => {
    const { served, sent } = await openReal(page, birta, LIST);
    await focusOn(page, await stopSp(page, "task bravo"));
    await page.keyboard.press("c");
    await page.keyboard.type("note meant for bravo");
    served.write("pr.md", LIST.replace("task charl", "task bravo"));
    await expect(page.locator("#content li", { hasText: "task bravo" })).toHaveCount(2);
    await page.keyboard.press("Enter");
    await expect(pill(page)).toHaveText(MOVED);
    await page.waitForTimeout(SETTLE_MS);
    expect(sentComments(sent)).toEqual([]);
  });

  test("a block note whose block a write replaced stays unsent, ⏎ sends nothing, and c re-points it", async ({
    page,
    birta,
  }) => {
    const { served, sent } = await openReal(page, birta, LIST);
    await focusOn(page, await stopSp(page, "task bravo"));
    await page.keyboard.press("c");
    await page.keyboard.type("note meant for bravo");
    served.write("pr.md", LIST_MOVED);
    await expect(page.locator("#content")).toContainText("task zerox");
    await page.keyboard.press("Enter");
    await expect(pill(page)).toHaveText(MOVED);
    const row = page.locator("birta-flow", { hasText: "note meant for bravo" });
    await expect(row).toContainText(MOVED);
    for (let i = 0; i < 2; i++) {
      await row.click();
      await page.keyboard.press("Enter");
      await expect(pill(page)).toHaveText(MOVED);
    }
    await page.waitForTimeout(SETTLE_MS);
    expect(sentComments(sent)).toEqual([]);
    await focusText(page, "task bravo");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("note meant for bravo");
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    expect(comments(served.root)[0].at.quote).toContain("task bravo");
    await expect(page.locator("birta-flow", { hasText: MOVED })).toHaveCount(0);
  });

  test("a selection note whose block a write replaced lets the server find its words", async ({ page, birta }) => {
    const { served, sent } = await openReal(page, birta, LIST);
    await selectWord(page, await stopSp(page, "task bravo"), "bravo");
    await page.keyboard.press("c");
    await page.keyboard.type("this one");
    served.write("pr.md", LIST_MOVED);
    await expect(page.locator("#content")).toContainText("task zerox");
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    const op = sentComments(sent)[0];
    expect(op.at).toMatchObject({ k: "quote", q: "bravo" });
    expect(op.at.sp).toBeUndefined();
    expect(comments(served.root)[0].at.quote).toBe("bravo");
  });

  test("half-typed words restored after a reload stay on their block when a write only moves it", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, LIST);
    await focusOn(page, await stopSp(page, "task bravo"));
    await page.keyboard.press("c");
    await page.keyboard.type("about bravo");
    await expect.poll(() => draftFiles(served.root).length).toBe(1);
    await page.reload();
    await booted(page);
    await expect(page.locator("birta-flow", { hasText: "about bravo" })).toBeVisible();
    // The list is kept whole, so its items stay and only their sps move
    served.write("pr.md", LIST.replace("# Title\n", "# Title\n\nA new opening line.\n"));
    await expect(page.locator("#content")).toContainText("A new opening line.");
    const bravo = await spNow(page, "task bravo");
    await expect.poll(async () => (await hostsAt(page, bravo!)).map((h) => h.text)).toEqual([
      expect.stringMatching(/about bravo[\s\S]*unsent · ⏎ to keep/),
    ]);
    await page.locator("birta-flow", { hasText: "about bravo" }).click();
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    expect(comments(served.root)[0].at.quote).toContain("task bravo");
  });

  test("half-typed words restored after a reload, on a block a write then replaced, never send to the wrong block", async ({
    page,
    birta,
  }) => {
    const { served, sent } = await openReal(page, birta, LIST);
    const bravo = await stopSp(page, "task bravo");
    await focusOn(page, bravo);
    await page.keyboard.press("c");
    await page.keyboard.type("about bravo");
    await expect.poll(() => draftFiles(served.root).length).toBe(1);
    await page.reload();
    await booted(page);
    served.write("pr.md", LIST_MOVED);
    await expect(page.locator("#content")).toContainText("task zerox");
    const row = page.locator("birta-flow", { hasText: "about bravo" });
    await expect(row).toContainText(MOVED);
    await row.click();
    await page.keyboard.press("Enter");
    await expect(pill(page)).toHaveText(MOVED);
    // ⏎ on what now sits at bravo's old sp is not bravo's
    await focusText(page, "task alpha");
    expect(await focused(page)).toBe(bravo);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(sentComments(sent)).toEqual([]);
    await focusText(page, "task bravo");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("about bravo");
    await page.keyboard.press("Enter");
    await expect.poll(() => comments(served.root).length).toBe(1);
    expect(comments(served.root)[0].at.quote).toContain("task bravo");
  });
});

test.describe("a nack never loses words", () => {
  test("a nacked note from a typed quote comes back with its quote", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC, { acks: "manual" });
    const para = await stopSp(page, "When the upload");
    await page.keyboard.press("c");
    await page.keyboard.type('"upload" do it');
    await expect.poll(() => mock.frames.some((f) => f.msg.type === "resolve")).toBe(true);
    const ask = mock.frames.filter((f) => f.msg.type === "resolve").at(-1)!.msg;
    const at = { k: "quote", q: "upload", pre: "", suf: "", sp: para, off: 9 };
    mock.push({ type: "resolved", cid: ask.cid, candidates: [{ at, preview: '→ "upload"' }] });
    await expect(page.locator("birta-flow").getByText('→ "upload"')).toBeVisible();
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "changed_since");
    await expect
      .poll(async () => texts(await hosts(page)).filter((t) => t.includes("unsent · ⏎ to keep")))
      .toEqual([expect.stringContaining('"upload" do it')]);
  });

  test("a nacked edit of your note's words comes back in the composer, and ⏎ edits again", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("drafts"), { state: fixtureState("drafts"), acks: "manual" });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await page.keyboard.press("End");
    await page.keyboard.type(" now");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "changed_since");
    await expect(page.getByText("changed since · try again")).toBeVisible();
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet now");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(opFields(mock.ops()[1])).toStrictEqual({ op: "amend", to: 5, body: "drop this bullet now" });
  });

  test("a nacked written blank answer comes back in its blank", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW_DOC, { state: fixtureState("flow"), acks: "manual" });
    await page.locator('birta-flow[data-kind="blank"]', { hasText: "is 503 the only code?" }).click();
    await page.keyboard.type("only 503");
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "changed_since");
    const area = page.locator('birta-flow[data-kind="blank"] textarea');
    await expect(area).toHaveValue("only 503");
    await area.focus();
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(2);
    expect(mock.ops()[1]).toMatchObject({ op: "reply", to: 21, body: "only 503" });
  });
});

test.describe("placement, harder cases", () => {
  test("a patch that shifts lines puts no note under the wrong block before the new state", async ({ page, birta }) => {
    const { served, mock } = await open(page, birta, STOPS_DOC);
    const para = await stopSp(page, "Detail words.");
    mock.pushState(notesState(STOPS_DOC, [{ sp: para, body: "about the detail" }]));
    await expect.poll(async () => texts(await hostsAt(page, para))).toEqual([expect.stringContaining("about the detail")]);
    // Every later sp moves by two, and no state for the new rev is pushed
    served.write("pr.md", STOPS_DOC.replace("## Why\n", "## Why\n\nA new first paragraph.\n"));
    await expect(page.locator("#content p", { hasText: "A new first paragraph." })).toBeAttached();
    await page.waitForTimeout(SETTLE_MS);
    const under = await page.evaluate(() => {
      const host = Array.from(document.querySelectorAll("birta-flow")).find((h) =>
        (h.shadowRoot?.textContent ?? "").includes("about the detail"),
      );
      if (!host || !host.isConnected || host.getBoundingClientRect().height === 0) return null;
      let prev = host.previousElementSibling;
      while (prev && prev.localName === "birta-flow") prev = prev.previousElementSibling;
      return prev ? prev.textContent!.trim() : "(first in its parent)";
    });
    expect([null, "Detail words."]).toContain(under);
  });

  test("a quote note on a loose list item's paragraph goes last in the item", async ({ page, birta }) => {
    const doc = "# T\n\n- first item\n\n  more of it\n\n  - nested\n\n- second item\n";
    const { mock } = await open(page, birta, doc);
    const [p, li] = await page.evaluate(() => {
      const item = document.querySelector("#content li")!;
      return [item.querySelector(":scope > p")!.getAttribute("data-sourcepos")!, item.getAttribute("data-sourcepos")!];
    });
    mock.pushState(notesState(doc, [{ sp: p, body: "on the words", scope: "quote" }]));
    await expect
      .poll(async () =>
        (await hosts(page)).filter((h) => h.text.includes("on the words")).map((h) => [h.slot.where, h.slot.sp, h.last]),
      )
      .toEqual([["in", li, true]]);
  });

  test("in a quote or an alert a note goes after the paragraph inside it, and a closed details keeps it outside", async ({
    page,
    birta,
  }) => {
    const doc = [
      "# Q",
      "",
      "> quoted words",
      "",
      "> [!NOTE]",
      "> alert words",
      "",
      "<details>",
      "<summary>More</summary>",
      "",
      "Inside details.",
      "",
      "</details>",
      "",
      "After.",
      "",
    ].join("\n");
    const { mock } = await open(page, birta, doc);
    const spOf = (text: string) =>
      page.evaluate(
        (text) => Array.from(document.querySelectorAll("#content p")).find((p) => p.textContent!.trim() === text)!.getAttribute("data-sourcepos")!,
        text,
      );
    const notes = { "quoted words": "on the quote", "alert words": "on the alert", "Inside details.": "in the details" };
    const sps: Record<string, string> = {};
    for (const text of Object.keys(notes)) sps[text] = await spOf(text);
    mock.pushState(notesState(doc, Object.entries(notes).map(([text, body]) => ({ sp: sps[text], body }))));
    const where = (body: string) =>
      page.evaluate((body) => {
        const host = Array.from(document.querySelectorAll("birta-flow")).find((h) => (h.shadowRoot?.textContent ?? "").includes(body));
        if (!host) return null;
        const prev = host.previousElementSibling;
        return {
          parent: host.parentElement!.localName + (host.parentElement!.classList.contains("markdown-alert") ? ".alert" : ""),
          prev: prev ? `${prev.localName}:${prev.textContent!.trim().slice(0, 20)}` : null,
          shown: host.getBoundingClientRect().height > 0,
        };
      }, body);
    await expect.poll(() => where("on the quote")).toEqual({ parent: "blockquote", prev: "p:quoted words", shown: true });
    const alert = await where("on the alert");
    expect(alert!.prev).toBe("p:alert words");
    expect(alert!.shown).toBe(true);
    expect(await where("in the details")).toMatchObject({ parent: "article", shown: true });
    expect((await where("in the details"))!.prev).toMatch(/^details:/);
  });

  test("a note whose place the page cannot find sits in the doc slot", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    mock.pushState(notesState(STOPS_DOC, [{ sp: "99:1-99:5", body: "placeless" }]));
    await expect.poll(async () => texts(await docSlot(page)).join("\n")).toContain("placeless");
  });

  test("the doc slot holds whole-doc notes, the turn note, the lost row, then the rewrite row", async ({ page, birta }) => {
    const state = fixtureState("flow");
    state.rewrite = { turn: 12, pct: 0.6, blocks_changed: 5, blocks_total: 8 };
    await open(page, birta, FLOW_DOC, { state });
    await expect.poll(async () => (await docSlot(page)).map((h) => h.kind)).toEqual([
      "note",
      "reply",
      "note",
      "turn_note",
      "lost",
      "rewrite",
    ]);
  });

  test("a patch with notes between blocks renders the same doc as a fresh load", async ({ page, birta }) => {
    const { served, mock } = await open(page, birta, STOPS_DOC);
    const sps = [await stopSp(page, "When the upload"), await stopSp(page, "404"), await stopSp(page, "Detail words.")];
    mock.pushState(notesState(STOPS_DOC, sps.map((sp, i) => ({ sp, body: `note ${i}` }))));
    await expect.poll(async () => (await hosts(page)).length).toBe(3);
    // Blocks the edit leaves alone keep their nodes, notes between them or not
    await page.evaluate(() => ((document.querySelector("#content table") as HTMLElement & { probe?: number }).probe = 7));
    served.write("pr.md", STOPS_DOC.replace("Detail words.", "Detail words, edited."));
    await expect(page.locator("#content p", { hasText: "Detail words, edited." })).toBeAttached();
    expect(await page.evaluate(() => (document.querySelector("#content table") as HTMLElement & { probe?: number }).probe)).toBe(7);
    served.write("pr.md", STOPS_DOC.replace("- metrics dashboard\n", "").replace("Detail words.", "Detail words, edited."));
    await expect(page.locator("#content li", { hasText: "metrics dashboard" })).toHaveCount(0);
    // Every block with its sourcepos and text, so a block kept with another's sp shows
    const patched = await page.evaluate(() =>
      Array.from(document.querySelectorAll("#content [data-sourcepos]")).map(
        (e) => `${e.localName} ${e.getAttribute("data-sourcepos")}: ${e.textContent!.trim()}`,
      ),
    );
    // A fresh render of the same bytes, uncached
    const fresh = await page.evaluate(async () => {
      const t = document.createElement("template");
      t.innerHTML = await (await fetch("/render/pr.md", { cache: "no-store" })).text();
      return Array.from(t.content.querySelectorAll("[data-sourcepos]")).map(
        (e) => `${e.localName} ${e.getAttribute("data-sourcepos")}: ${e.textContent!.trim()}`,
      );
    });
    expect(patched).toEqual(fresh);
  });
});

test.describe("copy, harder cases", () => {
  test("the reply under a fixed note says what the fix changed", async ({ page, birta }) => {
    const state = fixtureState("flow");
    await open(page, birta, FLOW_DOC, { state });
    const reply = async () => texts(await hostsAt(page, "11:1-11:44")).find((t) => t.startsWith("↳")) ?? "";
    await expect.poll(reply).toMatch(/^↳ Claude · done · 1 line → 1\s+·?\s*y keep\s+x undo\s+c instead$/);
  });

  test("a fix that changed words says how many", async ({ page, birta }) => {
    const state = fixtureState("flow");
    state.changes.find((c: State) => c.id === "15.0").what.words = 3;
    await open(page, birta, FLOW_DOC, { state });
    const reply = async () => texts(await hostsAt(page, "11:1-11:44")).find((t) => t.startsWith("↳")) ?? "";
    await expect.poll(reply).toMatch(/^↳ Claude · done · 3 words changed\s+·?\s*y keep/);
  });

  test("a receipt counts lines in the plural", async ({ page, birta }) => {
    const state = fixtureState("flow");
    Object.assign(state.changes.find((c: State) => c.id === "13.0").what, { old_lines: 9, new_lines: 4 });
    await open(page, birta, FLOW_DOC, { state });
    await expect.poll(async () => texts(await hostsAt(page, "5:1-6:68")).join("\n")).toMatch(/· 9 lines → 4\s/);
  });

  test("one lost note reads in the singular, and ▸ opens it", async ({ page, birta }) => {
    const state = fixtureState("flow");
    state.lost = [4];
    state.items = state.items.filter((i: State) => i.n !== 5);
    await open(page, birta, FLOW_DOC, { state });
    const row = page.locator('birta-flow[data-kind="lost"]');
    await expect(row).toContainText("1 note lost its place ▸");
    await expect(page.getByText("cut this")).toHaveCount(0);
    await row.getByText("▸").click();
    await expect(page.locator("birta-flow").getByText("cut this")).toBeVisible();
  });

  test("▸ on the lost row opens every lost note", async ({ page, birta }) => {
    await open(page, birta, FLOW_DOC, { state: fixtureState("flow") });
    const row = page.locator('birta-flow[data-kind="lost"]');
    await expect(row).toContainText("2 notes lost their place ▸");
    await row.getByText("▸").click();
    await expect(page.locator("birta-flow").getByText("cut this")).toBeVisible();
    await expect(page.locator("birta-flow").getByText("and this")).toBeVisible();
  });

  test("once its changes are seen the rewrite row stops offering y, and stays until the next round", async ({
    page,
    birta,
  }) => {
    const first = fixtureState("rewrite");
    const { mock } = await open(page, birta, fixtureDoc("rewrite"), { state: first });
    const row = async () => (await docSlot(page)).filter((h) => h.kind === "rewrite").map((h) => h.text);
    await expect.poll(row).toEqual(["Claude rewrote most of it · y mark seen"]);
    // The fold drops `rewrite` once the turn's changes are seen
    const seen = JSON.parse(JSON.stringify(first));
    for (const c of seen.changes) c.seen = true;
    seen.rewrite = null;
    mock.pushState(seen);
    await expect.poll(row).toEqual(["Claude rewrote most of it"]);
    const next = JSON.parse(JSON.stringify(seen));
    next.rounds.push({ ...next.rounds.at(-1), submit: 20, ts: next.rounds.at(-1).ts + 60_000, claim: undefined, claimed_by: undefined, yield: undefined });
    mock.pushState(next);
    await expect.poll(row).toEqual([]);
  });

  test("markup in bodies, names, quotes, previews and nack reasons stays text", async ({ page, birta }) => {
    const EVIL = '<img src=x onerror="window.__xss=1">';
    const state = fixtureState("flow");
    const item = (n: number) => state.items.find((i: State) => i.n === n);
    item(7).body = `b${EVIL}`;
    item(6).at.shown = `q${EVIL}`;
    item(8).replies[0].by.name = `n${EVIL}`;
    item(10).body = `c${EVIL}`;
    const { mock } = await open(page, birta, FLOW_DOC, { state, acks: "manual" });
    await page.keyboard.press("c");
    await page.keyboard.type('"upload" x');
    await expect.poll(() => mock.frames.some((f) => f.msg.type === "resolve")).toBe(true);
    const ask = mock.frames.find((f) => f.msg.type === "resolve")!.msg;
    mock.push({ type: "resolved", cid: ask.cid, candidates: [{ at: { k: "doc" }, preview: `p${EVIL}` }] });
    await expect(page.locator("birta-flow").getByText(`p${EVIL}`)).toBeVisible();
    await page.keyboard.press("Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, `r${EVIL}`);
    await page.waitForTimeout(SETTLE_MS);
    // CSP would stop the handler anyway; the count of parsed <img> is the real check
    const imgs = await page.evaluate(() => {
      const roots: (Document | ShadowRoot)[] = [document];
      for (const el of document.querySelectorAll("*")) if (el.shadowRoot) roots.push(el.shadowRoot);
      return roots.reduce((n, r) => n + r.querySelectorAll('img[src="x"]').length, 0);
    });
    expect(imgs).toBe(0);
    expect(await page.evaluate(() => (window as unknown as { __xss?: number }).__xss)).toBeUndefined();
    const all = texts(await hosts(page)).join("\n");
    for (const s of [`b${EVIL}`, `c${EVIL}`]) expect(all).toContain(s);
  });

  test("a right-to-left override in a cited note cannot reorder the receipt's keys", async ({ page, birta }) => {
    const state = fixtureState("flow");
    state.items.find((i: State) => i.n === 10).body = "‮say why it fails";
    await open(page, birta, FLOW_DOC, { state });
    await expect.poll(async () => (await hostsAt(page, "5:1-6:68")).length).toBe(1);
    const lefts = await page.evaluate(() => {
      const root = document.querySelector('birta-flow[data-kind="receipt"]')!.shadowRoot!;
      return ["y keep", "x undo", "c instead"].map((word) => {
        const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
        for (let n = w.nextNode() as Text | null; n; n = w.nextNode() as Text | null) {
          const at = n.data.indexOf(word);
          if (at < 0) continue;
          const r = document.createRange();
          r.setStart(n, at);
          r.setEnd(n, at + 1);
          return r.getBoundingClientRect().left;
        }
        return NaN;
      });
    });
    expect(lefts[0]).toBeLessThan(lefts[1]);
    expect(lefts[1]).toBeLessThan(lefts[2]);
  });

  test("a short reply of many lines folds, a one-line reply does not, and a note keeps its line breaks", async ({
    page,
    birta,
  }) => {
    const state = fixtureState("flow");
    state.items.find((i: State) => i.n === 8).replies[0].body = "one\ntwo\nthree\nfour";
    state.items.find((i: State) => i.n === 7).body = "first line\nsecond line";
    const short = { ...JSON.parse(JSON.stringify(state.items.find((i: State) => i.n === 8).replies[0])), n: 19, id: "#19", body: "short answer" };
    state.items.find((i: State) => i.n === 10).replies = [short];
    await open(page, birta, FLOW_DOC, { state });
    await expect(page.locator('birta-flow[data-kind="reply"]', { hasText: "three" }).getByText("▸ more")).toBeVisible();
    const one = page.locator('birta-flow[data-kind="reply"]', { hasText: "short answer" });
    await expect(one).toBeVisible();
    await expect(one.getByText("▸ more")).toBeHidden();
    await expect
      .poll(async () => (await hosts(page)).find((h) => h.text.includes("first line"))?.text ?? "")
      .toMatch(/first line\n+second line/);
  });
});

test.describe("rewrite row, other turns", () => {
  function withOthers(): State {
    const state = fixtureState("rewrite");
    const base = state.changes[0];
    state.changes.push(
      { ...JSON.parse(JSON.stringify(base)), id: "3.0", op: 3, turn: 2 },
      { ...JSON.parse(JSON.stringify(base)), id: "12.0", op: 12, turn: undefined, attr: "outside", by: { type: "birta" } },
    );
    return state;
  }

  test("y on the rewrite row marks only that turn's changes seen", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("rewrite"), { state: withOthers() });
    await expect.poll(async () => (await docSlot(page)).some((h) => h.kind === "rewrite")).toBe(true);
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect([...mock.ops()[0].ids].sort()).toEqual(["10.0", "10.1", "10.2", "10.3", "9.0"]);
  });

  test("y with a block focused is that block's verdict, not the rewrite's seen", async ({ page, birta }) => {
    const state = fixtureState("rewrite");
    const { mock } = await open(page, birta, fixtureDoc("rewrite"), { state });
    // Not the whole-doc H1, whose y is the doc's, and no changed block, whose row takes y
    const changed = new Set(state.changes.map((c: State) => c.range && c.range.sp));
    const first = (await stops(page)).slice(1).find((s) => !changed.has(s.sp))!.sp;
    await focusOn(page, first);
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "comment", verdict: "yes", at: { k: "block", sp: first } });
  });
});
