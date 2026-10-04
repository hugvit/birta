import { Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  Box,
  GATE_ON,
  Mock,
  OPEN_ROOTS,
  STOPS_DOC,
  State,
  booted,
  cacheCdn,
  fixtureDoc,
  fixtureState,
  focused,
  marks,
  mockReview,
  revOf,
  reviewLog,
  stops,
  withRev,
  stripKeys,
  stripMessage,
  expectSheetWords,
} from "./harness";

// The phrase prints local times; UTC lets the spec name them from the epoch alone
test.use({ timezoneId: "UTC" });

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
const MIN = 60_000;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** HH:MM of an epoch ms in UTC, as the phrase prints it under the spec's timezone. */
const hhmm = (ts: number) => new Date(ts).toISOString().slice(11, 16);

/** The key hint the turn button shows on this platform. */
function hint(page: Page): Promise<string> {
  return page.evaluate(() => (/Mac/.test(navigator.platform) ? "⌘⏎" : "Ctrl⏎"));
}

interface Chrome {
  phrase: { text: string; title: string | null; box: Box } | null;
  turn: { text: string; kbd: string | null; disabled: boolean; title: string | null; box: Box } | null;
  /** The strip's message while one shows. */
  message: string | null;
  /** The menu's rows while it is open in the top layer. */
  menu: string[] | null;
}

/** What the header and the popovers show, read through the (forced open) shadow roots. */
function chrome(page: Page): Promise<Chrome> {
  return page.evaluate(() => {
    const textOf = (n: Node | null): string => {
      let out = "";
      const walk = (x: Node) => {
        if (x.nodeType === 3) {
          out += x.textContent;
          return;
        }
        if (x.nodeType === 1 && ["STYLE", "SCRIPT"].includes((x as Element).tagName)) return;
        const root = x.nodeType === 1 ? (x as Element).shadowRoot : null;
        for (const c of Array.from((root ?? x).childNodes)) walk(c);
      };
      if (n) walk(n);
      return out.replace(/\s+/g, " ").trim();
    };
    const box = (el: Element) => {
      const r = el.getBoundingClientRect();
      return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
    };
    const shown = (el: Element | null): el is Element =>
      !!el && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
    // Open in the top layer, and painted: a popover inside a hidden host is open but unseen
    const open = (el: Element) => {
      try {
        return el.matches(":popover-open") && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
      } catch {
        return false;
      }
    };
    const roots = (sel: string) =>
      Array.from(document.querySelectorAll(sel)).map((h) => (h.shadowRoot ?? h) as ParentNode);

    const p = document.querySelector(".file-header birta-phrase");
    const phrase = shown(p) ? { text: textOf(p), title: p.getAttribute("title"), box: box(p) } : null;

    const t = document.querySelector(".file-header birta-turn");
    let turn = null;
    const btn = shown(t) ? (t.shadowRoot ?? t).querySelector("button") : null;
    if (btn) {
      const all = textOf(btn);
      const kbd = btn.querySelector("kbd") ? textOf(btn.querySelector("kbd")) : null;
      const text = kbd && all.endsWith(kbd) ? all.slice(0, -kbd.length).trim() : all;
      turn = { text, kbd, disabled: btn.disabled || btn.getAttribute("aria-disabled") === "true", title: btn.getAttribute("title"), box: box(btn) };
    }

    // A message shows in the strip's keys part while the host carries data-message
    const stripHost = document.querySelector("birta-strip");
    const keysPart = stripHost && stripHost.getClientRects().length && stripHost.hasAttribute("data-message")
      ? (stripHost.shadowRoot?.querySelector('[data-part="keys"]') ?? stripHost.querySelector('[data-part="keys"]'))
      : null;
    const message: string | null = keysPart ? textOf(keysPart) || null : null;
    let menu: string[] | null = null;
    for (const root of roots("birta-pop")) {
      for (const el of Array.from(root.querySelectorAll('[role="menu"]')))
        if (open(el)) menu = Array.from(el.querySelectorAll('[role="menuitem"]')).map((r) => textOf(r));
    }
    return { phrase, turn, message, menu };
  });
}

const phraseOf = async (page: Page) => (await chrome(page)).phrase?.text ?? null;
const menuOf = async (page: Page) => (await chrome(page)).menu;
const turnOf = async (page: Page) => {
  const t = (await chrome(page)).turn;
  return t && { text: t.text, kbd: t.kbd, disabled: t.disabled };
};

interface OpenOpts {
  state?: State;
  acks?: "auto" | "manual";
  viewport?: { width: number; height: number };
}

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, opts: OpenOpts = {}) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(opts.viewport ?? WIDE);
  const mock = await mockReview(page, { state: opts.state, acks: opts.acks });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

/** Serves committed fixture `name` with its state, bent by `edit`. The folder state has no doc of its own. */
async function openFixture(page: Page, birta: Start, name: string, edit?: (s: State) => void, opts: OpenOpts = {}) {
  const doc = name === "dir" ? STOPS_DOC : fixtureDoc(name);
  const state = name === "dir" ? withRev(fixtureState(name), revOf(doc)) : fixtureState(name);
  edit?.(state);
  const out = await open(page, birta, doc, { ...opts, state });
  return { ...out, state };
}

/**
 * Turns the mock's own state object into `next` and pushes it. The mock resends that object
 * whenever the server repeats `content`, so a pushed copy alone could be undone.
 */
function swap(mock: Mock, state: State, next: State) {
  for (const k of Object.keys(state)) delete state[k];
  Object.assign(state, JSON.parse(JSON.stringify(next)));
  mock.pushState(state);
}

/** Serves `files` with the real socket; `pr.md` is the page. */
async function openReal(page: Page, birta: Start, doc: string, env: Record<string, string> = {}, args?: string[]) {
  const served = await birta({ "pr.md": doc }, { ...GATE_ON, ...env }, args);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return served;
}

/** A second tab on the same server, able to read the chrome's roots. */
async function secondPage(page: Page, url: string): Promise<Page> {
  const other = await page.context().newPage();
  await cacheCdn(other);
  await other.addInitScript(OPEN_ROOTS);
  await other.setViewportSize(WIDE);
  await other.goto(url);
  await booted(other);
  return other;
}

/** Clicks the phrase and waits until its menu is open and painted. */
async function openMenu(page: Page) {
  await page.locator(".file-header birta-phrase").click();
  await expect.poll(() => menuOf(page)).not.toBeNull();
}

const glyphRoles = async (page: Page) =>
  (await marks(page))
    .filter((m) => m.mark === "glyph")
    .sort((a, b) => a.top - b.top)
    .map((m) => m.role);

const WITHDRAW = "withdraw";
const COPY = "copy hand-off line";
const MINE = "drafted by Claude · mark as yours";
// The row's label and its key, which this probe reads as one run of text
const KEYS = "keys?";

// --- The phrase and the turn button ----------------------------------------------------------

