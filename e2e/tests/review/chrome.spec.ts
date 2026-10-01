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
  /** The pill's text while it is open in the top layer. */
  pill: string | null;
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

    let pill: string | null = null;
    let menu: string[] | null = null;
    for (const root of roots("birta-pop")) {
      for (const el of Array.from(root.querySelectorAll('[role="status"]')))
        if (open(el) && textOf(el)) pill = textOf(el);
      for (const el of Array.from(root.querySelectorAll('[role="menu"]')))
        if (open(el)) menu = Array.from(el.querySelectorAll('[role="menuitem"]')).map((r) => textOf(r));
    }
    return { phrase, turn, pill, menu };
  });
}

const phraseOf = async (page: Page) => (await chrome(page)).phrase?.text ?? null;
const pillOf = async (page: Page) => (await chrome(page)).pill;
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

/** The doc's own item stops, which the fresh phrase counts. */
async function itemCount(page: Page): Promise<number> {
  return (await stops(page)).filter((s) => s.kind === "list_item" || s.kind === "table_row").length;
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

// --- The phrase and the turn button ----------------------------------------------------------

test.describe("phrase and button", () => {
  const SHOWN: [string, string, string, "hint" | null, boolean][] = [
    ["fresh", "drafted by Claude · 3 items · your turn", "Approve", "hint", false],
    ["drafts", "1 of 3 decided", "Hand back · 3", "hint", false],
    ["dir", "read-only · notes only", "Hand back · 2", "hint", false],
    ["sent", "sent 14:13 · not picked up yet", "With Claude", null, true],
    ["replied", "Claude replied 14:13 · 1 done · 1 answered · 1 not handled", "Approve · 1 unseen", null, false],
    ["approved", "approved 14:13 · 1 later", "Approved", null, true],
  ];
  for (const [name, phrase, label, kbd, disabled] of SHOWN) {
    test(`${name}.json reads "${phrase}" beside "${label}"`, async ({ page, birta }) => {
      await openFixture(page, birta, name);
      await expect.poll(() => phraseOf(page)).toBe(phrase);
      expect(await turnOf(page)).toEqual({ text: label, kbd: kbd && (await hint(page)), disabled });
    });
  }

  test("a long phrase gives way before the button, and keeps its full text as its title", async ({ page, birta }) => {
    const full = "Claude replied 14:13 · 12 done · 3 answered · 2 for you · 4 unasked · 5 not handled";
    await openFixture(page, birta, "replied", (s) =>
      Object.assign(s.phrase.replied, { done: 12, answered: 3, asked: 2, unasked: 4, not_handled: 5 }),
    );
    await expect.poll(() => phraseOf(page)).toBe(full);
    const c = await chrome(page);
    expect(c.phrase!.title).toBe(full);
    expect(c.phrase!.box.right).toBeLessThanOrEqual(c.turn!.box.left);
    // Truncated by CSS: some box in the phrase is narrower than its text, with an ellipsis
    const cut = await page.evaluate(() => {
      const host = document.querySelector(".file-header birta-phrase")!;
      const els = [host, ...Array.from((host.shadowRoot ?? host).querySelectorAll("*"))];
      return els.some((el) => el.scrollWidth > el.clientWidth + 1 && getComputedStyle(el).textOverflow === "ellipsis");
    });
    expect(cut).toBe(true);
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
        ["Approve · 1 unseen", withRev(fixtureState("replied"), revOf(doc))],
        ["With Claude", withRev(fixtureState("sent"), revOf(doc))],
        ["Approved", withRev(fixtureState("approved"), revOf(doc))],
        ["Approve · 1 unseen", long],
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
        expect(c.phrase!.title, label).toBe(c.phrase!.text);
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
    expect(c.phrase?.text).toBe("Claude replied 14:13 · 1 done · 1 answered · 2 unasked · 1 not handled");
    expect(c.turn?.text).toBe("Approve · 3 unseen");
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
    const n = await itemCount(page);
    await expect.poll(() => phraseOf(page)).toBe(`${n} items · your turn`);
    expect(await turnOf(page)).toEqual({ text: "Approve", kbd: await hint(page), disabled: false });
    await page.reload();
    await booted(page);
    await expect.poll(() => phraseOf(page)).toBe(`${n} items · your turn`);
    // Another tab hasn't started a review
    const other = await secondPage(page, `${served.url}/view/pr.md`);
    await other.waitForTimeout(SETTLE_MS);
    await expect(other.locator(".file-header birta-phrase")).toBeHidden();
    expect(reviewLog(served.root)).toBeNull();
  });

  test("an agent's draft shows its phrase and Approve on load, and viewing writes nothing", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, {}, ["pr.md", "--drafted-by", "claude"]);
    const n = await itemCount(page);
    await expect.poll(() => phraseOf(page)).toBe(`drafted by Claude · ${n} items · your turn`);
    expect(await turnOf(page)).toEqual({ text: "Approve", kbd: await hint(page), disabled: false });
    expect(reviewLog(served.root)).toBeNull();
  });

  test("a review file of yours shows the chrome on load", async ({ page, birta }) => {
    await openFixture(page, birta, "fresh", (s) => (s.owner.drafted_by = null));
    await expect.poll(() => phraseOf(page)).toBe("3 items · your turn");
  });
});