test.describe("phrase and button", () => {
  const SHOWN: [string, string, string, "hint" | null, boolean][] = [
    ["fresh", "drafted by Claude · your turn", "Approve", "hint", false],
    // The phrase never counts your notes; the button does
    ["drafts", "drafted by Claude · your turn", "Hand back · 3", "hint", false],
    ["dir", "read-only · notes only", "Hand back · 2", "hint", false],
    // Nothing picked up the hand-back in its first 10 s
    ["sent", "sent 14:13 · not picked up yet", "Copy hand-off line", "hint", false],
    ["picked", "Claude picked up 14:13", "With Claude", null, true],
    // One count: the stops left to check; the guard counts for Approve
    ["replied", "Claude replied 14:13 · 3 to check", "Approve", "hint", false],
    ["approved", "approved 14:13 · c to reopen", "Approved ✓", null, true],
  ];
  for (const [name, phrase, label, kbd, disabled] of SHOWN) {
    test(`${name}.json reads "${phrase}" beside "${label}"`, async ({ page, birta }) => {
      await openFixture(page, birta, name);
      await expect.poll(() => phraseOf(page)).toBe(phrase);
      expect(await turnOf(page)).toEqual({ text: label, kbd: kbd && (await hint(page)), disabled });
    });
  }

  test("a long phrase wraps to a second header line, never cut short, and keeps clear of the button", async ({ page, birta }) => {
    const name = "an-agent-with-a-name-far-longer-than-any-header-has-room-for-even-on-a-wide-screen";
    const full = "An-agent-with-a-name-far-longer-than-any-header-has-room-for-even-on-a-wide-screen replied 14:13 · 3 to check";
    await openFixture(page, birta, "replied", (s) => {
      s.phrase.picked_up.by = s.phrase.replied.by = s.rounds[0].claimed_by = name;
      Object.assign(s.phrase.replied, { done: 12, answered: 3, asked: 2, unasked: 4, not_handled: 5 });
    });
    await expect.poll(() => phraseOf(page)).toBe(full);
    const c = await chrome(page);
    expect(c.phrase!.title).toBe(full);
    expect(c.phrase!.box.right).toBeLessThanOrEqual(c.turn!.box.left);
    const laid = await page.evaluate((full) => {
      const host = document.querySelector(".file-header birta-phrase")!;
      const root = host.shadowRoot ?? host;
      const els = [host, ...Array.from(root.querySelectorAll("*"))];
      const cut = els.some((el) => el.scrollWidth > el.clientWidth + 1);
      // The words on one line, in the phrase's own font, against the room the header leaves them
      const words = (root.querySelector("*:not(style)") ?? host) as Element;
      const ctx = document.createElement("canvas").getContext("2d")!;
      ctx.font = getComputedStyle(words).font;
      const room = document.querySelector(".file-header birta-turn")!.getBoundingClientRect().left - host.getBoundingClientRect().left;
      // Each line of the words is its own run of client rects
      const r = document.createRange();
      r.selectNodeContents(root);
      const tops = new Set(Array.from(r.getClientRects()).filter((x) => x.width > 0).map((x) => Math.round(x.top)));
      const header = document.querySelector(".file-header")!.getBoundingClientRect();
      const box = host.getBoundingClientRect();
      return {
        oneLine: ctx.measureText(full).width,
        room,
        cut,
        lines: tops.size,
        inHeader: box.top >= header.top - 0.5 && box.bottom <= header.bottom + 0.5,
      };
    }, full);
    expect(laid.oneLine, "setup: the words can't fit on one line beside the button").toBeGreaterThan(laid.room);
    expect(laid.cut, "no part of the phrase overflows its box").toBe(false);
    expect(c.phrase!.text.endsWith("…"), "the phrase is never cut short with …").toBe(false);
    expect(laid.lines, "the phrase wraps to a second line").toBeGreaterThanOrEqual(2);
    expect(laid.inHeader, "the header grows to hold both lines").toBe(true);
  });

  for (const width of [1280, 1440]) {
    test(`at ${width}px the button keeps its right edge across labels, never under 112px`, async ({ page, birta }) => {
      const doc = fixtureDoc("fresh");
      const { mock, state } = await openFixture(page, birta, "fresh", undefined, { viewport: { width, height: 900 } });
      const long = withRev(fixtureState("replied"), revOf(doc));
      Object.assign(long.phrase.replied, { done: 12, answered: 3, asked: 2, unasked: 4, not_handled: 5 });
      const seq = [
        ["Approve", withRev(fixtureState("fresh"), revOf(doc))],
        ["Hand back · 3", withRev(fixtureState("drafts"), revOf(doc))],
        ["Approve", withRev(fixtureState("replied"), revOf(doc))],
        ["With Claude", withRev(fixtureState("picked"), revOf(doc))],
        ["Copy hand-off line", withRev(fixtureState("sent"), revOf(doc))],
        ["Approved ✓", withRev(fixtureState("approved"), revOf(doc))],
        ["Approve", long],
      ] as [string, State][];
      const rights: number[] = [];
      for (const [label, next] of seq) {
        swap(mock, state, next);
        await expect.poll(async () => (await turnOf(page))?.text).toBe(label);
        const c = await chrome(page);
        const copy = await page.evaluate(() => document.getElementById("copy-md-btn")!.getBoundingClientRect().left);
        expect(c.turn!.box.width, label).toBeGreaterThanOrEqual(112);
        expect(c.turn!.box.right, label).toBeLessThanOrEqual(copy);
        expect(c.phrase!.box.right, label).toBeLessThanOrEqual(c.turn!.box.left);
        // The clean view leaves what waits for later to the tooltip, and a reply's tallies past three go there too
        if (label === "Approved ✓") expect(c.phrase!.title, label).toBe("approved 14:13 · 1 later · c to reopen");
        else expect(c.phrase!.title!.startsWith(c.phrase!.text), label).toBe(true);
        rights.push(c.turn!.box.right);
      }
      for (const r of rights) expect(Math.abs(r - rights[0])).toBeLessThanOrEqual(0.5);
    });
  }

  test("m hides the lane but leaves the phrase and the button", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.keyboard.press("m");
    await page.waitForTimeout(SETTLE_MS);
    const c = await chrome(page);
    expect(c.phrase?.text).toBe("Claude replied 14:13 · 5 to check");
    expect(c.turn?.text).toBe("Approve");
  });
});

// --- When the chrome shows (design call 4) ---------------------------------------------------

test.describe("when the chrome shows", () => {
  test("with the gate off there is no phrase and no button", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC });
    await page.goto(`${served.url}/view/pr.md`);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.locator("birta-phrase, birta-turn, birta-pop")).toHaveCount(0);
    await expect(page).toHaveTitle("pr.md — birta");
    expect(await page.evaluate(() => document.querySelector('link[rel~="icon"]')!.getAttribute("href"))).toBe("/favicon.png");
  });

  test("a doc of yours with no review file shows nothing until a review key, then stays for the tab", async ({
    page,
    birta,
  }) => {
    const served = await openReal(page, birta, STOPS_DOC);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.locator(".file-header birta-phrase")).toBeHidden();
    await expect(page.locator(".file-header birta-turn")).toBeHidden();
    await page.keyboard.press("j");
    await expect.poll(() => phraseOf(page)).toBe("your turn");
    expect(await turnOf(page)).toEqual({ text: "Approve", kbd: await hint(page), disabled: false });
    await page.reload();
    await booted(page);
    await expect.poll(() => phraseOf(page)).toBe("your turn");
    // Another tab hasn't started a review
    const other = await secondPage(page, `${served.url}/view/pr.md`);
    await other.waitForTimeout(SETTLE_MS);
    await expect(other.locator(".file-header birta-phrase")).toBeHidden();
    expect(reviewLog(served.root)).toBeNull();
  });

  test("an agent's draft shows its phrase and Approve on load, and viewing writes nothing", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, {}, ["pr.md", "--drafted-by", "claude"]);
    await expect.poll(() => phraseOf(page)).toBe("drafted by Claude · your turn");
    expect(await turnOf(page)).toEqual({ text: "Approve", kbd: await hint(page), disabled: false });
    expect(reviewLog(served.root)).toBeNull();
  });

  test("a review file of yours shows the chrome on load", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh", (s) => (s.owner.drafted_by = null));
    await expect.poll(() => phraseOf(page)).toBe("your turn");
  });
});

// --- What the turn button and ⌘⏎ send ------------------------------------------------------------

test.describe("turn", () => {
  // A ⌘⏎ that beats the note's state must hand the note back, never approve past it
  test("⌘⏎ right after ⏎ hands back the new note once it is acked, and never approves", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "fresh", undefined, { acks: "manual" });
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve");
    await page.keyboard.press("c");
    await page.keyboard.type("say why");
    const pressed = Date.now();
    await page.keyboard.press("Enter");
    await page.keyboard.press("ControlOrMeta+Enter");
    expect(Date.now() - pressed).toBeLessThan(50);
    // Counted from the page's own note before any state or ack
    await expect.poll(async () => (await turnOf(page))?.text, { timeout: 400 }).toBe("Hand back · 1");
    await page.waitForTimeout(200);
    expect(mock.ops().map((o) => o.op)).toEqual(["comment"]);
    mock.ack(mock.ops()[0].cid);
    await expect.poll(() => mock.ops().map((o) => o.op)).toEqual(["comment", "hand_back"]);
    // The state that shows the note comes 500 ms after the keys
    const shown = withRev(fixtureState("drafts"), state.rev);
    shown.log = { ...shown.log, lines: 500 };
    await page.waitForTimeout(Math.max(0, 500 - (Date.now() - pressed)));
    mock.pushState(shown);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().map((o) => o.op)).toEqual(["comment", "hand_back"]);
    expect(mock.ops().some((o) => o.op === "approve")).toBe(false);
  });

  test("a note refused by the server leaves a quick ⌘⏎ with nothing to approve", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "fresh", undefined, { acks: "manual" });
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve");
    await page.keyboard.press("c");
    await page.keyboard.type("say why");
    await page.keyboard.press("Enter");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().map((o) => o.op)).toEqual(["comment"]);
    mock.nack(mock.ops()[0].cid, "changed_since");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().map((o) => o.op)).toEqual(["comment"]);
  });

  test("⌘⏎, Ctrl+Enter and a click each hand back the drafts", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    await page.keyboard.press("Meta+Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.keyboard.press("Control+Enter");
    await expect.poll(() => mock.ops().length).toBe(2);
    await page.locator(".file-header birta-turn button").click();
    await expect.poll(() => mock.ops().length).toBe(3);
    for (const op of mock.ops()) {
      expect(Object.keys(op).sort()).toEqual(["cid", "op", "path", "rev", "scope", "type"]);
      expect(op).toMatchObject({ type: "op", op: "hand_back", scope: "doc", path: "pr.md", rev: revOf(fixtureDoc("drafts")) });
    }
  });

  test("⌘⏎ approves when nothing is pending, and a refusal says why in the pill", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "fresh", undefined, { acks: "manual" });
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    const op = mock.ops()[0];
    expect(Object.keys(op).sort()).toEqual(["cid", "op", "path", "rev", "type"]);
    expect(op.op).toBe("approve");
    mock.nack(op.cid, "changed_since");
    await expect.poll(() => stripMessage(page)).toBe("changed since · try again");
  });

  for (const [reason, text] of [
    ["nothing_to_send", "nothing to hand back"],
    ["hold_pending", "already handing back"],
    // Any other reason is its id in words
    ["bad_anchor", "bad anchor"],
  ]) {
    test(`a hand-back refused with ${reason} says "${text}"`, async ({ page, birta }) => {
      const { mock } = await openFixture(page, birta, "drafts", undefined, { acks: "manual" });
      await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
      await page.keyboard.press("ControlOrMeta+Enter");
      await expect.poll(() => mock.ops().length).toBe(1);
      mock.nack(mock.ops()[0].cid, reason);
      await expect.poll(() => stripMessage(page)).toBe(text);
    });
  }

  test("a withdraw refused because the agent picked it up names that agent", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "sent", undefined, { acks: "manual" });
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: WITHDRAW }).click();
    await expect.poll(() => mock.ops().length).toBe(1);
    const picked = withRev(fixtureState("picked"), state.rev);
    picked.phrase.picked_up.by = "codex";
    picked.rounds[0].claimed_by = "codex";
    swap(mock, state, picked);
    await expect.poll(() => phraseOf(page)).toBe("Codex picked up 14:13");
    mock.nack(mock.ops()[0].cid, "claimed");
    await expect.poll(() => stripMessage(page)).toBe("Codex already picked it up");
  });

  test("the button takes every click while states keep arriving", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "drafts");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    let tick = 0;
    // The count changes on every push, so the label is rewritten under the pointer
    const stream = setInterval(() => {
      tick++;
      state.server_now += 50;
      state.counts.drafts = 3 + (tick % 2);
      mock.pushState(state);
    }, 50);
    try {
      for (let i = 0; i < 20; i++) {
        await page.locator(".file-header birta-turn button").click();
        await expect.poll(() => mock.ops().length, { timeout: 1_000, message: `click ${i + 1}` }).toBe(i + 1);
      }
    } finally {
      clearInterval(stream);
    }
  });

  for (const name of ["sent", "approved"]) {
    test(`with ${name}.json neither ⌘⏎ nor a click sends anything`, async ({ page, birta }) => {
      // Within its first 10 s a hand-back's button is With Claude, disabled
      const { mock } = await openFixture(page, birta, name, (s) => {
        if (name === "sent") s.server_now = s.phrase.sent.ts + 2_000;
      });
      await expect.poll(async () => (await turnOf(page))?.disabled).toBe(true);
      await page.keyboard.press("ControlOrMeta+Enter");
      await page.locator(".file-header birta-turn button").click({ force: true });
      await page.waitForTimeout(SETTLE_MS);
      expect(mock.ops()).toEqual([]);
    });
  }
});

// --- The hold ----------------------------------------------------------------------------------------

test.describe("hold", () => {
  test("inside a hand-back's hold the phrase and button keep their words and the pill speaks", async ({ page, birta }) => {
    // The fixture's server clock is days behind this machine's, so only the server's clock can hold this
    await openFixture(page, birta, "hold", (s) => (s.hold.not_before = s.server_now + 20_000));
    await expect.poll(() => stripMessage(page)).toBe("handing back · 3 notes · z to undo");
    const c = await chrome(page);
    expect(c.phrase?.text).toBe("drafted by Claude · your turn");
    expect(c.turn).toMatchObject({ text: "Hand back · 3", kbd: await hint(page), disabled: false });
    expect(await glyphRoles(page)).toEqual(["you", "you"]);
    await page.waitForTimeout(1_000);
    expect(await stripMessage(page)).toBe("handing back · 3 notes · z to undo");
  });

  test("when the hold passes, with no push, the hand-back has left", async ({ page, birta }) => {
    const { state } = await openFixture(page, birta, "hold", (s) => (s.hold.not_before = s.server_now + 1_500));
    await expect.poll(() => stripMessage(page)).toBe("handing back · 3 notes · z to undo");
    expect(await glyphRoles(page)).toEqual(["you", "you"]);
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toBe(`sent ${hhmm(state.phrase.sent.ts)} · not picked up yet`);
    const c = await chrome(page);
    expect(c.message).toBeNull();
    expect(c.turn).toMatchObject({ text: "With Claude", kbd: null, disabled: true });
    await expect.poll(() => glyphRoles(page)).toEqual(["agent", "agent"]);
  });

  test("inside an approve's hold the phrase and button keep their words and the pill says approving", async ({
    page,
    birta,
  }) => {
    await openFixture(page, birta, "approved", (s) => (s.hold.not_before = s.server_now + 20_000));
    await expect.poll(() => stripMessage(page)).toBe("approving · z to undo");
    const c = await chrome(page);
    expect(c.phrase?.text).toBe("Claude replied 14:13 · all checked");
    expect(c.turn).toMatchObject({ text: "Approve", kbd: await hint(page), disabled: false });
  });

  test("with the socket down inside the hold the pill says the hand-back stands", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "hold", (s) => (s.hold.not_before = s.server_now + 30_000));
    await expect.poll(() => stripMessage(page)).toBe("handing back · 3 notes · z to undo");
    // Later routes win: every reconnect is refused from here on
    await page.routeWebSocket(/\/ws$/, (ws) => ws.close());
    await mock.drop();
    await expect.poll(() => stripMessage(page), { timeout: 3_000 }).toBe("connection lost · hand-back stands");
  });

  test("on a real server a second tab shows the same hold, both flip, and its menu withdraws the hand-back", async ({
    page,
    birta,
  }) => {
    const served = await openReal(page, birta, STOPS_DOC, { BIRTA_REVIEW_HOLD_MS: "6000" }, ["pr.md", "--drafted-by", "claude"]);
    await page.keyboard.press("c");
    await page.keyboard.type("first note");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 1");
    expect(await phraseOf(page)).toBe("drafted by Claude · your turn");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(5);
    const submit = reviewLog(served.root)![4];
    expect(submit.op).toBe("submit");

    const other = await secondPage(page, `${served.url}/view/pr.md`);
    const during = {
      phrase: "drafted by Claude · your turn",
      turn: { text: "Hand back · 1", kbd: await hint(page), disabled: false },
      pill: "handing back · 1 note · z to undo",
    };
    for (const p of [page, other]) {
      await expect.poll(() => stripMessage(p)).toBe(during.pill);
      expect(await phraseOf(p)).toBe(during.phrase);
      expect(await turnOf(p)).toEqual(during.turn);
    }

    // A hand-back is sent when its hold ends, which can fall in the next minute
    const after = `sent ${hhmm(submit.not_before)} · not picked up yet`;
    for (const p of [page, other]) {
      await expect.poll(() => phraseOf(p), { timeout: 8_000 }).toBe(after);
      expect(await stripMessage(p)).toBeNull();
      expect(await turnOf(p)).toEqual({ text: "With Claude", kbd: null, disabled: true });
    }

    await other.locator(".file-header birta-phrase").click();
    await expect.poll(() => menuOf(other)).toContain(WITHDRAW);
    await other.locator('birta-pop [role="menuitem"]', { hasText: WITHDRAW }).click();
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(6);
    expect(reviewLog(served.root)![5]).toMatchObject({ op: "withdraw", to: 5 });
    for (const p of [page, other]) {
      await expect.poll(() => phraseOf(p)).toBe("drafted by Claude · your turn");
      await expect.poll(async () => (await turnOf(p))?.text).toBe("Hand back · 1");
    }
  });

  test("on a real server an approve holds, then names its time", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, { BIRTA_REVIEW_HOLD_MS: "3000" }, ["pr.md", "--drafted-by", "claude"]);
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => stripMessage(page)).toBe("approving · z to undo");
    const log = reviewLog(served.root)!;
    const approve = log[log.length - 1];
    expect(approve).toMatchObject({ op: "approve", rev: revOf(STOPS_DOC) });
    await expect.poll(() => phraseOf(page), { timeout: 6_000 }).toBe(`approved ${hhmm(approve.not_before)} · c to reopen`);
    expect(await stripMessage(page)).toBeNull();
    expect(await turnOf(page)).toEqual({ text: "Approved ✓", kbd: null, disabled: true });
  });
});