// --- What the turn button and ⌘⏎ send ------------------------------------------------------------

test.describe("turn", () => {
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
    await expect.poll(() => pillOf(page)).toBe("changed since · try again");
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
      await expect.poll(() => pillOf(page)).toBe(text);
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
    await expect.poll(() => pillOf(page)).toBe("Codex already picked it up");
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
      const { mock } = await openFixture(page, birta, name);
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
    await expect.poll(() => pillOf(page)).toBe("handing back · 3 notes · z to undo");
    const c = await chrome(page);
    expect(c.phrase?.text).toBe("1 of 3 decided");
    expect(c.turn).toMatchObject({ text: "Hand back · 3", kbd: await hint(page), disabled: false });
    expect(await glyphRoles(page)).toEqual(["you", "you"]);
    await page.waitForTimeout(1_000);
    expect(await pillOf(page)).toBe("handing back · 3 notes · z to undo");
  });

  test("when the hold passes, with no push, the hand-back has left", async ({ page, birta }) => {
    const { state } = await openFixture(page, birta, "hold", (s) => (s.hold.not_before = s.server_now + 1_500));
    await expect.poll(() => pillOf(page)).toBe("handing back · 3 notes · z to undo");
    expect(await glyphRoles(page)).toEqual(["you", "you"]);
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toBe(`sent ${hhmm(state.phrase.sent.ts)} · not picked up yet`);
    const c = await chrome(page);
    expect(c.pill).toBeNull();
    expect(c.turn).toMatchObject({ text: "With Claude", kbd: null, disabled: true });
    await expect.poll(() => glyphRoles(page)).toEqual(["agent", "agent"]);
  });

  test("inside an approve's hold the phrase and button keep their words and the pill says approving", async ({
    page,
    birta,
  }) => {
    await openFixture(page, birta, "approved", (s) => (s.hold.not_before = s.server_now + 20_000));
    await expect.poll(() => pillOf(page)).toBe("approving · z to undo");
    const c = await chrome(page);
    expect(c.phrase?.text).toBe("Claude replied 14:13 · 1 done · 1 answered · 1 not handled");
    expect(c.turn).toMatchObject({ text: "Approve", kbd: await hint(page), disabled: false });
  });

  test("with the socket down inside the hold the pill says the hand-back stands", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "hold", (s) => (s.hold.not_before = s.server_now + 30_000));
    await expect.poll(() => pillOf(page)).toBe("handing back · 3 notes · z to undo");
    // Later routes win: every reconnect is refused from here on
    await page.routeWebSocket(/\/ws$/, (ws) => ws.close());
    await mock.drop();
    await expect.poll(() => pillOf(page), { timeout: 3_000 }).toBe("connection lost · hand-back stands");
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
    await expect.poll(() => phraseOf(page)).toBe("your turn · 1 note");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(5);
    const submit = reviewLog(served.root)![4];
    expect(submit.op).toBe("submit");

    const other = await secondPage(page, `${served.url}/view/pr.md`);
    const during = {
      phrase: "your turn · 1 note",
      turn: { text: "Hand back · 1", kbd: await hint(page), disabled: false },
      pill: "handing back · 1 note · z to undo",
    };
    for (const p of [page, other]) {
      await expect.poll(() => pillOf(p)).toBe(during.pill);
      expect(await phraseOf(p)).toBe(during.phrase);
      expect(await turnOf(p)).toEqual(during.turn);
    }

    // A hand-back is sent when its hold ends, which can fall in the next minute
    const after = `sent ${hhmm(submit.not_before)} · not picked up yet`;
    for (const p of [page, other]) {
      await expect.poll(() => phraseOf(p), { timeout: 8_000 }).toBe(after);
      expect(await pillOf(p)).toBeNull();
      expect(await turnOf(p)).toEqual({ text: "With Claude", kbd: null, disabled: true });
    }

    await other.locator(".file-header birta-phrase").click();
    await expect.poll(() => menuOf(other)).toContain(WITHDRAW);
    await other.locator('birta-pop [role="menuitem"]', { hasText: WITHDRAW }).click();
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(6);
    expect(reviewLog(served.root)![5]).toMatchObject({ op: "withdraw", to: 5 });
    for (const p of [page, other]) await expect.poll(() => phraseOf(p)).toBe("your turn · 1 note");
  });

  test("on a real server an approve holds, then names its time", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, { BIRTA_REVIEW_HOLD_MS: "3000" }, ["pr.md", "--drafted-by", "claude"]);
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => pillOf(page)).toBe("approving · z to undo");
    const log = reviewLog(served.root)!;
    const approve = log[log.length - 1];
    expect(approve).toMatchObject({ op: "approve", rev: revOf(STOPS_DOC) });
    await expect.poll(() => phraseOf(page), { timeout: 6_000 }).toBe(`approved ${hhmm(approve.not_before)}`);
    expect(await pillOf(page)).toBeNull();
    expect(await turnOf(page)).toEqual({ text: "Approved", kbd: null, disabled: true });
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
    await expect.poll(() => menuOf(page)).toEqual([WITHDRAW, COPY, MINE]);
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
      // The first row is current on open
      await page.keyboard.press("j");
      await page.keyboard.press("Enter");
      await expect.poll(copied).toEqual([state.hand_off]);
      expect(await focused(page)).toBeNull();
      expect(mock.ops()).toEqual([]);
      await expect.poll(() => menuOf(page)).toBeNull();
      await openMenu();
      await page.keyboard.press("j");
      await page.keyboard.press("k");
      await page.keyboard.press("Enter");
      await expect.poll(() => mock.ops().length).toBe(1);
      expect(mock.ops()[0]).toMatchObject({ op: "withdraw", to: state.phrase.sent.n });
      expect(await focused(page)).toBeNull();
    });
  }

  test("after 15 minutes of silence, with no push, the phrase says so and release lets go", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "picked", (s) => {
      s.server_now = s.phrase.picked_up.silence.since_ts + 15 * MIN - 1_500;
    });
    await expect.poll(() => phraseOf(page)).toBe("Claude picked up 14:13");
    await openMenu(page);
    expect(await menuOf(page)).not.toContain("release");
    await page.keyboard.press("Escape");
    await expect
      .poll(() => phraseOf(page), { timeout: 4_000 })
      .toBe("Claude picked up 14:13 · no reply in 15 min · release");
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: "release" }).click();
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toMatchObject({ op: "release", claim: state.phrase.picked_up.n });
  });

  test("on a real server the owner row marks an agent's draft as yours", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, {}, ["pr.md", "--drafted-by", "claude"]);
    const n = await itemCount(page);
    await expect.poll(() => phraseOf(page)).toBe(`drafted by Claude · ${n} items · your turn`);
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: MINE }).click();
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    const log = reviewLog(served.root)!;
    expect(log[2]).toMatchObject({ op: "owner", drafted_by: "claude" });
    expect(log[3]).toMatchObject({ op: "owner", drafted_by: null });
    await expect.poll(() => phraseOf(page)).toBe(`${n} items · your turn`);
  });

  test("Esc drops one layer per press: theme menu, phrase menu, composer, then focus", async ({ page, birta }) => {
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
    await expect.poll(layers).toEqual({ theme: false, menu: false, composer: 0, focus: null });
  });

  test("with focus on the page, Esc closes the menu before the composer and the doc's focus", async ({ page, birta }) => {
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
    await expect.poll(layers).toEqual({ menu: false, composer: 0, focus: null });
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
      const texts = [c.phrase!.text, c.phrase!.title ?? "", c.turn!.text, c.pill ?? "", ...c.menu!];
      for (const t of texts) {
        expect(t).not.toMatch(/\bby\s*(·|$)/);
        expect(t).not.toMatch(/undefined|null|NaN/);
      }
    });
  }
});