// --- The phrase menu ----------------------------------------------------------------------------------

/** Records what the page copies; call before `goto`. */
async function captureClipboard(page: Page): Promise<() => Promise<string[]>> {
  await page.addInitScript(() => {
    const w = window as unknown as { __copied: string[] };
    w.__copied = [];
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: (text: string) => (w.__copied.push(text), Promise.resolve()) },
    });
  });
  return () => page.evaluate(() => (window as unknown as { __copied: string[] }).__copied);
}

test.describe("menu", () => {
  test("a click on the phrase opens the menu in the top layer", async ({ page, birta }) => {
    await openFixture(page, birta, "sent");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    expect(await menuOf(page)).toBeNull();
    await page.locator(".file-header birta-phrase").click();
    await expect.poll(() => menuOf(page)).toEqual([COPY, MINE, KEYS, WITHDRAW]);
  });

  test("the phrase opens its menu on every click while states keep arriving", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "sent");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    const ts = state.phrase.sent.ts;
    let tick = 0;
    // Every push moves the server clock and changes the phrase's words, so each one re-renders
    const stream = setInterval(() => {
      tick++;
      state.server_now += 50;
      state.phrase.sent.ts = ts + (tick % 2) * MIN;
      mock.pushState(state);
    }, 50);
    try {
      for (let i = 0; i < 20; i++) {
        await page.locator(".file-header birta-phrase").click();
        await expect.poll(() => menuOf(page), { timeout: 1_000, message: `click ${i + 1}` }).not.toBeNull();
        await page.keyboard.press("Escape");
        await expect.poll(() => menuOf(page), { timeout: 1_000 }).toBeNull();
      }
    } finally {
      clearInterval(stream);
    }
  });

  test("a menu row takes every click while states keep arriving", async ({ page, birta }) => {
    const copied = await captureClipboard(page);
    const { mock, state } = await openFixture(page, birta, "sent");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    const stream = setInterval(() => {
      state.server_now += 50;
      mock.pushState(state);
    }, 50);
    try {
      for (let i = 0; i < 20; i++) {
        await openMenu(page);
        await page.locator('birta-pop [role="menuitem"]', { hasText: COPY }).click();
        await expect.poll(copied, { timeout: 1_000, message: `click ${i + 1}` }).toHaveLength(i + 1);
        await expect.poll(() => menuOf(page), { timeout: 1_000 }).toBeNull();
      }
    } finally {
      clearInterval(stream);
    }
  });

  test("withdraw takes back the hand-back it names, and closes the menu", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "sent");
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: WITHDRAW }).click();
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "withdraw", to: state.phrase.sent.n });
    await expect.poll(() => menuOf(page)).toBeNull();
  });

  test("an approve nobody has claimed can be withdrawn", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "approved");
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: WITHDRAW }).click();
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "withdraw", to: state.approve.n });
  });

  test("copy hand-off line copies the state's line", async ({ page, birta }) => {
    const copied = await captureClipboard(page);
    const { state } = await openFixture(page, birta, "sent");
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: COPY }).click();
    await expect.poll(copied).toEqual([state.hand_off]);
  });

  for (const where of ["in the menu", "on the page"]) {
    test(`with focus ${where}, j, k and ⏎ pick rows and leave the doc's focus alone`, async ({ page, birta }) => {
      const copied = await captureClipboard(page);
      const { mock, state } = await openFixture(page, birta, "sent");
      const openMenu = async () => {
        await page.locator(".file-header birta-phrase").click();
        await expect.poll(() => menuOf(page)).not.toBeNull();
        if (where === "on the page") await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
      };
      await openMenu();
      // The first row, the safest, is current on open
      await page.keyboard.press("Enter");
      await expect.poll(copied).toEqual([state.hand_off]);
      expect(await focused(page)).toBe("doc");
      expect(mock.ops()).toEqual([]);
      await expect.poll(() => menuOf(page)).toBeNull();
      await openMenu();
      // withdraw is last: j stops there, and k and j step back and forth
      for (const key of ["j", "j", "j", "j", "k", "j"]) await page.keyboard.press(key);
      await page.keyboard.press("Enter");
      await expect.poll(() => mock.ops().length).toBe(1);
      expect(mock.ops()[0]).toMatchObject({ op: "withdraw", to: state.phrase.sent.n });
      expect(await focused(page)).toBe("doc");
    });
  }

  test("after 15 minutes of silence, with no push, the phrase says so and the menu's release lets go", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "picked", (s) => {
      s.server_now = s.phrase.picked_up.silence.since_ts + 15 * MIN - 1_500;
    });
    await expect.poll(() => phraseOf(page)).toBe("Claude picked up 14:13");
    await openMenu(page);
    expect(await menuOf(page)).not.toContain("release");
    await page.keyboard.press("Escape");
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toBe("Claude picked up 14:13 · no reply in 15 min");
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: "release" }).click();
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "release", claim: state.phrase.picked_up.n });
  });

  test("on a real server the owner row marks an agent's draft as yours", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, {}, ["pr.md", "--drafted-by", "claude"]);
    await expect.poll(() => phraseOf(page)).toBe("drafted by Claude · your turn");
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: MINE }).click();
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    const log = reviewLog(served.root)!;
    expect(log[2]).toMatchObject({ op: "owner", drafted_by: "claude" });
    expect(log[3]).toMatchObject({ op: "owner", drafted_by: null });
    await expect.poll(() => phraseOf(page)).toBe("your turn");
  });

  test("Esc drops one layer per press: theme menu, phrase menu, then composer; focus stays", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: withRev(fixtureState("fresh"), revOf(STOPS_DOC)) });
    const all = await stops(page);
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(all[1].sp);
    await page.keyboard.press("c");
    const composer = page.locator("textarea");
    await expect(composer).toBeFocused();
    await page.locator(".file-header birta-phrase").click();
    await expect.poll(() => menuOf(page)).not.toBeNull();
    await page.evaluate(() => document.getElementById("theme-dropdown-btn")!.click());
    const dropdown = page.locator(".theme-dropdown");
    const layers = async () => ({
      theme: /\bopen\b/.test((await dropdown.getAttribute("class")) ?? ""),
      menu: (await menuOf(page)) !== null,
      composer: await composer.count(),
      focus: await focused(page),
    });
    await expect.poll(layers).toEqual({ theme: true, menu: true, composer: 1, focus: all[1].sp });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ theme: false, menu: true, composer: 1, focus: all[1].sp });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ theme: false, menu: false, composer: 1, focus: all[1].sp });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ theme: false, menu: false, composer: 0, focus: all[1].sp });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(await layers()).toEqual({ theme: false, menu: false, composer: 0, focus: all[1].sp });
  });

  test("with focus on the page, Esc closes the menu before the composer, and the doc's focus stays", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { state: withRev(fixtureState("fresh"), revOf(STOPS_DOC)) });
    const all = await stops(page);
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await page.keyboard.press("c");
    const composer = page.locator("textarea");
    await expect(composer).toBeFocused();
    await page.locator(".file-header birta-phrase").click();
    await expect.poll(() => menuOf(page)).not.toBeNull();
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    const layers = async () => ({
      menu: (await menuOf(page)) !== null,
      composer: await composer.count(),
      focus: await focused(page),
    });
    expect(await layers()).toEqual({ menu: true, composer: 1, focus: all[1].sp });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ menu: false, composer: 1, focus: all[1].sp });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ menu: false, composer: 0, focus: all[1].sp });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(await layers()).toEqual({ menu: false, composer: 0, focus: all[1].sp });
  });

  for (const [name, edit] of [
    ["sent", undefined],
    ["approved", undefined],
    ["hold", (s: State) => (s.hold.not_before = s.server_now + 20_000)],
  ] as [string, ((s: State) => void) | undefined][]) {
    test(`no line of the chrome leaves a nameless human's name blank (${name}.json)`, async ({ page, birta }) => {
      const { state } = await openFixture(page, birta, name, edit);
      // The fold's own output for a nameless human
      expect(state.rounds.length).toBeGreaterThan(0);
      expect(state.rounds.every((r: State) => r.by === "")).toBe(true);
      await expect.poll(() => phraseOf(page)).not.toBeNull();
      await page.locator(".file-header birta-phrase").click();
      await expect.poll(() => menuOf(page)).not.toBeNull();
      const c = await chrome(page);
      const texts = [c.phrase!.text, c.phrase!.title ?? "", c.turn!.text, c.message ?? "", ...c.menu!];
      for (const t of texts) {
        expect(t).not.toMatch(/\bby\s*(·|$)/);
        expect(t).not.toMatch(/undefined|null|NaN/);
      }
    });
  }
});

// --- Messages ----------------------------------------------------------------------------------------------

test.describe("messages", () => {
  test("the selection hint shows on the strip's keys side, not as a message", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    await page.evaluate(() => {
      const p = Array.from(document.querySelectorAll("#content p")).find((e) => e.textContent!.includes("When the upload"))!;
      const t = p.firstChild as Text;
      const r = document.createRange();
      r.setStart(t, 9);
      r.setEnd(t, 15);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    });
    await expect.poll(() => stripKeys(page)).toBe("c note · q ask · x cut these words");
    expect(await stripMessage(page)).toBeNull();
    await page.keyboard.press("Escape");
    await expect.poll(() => stripKeys(page)).not.toBe("c note · q ask · x cut these words");
    expect(await stripMessage(page)).toBeNull();
  });

  test("note saved is a message in the strip", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC);
    // A verdict pills its scope instead, so the first note is what creates the file here
    await page.keyboard.press("c");
    await page.keyboard.type("first");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    await expect.poll(() => stripMessage(page)).toBe("note saved · z to undo");
  });
});

// --- The tab -------------------------------------------------------------------------------------------------

test.describe("tab", () => {
  const icon = (page: Page) => page.evaluate(() => document.querySelector('link[rel~="icon"]')!.getAttribute("href"));

  test("news puts ● in the title and a dot on the favicon, and both go when it does", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen");
    // The tab reads what is left to check
    expect(state.counts.to_check).toBeGreaterThan(0);
    await expect(page).toHaveTitle("● pr.md — birta");
    await expect.poll(() => icon(page)).toMatch(/^data:image\/png;base64,/);
    const quiet = withRev(fixtureState("fresh"), state.rev);
    expect(quiet.counts.to_check).toBe(0);
    swap(mock, state, quiet);
    await expect(page).toHaveTitle("pr.md — birta");
    await expect.poll(() => icon(page)).toBe("/favicon.png");
  });

  test("with no news the title and the favicon stay as they were", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.waitForTimeout(SETTLE_MS);
    await expect(page).toHaveTitle("pr.md — birta");
    expect(await icon(page)).toBe("/favicon.png");
  });
});

// --- What the folder and the file allow, on a real server ------------------------------------------------------

test.describe("writability", () => {
  test("a doc birta can't write is read-only, notes only, and keeps its button", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON, ["pr.md", "--drafted-by", "claude"]);
    fs.chmodSync(path.join(served.root, "pr.md"), 0o444);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => phraseOf(page)).toBe("read-only · notes only");
    // Notes still work, so the turn does too
    expect(await turnOf(page)).toEqual({ text: "Approve", kbd: await hint(page), disabled: false });
  });

  test("a folder birta can't write has no review and no button", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON, ["pr.md", "--drafted-by", "claude"]);
    fs.chmodSync(served.root, 0o555);
    try {
      await page.setViewportSize(WIDE);
      await page.goto(`${served.url}/view/pr.md`);
      await booted(page);
      await expect.poll(() => phraseOf(page)).toBe("review unavailable · folder not writable");
      await expect(page.locator(".file-header birta-turn")).toBeHidden();
    } finally {
      // The fixture removes the folder when the test ends
      fs.chmodSync(served.root, 0o755);
    }
  });
});

// --- Adversary killers ----------------------------------------------------------------------------

const DAY = 86_400_000;

test.describe("killers: clock", () => {
  test("with the server's clock a day ahead, the hold still ends on time with no push", async ({ page, birta }) => {
    const { state } = await openFixture(page, birta, "hold", (s) => {
      s.server_now = Date.now() + DAY;
      s.hold.not_before = s.server_now + 1_500;
    });
    await expect.poll(() => stripMessage(page)).toBe("handing back · 3 notes · z to undo");
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toBe(`sent ${hhmm(state.phrase.sent.ts)} · not picked up yet`);
    expect(await stripMessage(page)).toBeNull();
  });

  test("a hold that arrives in a later push ends on time with no further push", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "drafts");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    const held = withRev(fixtureState("hold"), state.rev);
    // One server clock: the page's never runs backwards
    held.server_now = state.server_now;
    held.hold.not_before = held.server_now + 1_500;
    swap(mock, state, held);
    await expect.poll(() => stripMessage(page)).toBe("handing back · 3 notes · z to undo");
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toBe(`sent ${hhmm(held.phrase.sent.ts)} · not picked up yet`);
  });

  test("the silence keeps counting minutes with no push", async ({ page, birta }) => {
    await openFixture(page, birta, "picked", (s) => (s.server_now = s.phrase.picked_up.silence.since_ts + 15 * MIN + 58_000));
    await expect.poll(() => phraseOf(page)).toBe("Claude picked up 14:13 · no reply in 15 min");
    await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toBe("Claude picked up 14:13 · no reply in 16 min");
  });

  test.describe("in a zone off UTC", () => {
    test.use({ timezoneId: "Asia/Kathmandu" });
    test("times are the reader's local time", async ({ page, birta }) => {
      const { state } = await openFixture(page, birta, "sent");
      const local = new Date(state.phrase.sent.ts).toLocaleTimeString("en-GB", {
        timeZone: "Asia/Kathmandu",
        hour: "2-digit",
        minute: "2-digit",
        hour12: false,
      });
      expect(local).toBe("19:58");
      await expect.poll(() => phraseOf(page)).toBe(`sent ${local} · not picked up yet`);
    });
  });
});