// --- The pill ----------------------------------------------------------------------------------------------

test.describe("pill", () => {
  test("the selection hint is a popover in the top layer", async ({ page, birta }) => {
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
    await expect.poll(() => pillOf(page)).toBe("c note · q ask · x cut");
    await page.keyboard.press("Escape");
    await expect.poll(() => pillOf(page)).toBeNull();
  });

  test("the review-file-created pill is a popover in the top layer", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC);
    await page.keyboard.press("x");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    await expect.poll(() => pillOf(page)).toBe("review file created · z to undo");
  });
});

// --- The tab -------------------------------------------------------------------------------------------------

test.describe("tab", () => {
  const icon = (page: Page) => page.evaluate(() => document.querySelector('link[rel~="icon"]')!.getAttribute("href"));

  test("news puts ● in the title and a dot on the favicon, and both go when it does", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen");
    expect(state.counts.attention).toBeGreaterThan(0);
    await expect(page).toHaveTitle("● pr.md — birta");
    await expect.poll(() => icon(page)).toMatch(/^data:image\/png;base64,/);
    const quiet = withRev(fixtureState("fresh"), state.rev);
    expect(quiet.counts.attention).toBe(0);
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
    await expect.poll(() => pillOf(page)).toBe("handing back · 3 notes · z to undo");
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toBe(`sent ${hhmm(state.phrase.sent.ts)} · not picked up yet`);
    expect(await pillOf(page)).toBeNull();
  });

  test("a hold that arrives in a later push ends on time with no further push", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "drafts");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    const held = withRev(fixtureState("hold"), state.rev);
    // One server clock: the page's never runs backwards
    held.server_now = state.server_now;
    held.hold.not_before = held.server_now + 1_500;
    swap(mock, state, held);
    await expect.poll(() => pillOf(page)).toBe("handing back · 3 notes · z to undo");
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toBe(`sent ${hhmm(held.phrase.sent.ts)} · not picked up yet`);
  });

  test("the silence keeps counting minutes with no push", async ({ page, birta }) => {
    await openFixture(page, birta, "picked", (s) => (s.server_now = s.phrase.picked_up.silence.since_ts + 15 * MIN + 58_000));
    await expect.poll(() => phraseOf(page)).toBe("Claude picked up 14:13 · no reply in 15 min · release");
    await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toBe("Claude picked up 14:13 · no reply in 16 min · release");
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
    await expect.poll(() => menuOf(page)).toEqual([COPY, MINE]);
  });

  test("Approve with unseen changes sends no approve; with only unanswered questions it does", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "replied");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve · 1 unseen");
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.locator(".file-header birta-turn button").click();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().filter((o) => o.op === "approve")).toEqual([]);
    const asks = JSON.parse(JSON.stringify(state));
    asks.counts.unseen = 0;
    asks.counts.open_asks = 1;
    swap(mock, state, asks);
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Approve · 1 unanswered");
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
  test("the hold pill replaces review-file-created, which doesn't come back after", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, { BIRTA_REVIEW_HOLD_MS: "1500" });
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe("review file created · z to undo");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => pillOf(page)).toBe("handing back · 1 note · z to undo");
    await expect.poll(() => phraseOf(page), { timeout: 4_000 }).toMatch(/^sent \d\d:\d\d · not picked up yet$/);
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).toBeNull();
    expect(reviewLog(served.root)!.some((l) => l.op === "submit")).toBe(true);
  });

  test("with the socket down outside a hold nothing claims a hand-back stands", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await expect.poll(() => phraseOf(page)).not.toBeNull();
    await page.routeWebSocket(/\/ws$/, (ws) => ws.close());
    await mock.drop();
    await page.waitForTimeout(2_000);
    expect(await pillOf(page)).toBeNull();
  });

  test("when the socket comes back inside the hold the hold pill returns", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "hold", (s) => (s.hold.not_before = s.server_now + 30_000));
    await expect.poll(() => pillOf(page)).toBe("handing back · 3 notes · z to undo");
    await mock.drop(); // no refusal route: the page reconnects after ~1 s
    await expect.poll(() => pillOf(page)).toBe("connection lost · hand-back stands");
    await expect.poll(() => pillOf(page), { timeout: 6_000 }).toBe("handing back · 3 notes · z to undo");
  });
});