test.describe("killers: menu and turn", () => {
  test("a claim that lands while the menu is open takes withdraw away", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "sent");
    await openMenu(page);
    expect(await menuOf(page)).toContain(WITHDRAW);
    swap(mock, state, withRev(fixtureState("picked"), state.rev));
    await expect.poll(() => menuOf(page)).toEqual([COPY, MINE, KEYS]);
  });

  test("Approve with something to check sends no approve at first; a press past the guard's dwell does", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "replied");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve");
    expect(await phraseOf(page)).toBe("Claude replied 14:13 · 3 to check");
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.locator(".file-header birta-turn button").click();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().filter((o) => o.op === "approve")).toEqual([]);
    await page.waitForTimeout(1_000);
    await page.locator(".file-header birta-turn button").click();
    await expect.poll(() => mock.ops().filter((o) => o.op === "approve").length).toBe(1);
  });

  test("a folder that can't be written takes no ⌘⏎", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", (s) => (s.log_writable = false));
    await expect.poll(() => phraseOf(page)).toBe("review unavailable · folder not writable");
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  for (const [platform, want] of [
    ["Win32", "Ctrl⏎"],
    ["Linux x86_64", "Ctrl⏎"],
    ["MacIntel", "⌘⏎"],
  ]) {
    test(`on ${platform} the key hint is ${want}`, async ({ page, birta }) => {
      await page.addInitScript((p) => Object.defineProperty(Navigator.prototype, "platform", { get: () => p }), platform);
      await openFixture(page, birta, "drafts");
      await expect.poll(async () => (await turnOf(page))?.kbd).toBe(want);
    });
  }
});

test.describe("killers: pill and socket", () => {
  test("the hold pill replaces note-saved, which doesn't come back after", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, { BIRTA_REVIEW_HOLD_MS: "1500" });
    await page.keyboard.press("c");
    await page.keyboard.type("first");
    await page.keyboard.press("Enter");
    await expect.poll(() => stripMessage(page)).toBe("note saved · z to undo");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => stripMessage(page)).toBe("handing back · 1 note · z to undo");
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toMatch(/^sent \d\d:\d\d · not picked up yet$/);
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
    expect(reviewLog(served.root)!.some((l) => l.op === "submit")).toBe(true);
  });

  test("with the socket down outside a hold nothing claims a hand-back stands", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.routeWebSocket(/\/ws$/, (ws) => ws.close());
    await mock.drop();
    await page.waitForTimeout(2_000);
    expect(await stripMessage(page)).toBeNull();
  });

  test("when the socket comes back inside the hold the hold pill returns", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "hold", (s) => (s.hold.not_before = s.server_now + 30_000));
    await expect.poll(() => stripMessage(page)).toBe("handing back · 3 notes · z to undo");
    await mock.drop(); // no refusal route: the page reconnects after ~1 s
    await expect.poll(() => stripMessage(page)).toBe("connection lost · hand-back stands");
    await expect.poll(() => stripMessage(page), { timeout: 6_000 }).toBe("handing back · 3 notes · z to undo");
  });
});

test.describe("killers: names and tab", () => {
  test("a drafter's name with markup is text, never elements", async ({ page, birta }) => {
    const evil = '<img src=x onerror="window.__pwned=1"><b>bold</b>';
    await openReal(page, birta, STOPS_DOC, {}, ["pr.md", "--drafted-by", evil]);
    await expect.poll(() => phraseOf(page)).toBe(`drafted by ${evil} · your turn`);
    await openMenu(page);
    expect(await menuOf(page)).toContain(`drafted by ${evil} · mark as yours`);
    const injected = await page.evaluate(() =>
      ["birta-phrase", "birta-turn", "birta-pop"].some((t) =>
        Array.from(document.querySelectorAll(t)).some((h) => (h.shadowRoot ?? h).querySelector("img, b")),
      ),
    );
    expect(injected).toBe(false);
    expect(await page.evaluate(() => (window as any).__pwned)).toBeUndefined();
  });

  test("a long agent name keeps the button's right edge, and the phrase shows the whole name", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "picked");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("With Claude");
    const before = { c: await chrome(page) };
    const long = JSON.parse(JSON.stringify(state));
    long.phrase.picked_up.by = long.rounds[0].claimed_by = "agent-" + "x".repeat(54);
    swap(mock, state, long);
    await expect.poll(async () => (await turnOf(page))?.text).toMatch(/^With Agent-x/);
    const c = await chrome(page);
    expect(c.turn!.text).toBe("With Agent-" + "x".repeat(18) + "…");
    expect(c.turn!.title).toBe("With Agent-" + "x".repeat(54));
    const copy = await page.evaluate(() => document.getElementById("copy-md-btn")!.getBoundingClientRect().left);
    expect(Math.abs(c.turn!.box.right - before.c.turn!.box.right)).toBeLessThanOrEqual(0.5);
    expect(c.turn!.box.right).toBeLessThanOrEqual(copy);
    expect(c.phrase!.box.right).toBeLessThanOrEqual(c.turn!.box.left);
    // The phrase is never cut short: it wraps instead
    expect(c.phrase!.text).toBe("Agent-" + "x".repeat(54) + " picked up 14:13");
  });

  test("the phrase's title follows every push", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "fresh");
    for (const [name, title] of [
      ["drafts", "drafted by Claude · your turn"],
      ["sent", "sent 14:13 · not picked up yet"],
      ["approved", "approved 14:13 · 1 later · c to reopen"],
    ]) {
      swap(mock, state, withRev(fixtureState(name), state.rev));
      await expect.poll(async () => (await chrome(page)).phrase?.title).toBe(title);
    }
    // The clean view's phrase leaves what waits for later to the title
    swap(mock, state, withRev(fixtureState("approved"), state.rev));
    await expect.poll(async () => (await chrome(page)).phrase?.title).toBe("approved 14:13 · 1 later · c to reopen");
  });

  test("not-handled rows alone are news", async ({ page, birta }) => {
    // Claude's reply with everything checked but its not-handled row: one stop to check
    const { state } = await openFixture(page, birta, "replied", (s) => {
      for (const n of s.news) if (n.id !== "nh:5") n.check = n.kind === "change" ? "kept" : "ok";
      s.changes.forEach((c: State) => Object.assign(c, { seen: true, check: "kept" }));
      s.counts = { ...s.counts, unseen: 0, attention: 1, to_check: 1 };
    });
    expect(state.counts).toMatchObject({ unseen: 0, attention: 1 });
    await expect(page).toHaveTitle("● pr.md — birta");
  });

  test("an approve as is settles all news: the fold counts nothing, and the tab has no ●", async ({ page, birta }) => {
    const { state } = await openFixture(page, birta, "approved");
    expect(state.counts).toMatchObject({ unseen: 0, attention: 0, to_check: 0 });
    expect(state.news).toEqual([]);
    await expect.poll(async () => (await chrome(page)).phrase).not.toBeNull();
    await page.waitForTimeout(SETTLE_MS);
    await expect(page).toHaveTitle("pr.md — birta");
  });

  test("news that ends before the dotted icon is drawn leaves the plain icon", async ({ page, birta }) => {
    const icon = () => page.evaluate(() => document.querySelector('link[rel~="icon"]')!.getAttribute("href"));
    // The icon loads only after the news has gone
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/favicon.png", async (route) => {
      await held;
      await route.fallback();
    });
    const { mock, state } = await openFixture(page, birta, "unseen");
    await expect(page).toHaveTitle("● pr.md — birta");
    swap(mock, state, withRev(fixtureState("fresh"), state.rev));
    await expect(page).toHaveTitle("pr.md — birta");
    release();
    await page.waitForTimeout(1_000);
    await expect(page).toHaveTitle("pr.md — birta");
    expect(await icon()).toBe("/favicon.png");
  });

  test("repeated news never piles up the prefix", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen");
    for (let i = 0; i < 3; i++) mock.pushState(state);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page).toHaveTitle("● pr.md — birta");
  });
});

// --- Rulings after the adversary pass ------------------------------------------------------------

test.describe("rulings", () => {
  test("⌘⏎ does nothing before review starts in the tab", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    const sent: State[] = [];
    page.on("websocket", (ws) => ws.on("framesent", (f) => sent.push(JSON.parse(String(f.payload)))));
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(sent.filter((f) => f.type === "op")).toEqual([]);
    expect(reviewLog(served.root)).toBeNull();
    // Once review is on in the tab, the same key approves
    await page.keyboard.press("j");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => sent.filter((f) => f.type === "op" && f.op === "approve").length).toBe(1);
  });

  test("a refusal's pill goes after 4 seconds", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", undefined, { acks: "manual" });
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "nothing_to_send");
    await expect.poll(() => stripMessage(page)).toBe("nothing to hand back");
    await page.waitForTimeout(2_500);
    expect(await stripMessage(page)).toBe("nothing to hand back");
    await expect.poll(() => stripMessage(page), { timeout: 3_000 }).toBeNull();
  });

  test("a refusal's pill goes when the next op is kept", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", undefined, { acks: "manual" });
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "nothing_to_send");
    await expect.poll(() => stripMessage(page)).toBe("nothing to hand back");
    // A block, as a whole-doc y only says what to do
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(2);
    mock.ack(mock.ops()[1].cid);
    // The verdict's own pill speaks over the refusal
    await expect.poll(() => stripMessage(page), { timeout: 1_500 }).toBe("yes · ## Why (section) · z to undo");
  });

  test("a new hold replaces a refusal's pill", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "drafts", undefined, { acks: "manual" });
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "hold_pending");
    await expect.poll(() => stripMessage(page)).toBe("already handing back");
    const held = withRev(fixtureState("hold"), state.rev);
    held.server_now = state.server_now;
    held.hold.not_before = held.server_now + 20_000;
    swap(mock, state, held);
    await expect.poll(() => stripMessage(page)).toBe("handing back · 3 notes · z to undo");
  });

  test("a claimed refusal that comes before its state names the agent once the state does", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "sent", (s) => (s.owner.drafted_by = null), { acks: "manual" });
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: WITHDRAW }).click();
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "claimed");
    await expect.poll(() => stripMessage(page)).toBe("the agent already picked it up");
    const picked = withRev(fixtureState("picked"), state.rev);
    picked.owner.drafted_by = null;
    picked.phrase.picked_up.by = picked.rounds[0].claimed_by = "codex";
    swap(mock, state, picked);
    await expect.poll(() => stripMessage(page)).toBe("Codex already picked it up");
  });

  for (const where of ["in the menu", "on the page"]) {
    test(`with focus ${where}, a review key closes the menu and then acts`, async ({ page, birta }) => {
      const { mock } = await openFixture(page, birta, "fresh");
      await openMenu(page);
      if (where === "on the page") await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
      await page.keyboard.press("c");
      await expect(page.locator("textarea")).toBeFocused();
      expect(await menuOf(page)).toBeNull();
      await page.keyboard.press("Escape");
      await expect(page.locator("textarea")).toHaveCount(0);
      await openMenu(page);
      if (where === "on the page") await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
      // On the doc x only says what to do, which is still the key acting
      await page.keyboard.press("x");
      await expect.poll(() => stripMessage(page)).toBe("whole doc: c to say what's wrong");
      expect(mock.ops()).toEqual([]);
      expect(await menuOf(page)).toBeNull();
    });
  }

  test("a hold that has flipped stays flipped when an older server time comes again", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "hold", (s) => (s.hold.not_before = s.server_now + 1_500));
    const after = `sent ${hhmm(state.phrase.sent.ts)} · not picked up yet`;
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toBe(after);
    // The same snapshot again, with the server time from before the flip
    mock.pushState(state);
    await page.waitForTimeout(SETTLE_MS);
    expect(await phraseOf(page)).toBe(after);
    expect(await stripMessage(page)).toBeNull();
  });
});

// --- The hand-off line on the button, in both modes ------------------------------------------------

/** The serving modes a hand-back can sit unclaimed in: one file or a folder, Claude's draft or yours. */
const HAND_OFF_MODES: [string, boolean, string | null][] = [
  ["one file, Claude's draft", false, "claude"],
  ["one file, your doc", false, null],
  ["a folder, Claude's draft", true, "claude"],
  ["a folder, your doc", true, null],
];

test.describe("the hand-off line", () => {
  for (const [mode, folder, drafter] of HAND_OFF_MODES) {
    test(`${mode}: 10 s unclaimed turns the button into Copy hand-off line, with no push; ⌘⏎ and a click copy it`, async ({
      page,
      birta,
    }) => {
      const copied = await captureClipboard(page);
      const doc = fixtureDoc("sent");
      const state = withRev(fixtureState("sent"), revOf(doc));
      state.owner.drafted_by = drafter;
      // Six seconds after the hand-back: still inside the 10 s the agent has to pick it up
      state.server_now = state.phrase.sent.ts + 6_000;
      const files: Record<string, string> = { "pr.md": doc };
      let dir: State | undefined;
      if (folder) {
        files["checklist.md"] = "# Checklist\n";
        dir = {
          type: "dir_state",
          root: "",
          truncated: false,
          folder: null,
          files: [{ paths: ["pr.md"], summary: { stage: "sent", phrase: state.phrase, counts: state.counts, notes: 3 }, news: false }],
          totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0 },
        };
      }
      const served = await birta(files, GATE_ON, folder ? ["."] : undefined);
      await page.setViewportSize(WIDE);
      const mock = await mockReview(page, { state, dir });
      await page.goto(`${served.url}/view/pr.md`);
      await booted(page);
      const sent = `sent ${hhmm(state.phrase.sent.ts)} · not picked up yet`;
      await expect.poll(() => phraseOf(page)).toBe(sent);
      expect(await turnOf(page)).toEqual({ text: drafter ? "With Claude" : "With the agent", kbd: null, disabled: true });
      // The spec pushes nothing from here on, so the page's own clock must flip it
      await expect
        .poll(() => turnOf(page), { timeout: 8_000 })
        .toEqual({ text: "Copy hand-off line", kbd: await hint(page), disabled: false });
      expect(await phraseOf(page)).toBe(sent);
      await page.keyboard.press("ControlOrMeta+Enter");
      await expect.poll(copied).toEqual([state.hand_off]);
      await page.locator(".file-header birta-turn button").click();
      await expect.poll(copied).toEqual([state.hand_off, state.hand_off]);
      await page.waitForTimeout(SETTLE_MS);
      expect(mock.ops()).toEqual([]);
      expect(await phraseOf(page)).toBe(sent);
    });
  }

  test("once the agent picks up, the button is With Claude again and ⌘⏎ copies nothing", async ({ page, birta }) => {
    const copied = await captureClipboard(page);
    const { mock, state } = await openFixture(page, birta, "sent");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Copy hand-off line");
    swap(mock, state, withRev(fixtureState("picked"), state.rev));
    await expect.poll(() => turnOf(page)).toEqual({ text: "With Claude", kbd: null, disabled: true });
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(await copied()).toEqual([]);
    expect(mock.ops()).toEqual([]);
  });
});

// --- The key sheet's words ------------------------------------------------------------------------------

/** The open key sheet's rows: each row's text nodes joined by spaces, its keys, and its words. */
function sheetRows(page: Page): Promise<{ text: string; keys: string[]; note: string }[] | null> {
  return page.evaluate(() => {
    const words = (n: Node): string[] => {
      if (n.nodeType === 3) return [(n.textContent ?? "").trim()].filter(Boolean);
      if (n.nodeType === 1 && ["STYLE", "SCRIPT"].includes((n as Element).tagName)) return [];
      const root = n.nodeType === 1 ? (n as Element).shadowRoot : null;
      return Array.from((root ?? n).childNodes).flatMap(words);
    };
    for (const h of Array.from(document.querySelectorAll("birta-pop"))) {
      const root = (h.shadowRoot ?? h) as ParentNode;
      for (const el of Array.from(root.querySelectorAll('[role="dialog"]'))) {
        let open = false;
        try {
          open = el.matches(":popover-open") && el.getClientRects().length > 0;
        } catch {
          open = false;
        }
        if (!open) continue;
        return Array.from(el.querySelectorAll(".row")).map((r) => ({
          text: words(r).join(" ").replace(/\s+/g, " "),
          keys: Array.from(r.querySelectorAll("kbd")).map((k) => (k.textContent ?? "").trim()),
          note: (r.querySelector(".note")?.textContent ?? "").trim(),
        }));
      }
    }
    return null;
  });
}