test.describe("killers: names and tab", () => {
  test("a drafter's name with markup is text, never elements", async ({ page, birta }) => {
    const evil = '<img src=x onerror="window.__pwned=1"><b>bold</b>';
    await openReal(page, birta, STOPS_DOC, {}, ["pr.md", "--drafted-by", evil]);
    const n = await itemCount(page);
    await expect.poll(() => phraseOf(page)).toBe(`drafted by ${evil} · ${n} items · your turn`);
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

  test("a long agent name keeps the button's right edge and the header on one line", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "picked");
    await expect.poll(async () => (await turnOf(page))?.text).toBe("With Claude");
    const header = () => page.evaluate(() => document.querySelector(".file-header")!.getBoundingClientRect().height);
    const before = { c: await chrome(page), h: await header() };
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
    expect(await header()).toBe(before.h);
  });

  test("the phrase's title follows every push", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "fresh");
    for (const name of ["drafts", "sent", "approved"]) {
      swap(mock, state, withRev(fixtureState(name), state.rev));
      await expect.poll(async () => (await chrome(page)).phrase?.title).toBe(await phraseOf(page));
    }
  });

  test("not-handled rows alone are news", async ({ page, birta }) => {
    const { state } = await openFixture(page, birta, "approved");
    expect(state.counts).toMatchObject({ unseen: 0, attention: 1 });
    await expect(page).toHaveTitle("● pr.md — birta");
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
    await expect.poll(() => pillOf(page)).toBe("nothing to hand back");
    await page.waitForTimeout(2_500);
    expect(await pillOf(page)).toBe("nothing to hand back");
    await expect.poll(() => pillOf(page), { timeout: 3_000 }).toBeNull();
  });

  test("a refusal's pill goes when the next op is kept", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", undefined, { acks: "manual" });
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "nothing_to_send");
    await expect.poll(() => pillOf(page)).toBe("nothing to hand back");
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(2);
    mock.ack(mock.ops()[1].cid);
    await expect.poll(() => pillOf(page), { timeout: 1_500 }).toBeNull();
  });

  test("a new hold replaces a refusal's pill", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "drafts", undefined, { acks: "manual" });
    await expect.poll(async () => (await turnOf(page))?.text).toBe("Hand back · 3");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "hold_pending");
    await expect.poll(() => pillOf(page)).toBe("already handing back");
    const held = withRev(fixtureState("hold"), state.rev);
    held.server_now = state.server_now;
    held.hold.not_before = held.server_now + 20_000;
    swap(mock, state, held);
    await expect.poll(() => pillOf(page)).toBe("handing back · 3 notes · z to undo");
  });

  test("a claimed refusal that comes before its state names the agent once the state does", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "sent", (s) => (s.owner.drafted_by = null), { acks: "manual" });
    await openMenu(page);
    await page.locator('birta-pop [role="menuitem"]', { hasText: WITHDRAW }).click();
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(mock.ops()[0].cid, "claimed");
    await expect.poll(() => pillOf(page)).toBe("the agent already picked it up");
    const picked = withRev(fixtureState("picked"), state.rev);
    picked.owner.drafted_by = null;
    picked.phrase.picked_up.by = picked.rounds[0].claimed_by = "codex";
    swap(mock, state, picked);
    await expect.poll(() => pillOf(page)).toBe("Codex already picked it up");
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
      await page.keyboard.press("x");
      await expect.poll(() => mock.ops().length).toBe(1);
      expect(mock.ops()[0]).toMatchObject({ op: "comment", verdict: "no", at: { k: "doc" } });
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
    expect(await pillOf(page)).toBeNull();
  });
});