test.describe("the key sheet", () => {
  test("? gives every key its words, and says a verdict is a one-word note", async ({ page, birta }) => {
    await openFixture(page, birta, "drafts");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.keyboard.press("?");
    await expect.poll(() => sheetRows(page)).not.toBeNull();
    const rows = (await sheetRows(page))!;
    expectSheetWords(rows.map((r) => r.text));
    for (const r of rows) expect(r.note, `the row ${JSON.stringify(r.text)} has words`).not.toBe("");
    const mac = await page.evaluate(() => /Mac/.test(navigator.platform));
    // Every default key, the literal ones too, is on the sheet
    const keys = rows.flatMap((r) => r.keys);
    for (const k of ["y", "x", "l", ".", "c", "q", "j", "k", "n", "p", "⇧J", "⇧K", "Home", "End", "z", "u", "⏎", "Esc", "Tab", "?", "m"])
      expect(keys, `the sheet names ${k}`).toContain(k);
    expect(keys).toContain(await hint(page));
    expect(keys).toContain(`${mac ? "⌥" : "Alt"}u`);
    await page.keyboard.press("Escape");
    await expect.poll(() => sheetRows(page)).toBeNull();
  });
});

// --- adversary: the button the composer presses, and a phrase too long for two lines ---------------

test.describe("adversary: the hand-off line from the composer", () => {
  test("adversary: ⌘⏎ in the composer saves, then presses Copy hand-off line: it copies and never hands back again", async ({ page, birta }) => {
    const copied = await captureClipboard(page);
    const { mock, state } = await openFixture(page, birta, "sent");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Copy hand-off line");
    await page.keyboard.press("c");
    const field = page.locator('birta-flow[data-kind="composer"] textarea');
    await expect(field).toBeFocused();
    await page.keyboard.type("one more thing");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().length).toBeGreaterThanOrEqual(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().map((o) => o.op)).toEqual(["comment"]);
    await expect.poll(copied).toEqual([state.hand_off]);
  });
});

test.describe("adversary: a phrase three lines long", () => {
  test("adversary: at 520 px a phrase that needs three lines shows them all, inside the header", async ({ page, birta }) => {
    const name = "an-agent-with-a-name-far-longer-than-any-header-has-room-for-and-then-some-more-of-it-to-reach-a-third-line-at-least";
    await openFixture(
      page,
      birta,
      "replied",
      (s) => (s.phrase.picked_up.by = s.phrase.replied.by = s.rounds[0].claimed_by = name),
      { viewport: { width: 520, height: 900 } },
    );
    await expect.poll(() => phraseOf(page)).toMatch(/ replied 14:13 · 3 to check$/);
    const laid = await page.evaluate(() => {
      const host = document.querySelector(".file-header birta-phrase")!;
      const root = host.shadowRoot ?? host;
      const els = [host, ...Array.from(root.querySelectorAll("*"))].filter((el) => el.localName !== "style");
      const r = document.createRange();
      r.selectNodeContents(root);
      const rects = Array.from(r.getClientRects()).filter((x) => x.width > 0);
      const header = document.querySelector(".file-header")!.getBoundingClientRect();
      const box = host.getBoundingClientRect();
      return {
        lines: new Set(rects.map((x) => Math.round(x.top))).size,
        clipped: els.filter((el) => el.scrollHeight > el.clientHeight + 1 || el.scrollWidth > el.clientWidth + 1).map((el) => el.localName),
        // Every line of words is painted inside the phrase's box and the header's
        outside: rects.filter((x) => x.top < Math.max(header.top, box.top) - 0.5 || x.bottom > Math.min(header.bottom, box.bottom) + 0.5).length,
      };
    });
    expect(laid.lines, "setup: the phrase needs three lines").toBeGreaterThanOrEqual(3);
    expect(laid.clipped, "no box in the phrase clips its words").toEqual([]);
    expect(laid.outside, "no line of the phrase falls outside its box or the header").toBe(0);
  });
});

// --- repair: the composer's ⌘⏎ hint follows the button live, and the header at narrow widths ------

/** The open composer's hint line, or null with no composer. */
const composerHint = (page: Page) =>
  page.evaluate(() => {
    const host = document.querySelector('birta-flow[data-kind="composer"]');
    return host ? ((host.shadowRoot ?? host).querySelector(".hint")?.textContent ?? "").trim() : null;
  });

test.describe("repair: the composer's hint follows the button", () => {
  test("open before the 10 s switch, the hint gains ⌘⏎ for Copy hand-off line when the button turns, and the strip says the same", async ({
    page,
    birta,
  }) => {
    await openFixture(page, birta, "sent", (s) => (s.server_now = s.phrase.sent.ts + 5_000));
    await expect.poll(() => turnOf(page)).toEqual({ text: "With Claude", kbd: null, disabled: true });
    await page.keyboard.press("c");
    await expect(page.locator('birta-flow[data-kind="composer"] textarea')).toBeFocused();
    expect(await composerHint(page)).toBe("⏎ save · ⇧⏎ new line · esc save & close");
    const copy = `⏎ save · ⇧⏎ new line · ${await hint(page)} save & copy hand-off line · esc save & close`;
    await expect.poll(() => composerHint(page), { timeout: 8_000 }).toBe(copy);
    expect((await turnOf(page))?.text).toBe("Copy hand-off line");
    await expect.poll(() => stripKeys(page)).toBe(copy);
  });

  test("open when birta stops, the hint drops ⌘⏎ as the button goes disabled", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "fresh");
    await page.keyboard.press("c");
    await expect(page.locator('birta-flow[data-kind="composer"] textarea')).toBeFocused();
    expect(await composerHint(page)).toBe(`⏎ save · ⇧⏎ new line · ${await hint(page)} save & hand back · esc save & close`);
    // Later routes win: every reconnect is refused from here on
    await page.routeWebSocket(/\/ws$/, (ws) => ws.close());
    await mock.drop();
    await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toBe("birta stopped · notes wait here");
    expect((await turnOf(page))?.disabled).toBe(true);
    await expect.poll(() => composerHint(page)).toBe("⏎ save · ⇧⏎ new line · esc save & close");
  });
});

test.describe("repair: the header at narrow widths", () => {
  for (const width of [320, 520]) {
    test(`at ${width} px the phrase takes at most two header lines, whole, and the header stays short`, async ({ page, birta }) => {
      await openFixture(page, birta, "replied", undefined, { viewport: { width, height: 800 } });
      await expect.poll(() => phraseOf(page)).toBe("Claude replied 14:13 · 3 to check");
      const h = await page.evaluate(() => {
        const host = document.querySelector(".file-header birta-phrase")!;
        const root = host.shadowRoot ?? host;
        // Lines of the words themselves: each text node's own rects, not the boxes around them
        const tops = new Set<number>();
        const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
        for (let t = walker.nextNode(); t; t = walker.nextNode()) {
          if (!(t.textContent ?? "").trim() || t.parentElement?.localName === "style") continue;
          const r = document.createRange();
          r.selectNodeContents(t);
          for (const x of Array.from(r.getClientRects())) if (x.width > 0) tops.add(Math.round(x.top));
        }
        const lines = tops.size;
        const name = document.querySelector(".file-header-name")!;
        const header = document.querySelector(".file-header")!.getBoundingClientRect();
        const turn = document.querySelector(".file-header birta-turn")!.getBoundingClientRect();
        return {
          lines,
          width: host.getBoundingClientRect().width,
          header: header.height,
          nameWhole: name.scrollWidth <= name.clientWidth && name.getBoundingClientRect().width > 0,
          turnInside: turn.left >= header.left - 0.5 && turn.right <= header.right + 0.5 && turn.width >= 112,
        };
      });
      expect(h.lines, "the phrase wraps to two lines at most").toBeLessThanOrEqual(2);
      expect(h.width, "the phrase gets a line's worth of room, not a column").toBeGreaterThanOrEqual(150);
      // A header row is 26 px: two phrase lines and a row for the button, with padding
      expect(h.header, "the header stays a few lines tall").toBeLessThanOrEqual(130);
      expect(h.nameWhole, "the file name shows whole").toBe(true);
      expect(h.turnInside, "the button keeps its width inside the header").toBe(true);
    });
  }
});
