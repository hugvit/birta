import { Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  GATE_ON,
  Mock,
  REPO,
  State,
  booted,
  fixtureDoc,
  fixtureState,
  focusTo,
  focusTop,
  focused,
  marks,
  mockReview,
  revOf,
  reviewLog,
  stopSp,
  stops,
  withRev,
  stripMessage,
  SIZE_LABEL,
  changeRows,
  focusMark,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
/** How long after the approve guard's jump a ⌘⏎ is still ignored. */
const DWELL_MS = 1_000;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

interface OpenOpts {
  acks?: "auto" | "manual";
  viewport?: { width: number; height: number };
}

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, state: State | undefined, opts: OpenOpts = {}) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(opts.viewport ?? WIDE);
  const mock = await mockReview(page, { state, acks: opts.acks });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

/** Serves committed fixture `name` with its state, bent by `edit`. */
async function openFixture(page: Page, birta: Start, name: string, edit?: (s: State) => void, opts: OpenOpts = {}) {
  const state = fixtureState(name);
  edit?.(state);
  const out = await open(page, birta, fixtureDoc(name), state, opts);
  return { ...out, state };
}

/**
 * Turns the mock's own state object into `next` and pushes it. The mock resends that object
 * whenever the server repeats `content`, so a pushed copy alone could be undone.
 */
function swap(mock: Mock, state: State, next: State) {
  const copy = JSON.parse(JSON.stringify(next));
  for (const k of Object.keys(state)) delete state[k];
  Object.assign(state, copy);
  mock.pushState(state);
}

/** The fold's count for `s`: the stops still holding unchecked news, which the tab and the guard read. */
function recount(s: State): State {
  const stopOf = (n: State) => {
    if (n.kind === "removal") return `r:${n.id}`;
    if (n.kind === "question") return `q:${n.id}`;
    if (n.sp) return `b:${n.sp}`;
    const item = s.items.find((i: State) => `#${i.n}` === n.id || (i.replies ?? []).some((r: State) => r.id === n.id));
    return item?.at?.change ? `r:${item.at.change}` : "doc";
  };
  s.counts.to_check = new Set(s.news.filter((n: State) => n.check == null).map(stopOf)).size;
  s.counts.attention = s.counts.to_check;
  return s;
}

/** Checks news items `ids` as a seen would: a change or removal kept, anything else ok. */
function check(s: State, ids: string[]): State {
  for (const id of ids) {
    const n = s.news.find((x: State) => x.id === id);
    if (n && n.check == null) n.check = n.kind === "change" || n.kind === "removal" ? "kept" : n.kind === "question" ? "answered" : "ok";
    const c = s.changes.find((x: State) => x.id === id);
    if (c && c.check == null) c.check = "kept";
  }
  return recount(s);
}

/** Checks every news item: nothing is left to check. */
const checkAll = (s: State) => check(s, s.news.map((n: State) => n.id));

/** What the server pushes once it has kept `seen` for `ids`: each change seen, each item checked. */
function echoSeen(state: State, ...ids: string[]): State {
  const next = JSON.parse(JSON.stringify(state));
  for (const id of ids) {
    const c = next.changes.find((x: State) => x.id === id);
    if (c && !c.seen) {
      c.seen = true;
      next.counts.unseen -= 1;
    }
  }
  return check(next, ids);
}

/** The fields of a committed op frame, without the envelope the core adds. */
function fixtureOp(name: string): State {
  const file = path.join(REPO, "e2e", "fixtures", "review", "ops", `${name}.json`);
  return opFields(JSON.parse(fs.readFileSync(file, "utf-8")).frame);
}

function opFields(frame: State): State {
  const out = { ...frame };
  for (const k of ["type", "cid", "path", "rev"]) delete out[k];
  return out;
}

const seenOps = (mock: Mock) => mock.ops().filter((o) => o.op === "seen");
/** Each seen op's ids, sorted: a stop's ids may go in any order. */
const seenIds = (mock: Mock) => seenOps(mock).map((o) => [...o.ids].sort());
const opsNamed = (mock: Mock, name: string) => mock.ops().filter((o) => o.op === name);

/** The text of the flow host of `kind` whose text holds `has`, read through its (forced open) root. */
function hostText(page: Page, kind: string, has: string): Promise<string | null> {
  return page.evaluate(
    ({ kind, has }) => {
      for (const h of document.querySelectorAll(`birta-flow[data-kind="${kind}"]`)) {
        const root = h.shadowRoot;
        if (!root) continue;
        const text = Array.from(root.children)
          .filter((c) => c.localName !== "style")
          .map((c) => (c as HTMLElement).innerText ?? c.textContent ?? "")
          .join("\n")
          .replace(/[ \t]+/g, " ")
          .trim();
        if (text.includes(has)) return text;
      }
      return null;
    },
    { kind, has },
  );
}

const receipt = (page: Page, has: string) => page.locator('birta-flow[data-kind="receipt"]', { hasText: has });
/** Claude's change row under your own note, which opens with its block. */
const DONE = "↳ Claude · done";

/** Focuses the block under your note holding `has`, which opens Claude's done row there, and clicks the row. */
async function clickDone(page: Page, has = "say why it fails") {
  // Read from the page: after an outside write the boot data's stops are stale
  await expect.poll(() => slotOf(page, "note", has)).not.toBeNull();
  await focusTo(page, (await slotOf(page, "note", has))!);
  await receipt(page, DONE).click();
}
const reply = (page: Page, has: string) => page.locator('birta-flow[data-kind="reply"]', { hasText: has });

/** The page's own count of stops left to check. */
const toCheckCount = (page: Page) =>
  page.evaluate(() => (window as unknown as { BR: { currentState(): State } }).BR.currentState().counts.to_check);

/** The turn button's label without its key hint, and whether the hint shows. */
function turnOf(page: Page): Promise<{ text: string; kbd: boolean } | null> {
  return page.evaluate(() => {
    const t = document.querySelector(".file-header birta-turn");
    if (!t || !t.getClientRects().length) return null;
    const btn = (t.shadowRoot ?? t).querySelector("button");
    if (!btn) return null;
    const kbd = btn.querySelector("kbd");
    const all = (btn.textContent ?? "").replace(/\s+/g, " ").trim();
    const k = kbd ? (kbd.textContent ?? "").trim() : "";
    return { text: k && all.endsWith(k) ? all.slice(0, -k.length).trim() : all, kbd: !!kbd };
  });
}

/** Sourcepos of the blocks carrying a `--review-new` wash. */
async function washed(page: Page): Promise<string[]> {
  return (await marks(page))
    .filter((m) => m.mark === "wash" && m.width > 0)
    .map((m) => m.sp!)
    .sort();
}

/** The key hint the turn button shows on this platform. */
function hint(page: Page): Promise<string> {
  return page.evaluate(() => (/Mac/.test(navigator.platform) ? "⌘⏎" : "Ctrl⏎"));
}

/** Presses j or k until `sp` has focus. */
async function focusOn(page: Page, sp: string) {
  if (!(await stops(page)).some((s) => s.sp === sp)) throw new Error(`${sp} is not a stop`);
  await focusTo(page, sp);
}

/** Presses n until the walk lands on `sp`, failing after `max` stops. */
/** The question of the blank that holds the keyboard, or null. */
function blankAt(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const a = document.activeElement;
    return a?.getAttribute("data-kind") === "blank" ? (a.shadowRoot?.querySelector(".text bdi")?.textContent ?? "") : null;
  });
}

/** Presses n until the blank asking `question` holds the keyboard: a blank is its own stop. */
async function nToBlank(page: Page, question: string, max = 12) {
  for (let i = 0; i < max && (await blankAt(page)) !== question; i++) {
    await page.keyboard.press("n");
    await page.waitForTimeout(50);
  }
  await expect.poll(() => blankAt(page)).toBe(question);
}

async function nTo(page: Page, sp: string, max = 12) {
  for (let i = 0; i < max && (await focused(page)) !== sp; i++) {
    await page.keyboard.press("n");
    await page.waitForTimeout(50);
  }
  await expect.poll(() => focused(page)).toBe(sp);
}

/** Clicks the first text of the block `sp`, as a reader clicking into it would. */
async function clickIn(page: Page, sp: string) {
  const box = await page.evaluate((sp) => {
    const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
    const el = all[all.length - 1]!;
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => ((n.textContent ?? "").trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP),
    });
    const r = document.createRange();
    r.selectNodeContents(walker.nextNode()!);
    const b = r.getClientRects()[0];
    return { x: b.left + Math.min(20, b.width / 2), y: b.top + b.height / 2 };
  }, sp);
  await page.mouse.click(box.x, box.y);
}

// --- Seen ------------------------------------------------------------------------------------

test.describe("seen", () => {
  test("n walks every stop in doc order and sends no seen; y on each stop sends one seen naming all its news", async ({
    page,
    birta,
  }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    // Each stop: where n lands, and what one y there checks
    const walk: [string, string[]][] = [
      ["doc", ["#13", "#14"]],
      ["5:1-6:68", ["#4", "9.0"]],
      ["10:1-10:16", ["nh:5"]],
      ["11:1-11:44", ["10.0"]],
    ];
    const want: string[][] = [];
    for (const [at, ids] of walk) {
      await page.keyboard.press("n");
      await expect.poll(() => focused(page)).toBe(at);
      await page.waitForTimeout(SETTLE_MS);
      // Landing checks nothing
      expect(seenIds(mock)).toEqual(want);
      await page.keyboard.press("y");
      want.push(ids);
      await expect.poll(() => seenIds(mock)).toEqual(want);
    }
    await page.keyboard.press("n");
    await expect.poll(async () => (await focusMark(page))?.change).toBe("11.0");
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual(want);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([...want, ["11.0"]]);
    // n has nowhere further to go in one file
    await page.keyboard.press("n");
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toHaveLength(5);
    expect(mock.ops().filter((o) => o.op !== "seen")).toEqual([]);
  });

  test("p walks back to the change before", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    await nTo(page, "11:1-11:44");
    await page.keyboard.press("p");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await page.keyboard.press("p");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
  });

  test("n keeps walking when a step leaves the keys in a receipt, past every kind of change", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    // The doc slot's rows, then the questions: nothing is checked on the way
    await nToBlank(page, "is 503 the only code?");
    expect(seenIds(mock)).toEqual([]);
    // The open blank takes y as its answer
    await page.keyboard.press("y");
    await expect.poll(() => opsNamed(mock, "reply")).toEqual([expect.objectContaining({ op: "reply", to: 21, verdict: "yes" })]);
    // A receipt, an outside write, a not-handled row, a change with its done row, a reply alone, an unasked change, another outside write
    const want = ["5:1-6:68", "8:1-8:15", "10:1-10:16", "11:1-11:44", "12:1-13:0", "14:1-14:21", "16:1-16:19"];
    const got: (string | null)[] = [];
    for (let i = 0; i < want.length; i++) {
      await page.keyboard.press("n");
      await page.waitForTimeout(50);
      got.push(await focused(page));
    }
    expect(got).toEqual(want);
    expect(seenIds(mock)).toEqual([]);
    expect(mock.ops().filter((o) => o.op === "comment")).toEqual([]);
  });

  test("seen goes out in the same moment as the y, not after a pause", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen");
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("doc");
    // Times the page's own send, since the socket mock adds its own delay
    await page.evaluate(() => {
      const w = window as unknown as { birtaBus: { send(f: State): boolean }; __sent: { t: number; op: string }[]; __keyAt: number };
      w.__sent = [];
      const send = w.birtaBus.send.bind(w.birtaBus);
      w.birtaBus.send = (frame: State) => {
        w.__sent.push({ t: performance.now(), op: frame.op });
        return send(frame);
      };
      addEventListener("keydown", (e) => e.key === "y" && (w.__keyAt = performance.now()), true);
    });
    await page.keyboard.press("y");
    const gap = await page.waitForFunction(() => {
      const w = window as unknown as { __sent: { t: number; op: string }[]; __keyAt: number };
      const seen = w.__sent.find((s) => s.op === "seen");
      return seen ? { gap: seen.t - w.__keyAt } : null;
    });
    expect((await gap.jsonValue())!.gap).toBeLessThan(150);
  });

  test("j onto changed blocks and others sends nothing; y on the last changed one sends its seen", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    // Up to the item after the second change; the tick under the last heading is not a block's change
    const last = await stopSp(page, "metrics dashboard");
    const upTo = (await stops(page)).map((s) => s.sp).indexOf(last);
    for (let i = 0; i <= upTo; i++) await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(last);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    // Back onto the last changed block, the one before the metrics item
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["10.0"]]);
    expect(mock.ops().filter((o) => o.op !== "seen")).toEqual([]);
  });

  test("a click into any block sends nothing; y after a click into a changed block checks its stop", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    await clickIn(page, "1:1-1:16");
    await clickIn(page, "10:1-10:16");
    await clickIn(page, "5:1-6:68");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#4", "9.0"]]);
  });

  test("a click on the agent's reply sends nothing; y on its stop sends its id with the turn note's", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    await reply(page, "Yes: CI runs it").click();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#13", "#14"]]);
  });

  // K2, under the ruling that only y acknowledges a not-handled row
  test("a click on a not-handled row sends nothing; y there acknowledges it by its nh id", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    await reply(page, "not handled in").click();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops()).toEqual([expect.objectContaining({ op: "seen", ids: ["nh:5"] })]);
    await expect.poll(() => stripMessage(page)).toBe("ok · Claude's reply");
  });

  test("an acknowledged not-handled row stays open, with no dot", async ({ page, birta }) => {
    await openFixture(page, birta, "unseen", (s) => {
      s.items.find((i: State) => i.n === 5).seen = true;
      check(s, ["nh:5"]);
    });
    await expect.poll(() => hostText(page, "reply", "not handled in")).not.toBeNull();
    expect((await marks(page)).filter((m) => m.mark === "dot")).toEqual([]);
  });

  test("a click on an open blank sends nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await page.locator('birta-flow[data-kind="blank"]', { hasText: "is 503 the only code?" }).click();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  test("stepping onto an open blank sends nothing", async ({ page, birta }) => {
    // Only the blank is left to walk
    const { mock: m2 } = await openFixture(page, birta, "flow", (s) => {
      s.changes = [];
      s.deletions = [];
      for (const i of s.items) {
        if (i.status === "not_handled") i.seen = true;
        for (const r of i.replies) r.seen = true;
      }
      s.counts.unseen = 0;
      s.news = s.news.filter((n: State) => n.id === "#21");
      recount(s);
    });
    // The blank is a stop of its own, and holds the keyboard
    await nToBlank(page, "is 503 the only code?");
    await page.waitForTimeout(SETTLE_MS);
    expect(m2.ops()).toEqual([]);
  });

  test("scrolling past every change sends nothing", async ({ page, birta }) => {
    const doc = fixtureDoc("unseen") + Array.from({ length: 60 }, (_, i) => `\nFiller paragraph ${i + 1}.\n`).join("");
    const { mock } = await open(page, birta, doc, withRev(fixtureState("unseen"), revOf(doc)), {
      viewport: { width: 1280, height: 480 },
    });
    await expect.poll(() => washed(page)).toContain("5:1-6:68");
    // Focus first, so the scroll also moves it, which must send nothing either
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    for (let i = 0; i < 12; i++) {
      await page.mouse.wheel(0, 400);
      await page.waitForTimeout(30);
    }
    await page.keyboard.press("End");
    await page.keyboard.press("Home");
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.waitForTimeout(SETTLE_MS * 2);
    expect(mock.ops()).toEqual([]);
    expect(await washed(page)).toContain("5:1-6:68");
  });
});

// --- The approve guard -----------------------------------------------------------------------

test.describe("approve guard", () => {
  test("the first ⌘⏎ with something to check goes to it instead of approving; a later one approves", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "replied");
    // The button never counts: the guard says what is left
    await expect.poll(() => turnOf(page)).toEqual({ text: "Approve", kbd: true });
    expect(await toCheckCount(page)).toBe(3);
    const key = await hint(page);
    await page.keyboard.press("ControlOrMeta+Enter");
    // Whole doc's reply and turn note come first in walk order; landing there checks nothing
    await expect.poll(() => stripMessage(page)).toBe(`3 to check · n next · ${key} again approves as is`);
    expect(await focused(page)).toBe("doc");
    expect(seenIds(mock)).toEqual([]);
    expect(opsNamed(mock, "approve")).toEqual([]);

    const seen = checkAll(echoSeen(state, "9.0"));
    seen.items.find((i: State) => i.n === 6).replies[0].seen = true;
    swap(mock, state, seen);
    await expect.poll(() => toCheckCount(page)).toBe(0);
    expect(await turnOf(page)).toEqual({ text: "Approve", kbd: true });
    await page.waitForTimeout(DWELL_MS);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
    expect(opFields(opsNamed(mock, "approve")[0])).toEqual(fixtureOp("approve"));
  });

  test("a click on Approve with something to check goes there too", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "replied");
    await expect.poll(() => turnOf(page)).toEqual({ text: "Approve", kbd: true });
    const key = await hint(page);
    await page.locator(".file-header birta-turn button").click();
    await expect.poll(() => stripMessage(page)).toBe(`3 to check · n next · ${key} again approves as is`);
    expect(await focused(page)).toBe("doc");
    expect(seenIds(mock)).toEqual([]);
    expect(opsNamed(mock, "approve")).toEqual([]);
  });

  test("a hand-back is not guarded: with notes waiting and changes unseen, ⌘⏎ hands back", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => (s.counts.drafts = 1));
    await expect.poll(() => turnOf(page).then((t) => t?.text)).toBe("Hand back · 1");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => opsNamed(mock, "hand_back")).toHaveLength(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(seenOps(mock)).toEqual([]);
    expect(await focused(page)).toBe("doc");
  });
});

// --- Receipts --------------------------------------------------------------------------------

/** flow.json's cited receipt sits under the paragraph; its fix of note 6 is Claude's done row under the item. */
const CITED = "because you said";

test.describe("receipt keys", () => {
  test("u on a receipt sends undo_change for its change", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toHaveLength(1);
    const op = opFields(opsNamed(mock, "undo_change")[0]);
    expect(op).toEqual({ ...fixtureOp("undo_change"), change: "13.0" });
    expect(mock.ops().filter((o) => o.op === "comment")).toEqual([]);
  });

  test("u on the done reply under your note reverts the change it names", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "11:1-11:44");
    await receipt(page, DONE).click();
    expect(await hostText(page, "receipt", DONE)).toMatch(/u revert/);
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toHaveLength(1);
    expect(opFields(opsNamed(mock, "undo_change")[0])).toEqual({ ...fixtureOp("undo_change"), change: "15.0" });
  });

  test("y on a receipt sends one seen for its change; the click before it sends none", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(opFields(mock.ops()[0])).toEqual({ ...fixtureOp("seen"), ids: ["13.0"] });
    await expect.poll(() => stripMessage(page)).toBe("kept");
  });

  test("c on a receipt keeps a note on the change", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("c");
    const area = page.locator('birta-flow[data-kind="composer"] textarea');
    await expect(area).toBeFocused();
    await page.keyboard.type("why the cap?");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment")).toHaveLength(1);
    const op = opFields(opsNamed(mock, "comment")[0]);
    expect(Object.keys(op).sort()).toEqual(Object.keys(fixtureOp("comment-change")).sort());
    expect(op).toEqual({ op: "comment", at: { k: "change", id: "13.0" }, body: "why the cap?" });
  });

  test("a change that can be reverted offers u revert; one that can't offers y keep and c note", async ({ page, birta }) => {
    await openFixture(page, birta, "flow", (s) => {
      s.changes.find((c: State) => c.id === "13.0").reversible = { ok: false, reason: "later_overlap" };
    });
    await focusOn(page, "5:1-6:68");
    await expect.poll(() => hostText(page, "receipt", CITED)).toMatch(/· y keep · c note$/);
    expect(await hostText(page, "receipt", CITED)).not.toContain("u revert");
    // Control: the other change still can be
    await focusOn(page, "11:1-11:44");
    await expect.poll(() => hostText(page, "receipt", DONE)).toMatch(/y keep · u revert · c note/);
  });

  test("the done reply follows its change's revert too", async ({ page, birta }) => {
    await openFixture(page, birta, "flow", (s) => {
      s.changes.find((c: State) => c.id === "15.0").reversible = { ok: false, reason: "snap" };
    });
    await focusOn(page, "11:1-11:44");
    await expect.poll(() => hostText(page, "receipt", DONE)).toMatch(/· y keep · c note$/);
  });

  test("u on a change that can't be undone sends nothing and says so", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      s.changes.find((c: State) => c.id === "13.0").reversible = { ok: false, reason: "later_overlap" };
    });
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("can't revert this one: a later change overlaps · c to ask Claude");
    expect(opsNamed(mock, "undo_change")).toEqual([]);
    expect(opsNamed(mock, "comment")).toEqual([]);
  });

  test("on a doc birta can't write, no row offers u revert", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => (s.writable = false));
    await focusOn(page, "11:1-11:44");
    await expect.poll(() => hostText(page, "receipt", DONE)).toMatch(/· y keep · c note$/);
    await focusOn(page, "5:1-6:68");
    await expect.poll(() => hostText(page, "receipt", CITED)).toMatch(/· y keep · c note$/);
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("can't undo this one · c instead");
    expect(opsNamed(mock, "undo_change")).toEqual([]);
  });

  test("an undo refused because the doc moved on says try again", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", undefined, { acks: "manual" });
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toHaveLength(1);
    mock.nack(opsNamed(mock, "undo_change")[0].cid, "changed_since");
    await expect.poll(() => stripMessage(page)).toBe("changed since · try again");
  });

  test("an undo refused for a read-only review file says so", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", undefined, { acks: "manual" });
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toHaveLength(1);
    mock.nack(opsNamed(mock, "undo_change")[0].cid, "read_only");
    await expect.poll(() => stripMessage(page)).toBe("review file is read-only");
  });

  test("n onto a change makes u its revert; j onto the block opens its row too, where x asks before no", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await nTo(page, "5:1-6:68");
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([expect.objectContaining({ change: "13.0" })]);
    expect(mock.ops().filter((o) => o.op === "comment")).toEqual([]);

    await page.reload();
    await booted(page);
    await focusOn(page, "5:1-6:68");
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe("x = no to this text · u reverts Claude's change · x again for no");
    await page.keyboard.press("x");
    await expect
      .poll(() => opsNamed(mock, "comment"))
      .toEqual([expect.objectContaining({ verdict: "no", at: { k: "block", sp: "5:1-6:68", scope: "block" } })]);
    expect(opsNamed(mock, "undo_change")).toHaveLength(1);
  });
});

/** The sp of the block a flow host of `kind` holding `has` belongs to: before it, or around it in an item. */
function slotOf(page: Page, kind: string, has: string): Promise<string | null> {
  return page.evaluate(
    ({ kind, has }) => {
      for (const h of document.querySelectorAll(`birta-flow[data-kind="${kind}"]`)) {
        if (!(h.shadowRoot?.textContent ?? "").includes(has)) continue;
        if (h.parentElement!.localName === "li") return h.parentElement!.getAttribute("data-sourcepos");
        let prev = h.previousElementSibling;
        while (prev && prev.localName === "birta-flow") prev = prev.previousElementSibling;
        return prev?.getAttribute("data-sourcepos") ?? null;
      }
      return null;
    },
    { kind, has },
  );
}

const fact = (page: Page, has: string) => page.locator('birta-flow[data-kind="fact"]', { hasText: has });

test.describe("a receipt for every change", () => {
  test("an unasked change opens a not-asked-for row under its block, with its why and the keys", async ({ page, birta }) => {
    await openFixture(page, birta, "flow");
    await focusOn(page, "14:1-14:21");
    await expect
      .poll(async () => (await changeRows(page)).map((r) => [r.lead, r.sp, r.kbd]))
      .toEqual([["Claude · not asked for: while I was there", "14:1-14:21", ["y", "u", "c"]]]);
    expect(await hostText(page, "receipt", "not asked for")).toMatch(/· y keep · u revert · c note$/);
    expect(await hostText(page, "receipt", "not asked for")).not.toMatch(SIZE_LABEL);
    expect(await slotOf(page, "receipt", "not asked for")).toBe("14:1-14:21");
  });

  test("u, y and c on a not-asked-for row revert, see and note its change", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "14:1-14:21");
    const row = receipt(page, "not asked for");
    await row.click();
    await page.waitForTimeout(SETTLE_MS);
    expect(seenOps(mock)).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["19.0"]]);
    await row.click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([expect.objectContaining({ change: "19.0" })]);
    await row.click();
    await page.keyboard.press("c");
    await page.keyboard.type("why?");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment")).toEqual([expect.objectContaining({ at: { k: "change", id: "19.0" }, body: "why?" })]);
  });

  test("a removed block's change opens its row at its removal stop, in the gap under the block above", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    for (let i = 0; i < 12 && (await slotOf(page, "receipt", "removed:")) === null; i++) await page.keyboard.press("n");
    await expect.poll(() => slotOf(page, "receipt", "removed:")).toBe("14:1-14:13");
    expect(await focusMark(page)).toMatchObject({ target: "removal", change: "11.0", after: "14:1-14:13", sp: null });
    expect(await hostText(page, "receipt", "removed:")).toMatch(/removed: Nothing yet\. · y keep · u revert · c note$/);
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([expect.objectContaining({ change: "11.0" })]);
  });

  test("n onto an unasked change makes u its revert", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    await nTo(page, "11:1-11:44");
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([expect.objectContaining({ change: "10.0" })]);
    expect(opsNamed(mock, "comment")).toEqual([]);
  });

  test("an outside write keeps its row and gains the keys, with x only when it can be undone", async ({ page, birta }) => {
    await openFixture(page, birta, "flow");
    await focusOn(page, "8:1-8:15");
    await expect.poll(() => hostText(page, "fact", "(git)")).toMatch(/^changed outside birta \(git\)[\s\S]* · y keep · c note$/);
    expect(await hostText(page, "fact", "(git)")).not.toMatch(SIZE_LABEL);
    expect(await slotOf(page, "fact", "(git)")).toBe("8:1-8:15");
    await focusOn(page, "16:1-16:19");
    await expect.poll(() => hostText(page, "fact", "changed outside")).not.toBeNull();
    const plain = await page.evaluate(() =>
      Array.from(document.querySelectorAll('birta-flow[data-kind="fact"]'))
        // The card's own text: the root also holds the host's style
        .map((h) => (h.shadowRoot?.querySelector(".flow")?.textContent ?? "").replace(/\s+/g, " ").trim())
        .filter((t) => !t.includes("(git)")),
    );
    expect(plain).toHaveLength(1);
    expect(plain[0]).toMatch(/^changed outside birta\b[\s\S]*y keep · c note$/);
    expect(plain[0]).not.toMatch(SIZE_LABEL);
  });

  test("u on an outside write that can't be undone says so; y sees it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "8:1-8:15");
    await fact(page, "(git)").click();
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("can't undo this one · c instead");
    expect(opsNamed(mock, "undo_change")).toEqual([]);
    await fact(page, "(git)").click();
    await page.waitForTimeout(SETTLE_MS);
    expect(seenOps(mock)).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["26.0"]]);
  });

  test("an outside write the fold can reverse offers x undo, and u sends it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      s.changes.find((c: State) => c.id === "26.0").reversible = { ok: true, reason: "none" };
    });
    await focusOn(page, "8:1-8:15");
    await expect.poll(() => hostText(page, "fact", "(git)")).toMatch(/y keep · u revert · c note$/);
    await fact(page, "(git)").click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([expect.objectContaining({ change: "26.0" })]);
  });
});

test.describe("undo of a many-hunk write", () => {
  const twoHunks = (s: State) => {
    const c = s.changes.find((x: State) => x.id === "13.0");
    c.op_hunks = 2;
    c.reversible = { ok: true, reason: "none" };
  };

  test("one u reverts a change of a many-hunk write, with no confirm and no question", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", twoHunks);
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toHaveLength(1);
    expect(opFields(opsNamed(mock, "undo_change")[0])).toEqual({ ...fixtureOp("undo_change"), change: "13.0" });
    expect((await stripMessage(page)) ?? "").not.toMatch(/again/);
  });
});

// --- The beacon ------------------------------------------------------------------------------

/** Records every `sendBeacon` call's URL and body text, then sends it as the page asked. */
const BEACON_SPY = `(() => {
  const send = navigator.sendBeacon.bind(navigator);
  window.__beacons = [];
  navigator.sendBeacon = function (url, data) {
    const rec = { url: String(url), body: null };
    window.__beacons.push(rec);
    new Response(data).text().then((t) => (rec.body = t));
    return send(url, data);
  };
})();`;

interface Beacon {
  url: string;
  body: State;
}

async function beacons(page: Page): Promise<Beacon[]> {
  const all = await page.evaluate(() => (window as unknown as { __beacons: { url: string; body: string | null }[] }).__beacons);
  return all.filter((b) => b.body !== null).map((b) => ({ url: b.url, body: JSON.parse(b.body!) }));
}

function hide(page: Page) {
  return page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
  });
}

/** Keeps a whole-doc note from the doc stop. */
async function noteOnDoc(page: Page, body: string) {
  await focusTop(page);
  await page.keyboard.press("c");
  await page.keyboard.type(body);
  await page.keyboard.press("Enter");
}

test.describe("beacon", () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(BEACON_SPY);
  });

  test("hiding the page sends the unacked seen and notes to /review/flush, and never the hand-back", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => (s.counts.drafts = 1), { acks: "manual" });
    await noteOnDoc(page, "overall fine");
    // Focus is on whole doc, whose reply and turn note one y checks
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#13", "#14"]]);
    await clickIn(page, "1:1-1:16");
    // The hand-back waits for the note's ack, which never comes here, so it is never sent at all
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().map((o) => o.op)).toEqual(["comment", "seen"]);
    await hide(page);
    await expect.poll(async () => (await beacons(page)).length).toBeGreaterThan(0);
    const all = await beacons(page);
    const { body, url } = all[all.length - 1];
    expect(new URL(url, page.url()).pathname).toBe("/review/flush");
    expect(typeof body.token === "string" && body.token.length > 0).toBe(true);
    expect(body.token).toBe(await page.evaluate(() => (window as unknown as { BOOT: State }).BOOT.review.token));
    expect(body.tab).toBe(mock.frames.find((f) => f.msg.type === "hello")!.msg.tab);
    expect(body.path).toBe("pr.md");
    expect(body.seen).toContain("#13");
    const note = opsNamed(mock, "comment")[0];
    expect(body.ops).toContainEqual(expect.objectContaining({ type: "op", cid: note.cid, op: "comment", body: "overall fine" }));
    for (const b of all) {
      expect(JSON.stringify(b.body.ops.map((o: State) => o.op))).not.toMatch(/hand_back|approve/);
    }
  });

  test("an unacked approve never rides the beacon", async ({ page, birta }) => {
    const seen = checkAll(echoSeen(fixtureState("replied"), "9.0"));
    seen.items.find((i: State) => i.n === 6).replies[0].seen = true;
    const { mock } = await open(page, birta, fixtureDoc("replied"), seen, { acks: "manual" });
    await expect.poll(() => turnOf(page).then((t) => t?.text)).toBe("Approve");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
    // A note kept after the approve: it rides the beacon, the approve doesn't
    await noteOnDoc(page, "one more thing");
    await hide(page);
    await expect.poll(async () => (await beacons(page)).length).toBeGreaterThan(0);
    for (const b of await beacons(page)) {
      expect(b.body.ops.map((o: State) => o.op)).not.toContain("approve");
      expect(b.body.ops.map((o: State) => o.op)).not.toContain("hand_back");
    }
    expect((await beacons(page)).flatMap((b) => b.body.ops.map((o: State) => o.body))).toContain("one more thing");
  });

  test("pagehide sends the beacon too", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", undefined, { acks: "manual" });
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#13", "#14"]]);
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    await expect.poll(async () => (await beacons(page)).flatMap((b) => b.body.seen)).toContain("#13");
  });

  test("what the server has acked stays out of the beacon", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    await noteOnDoc(page, "already kept");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#13", "#14"]]);
    await page.waitForTimeout(SETTLE_MS);
    await hide(page);
    await page.waitForTimeout(SETTLE_MS);
    for (const b of await beacons(page)) {
      expect(b.body.seen ?? []).not.toContain("#13");
      expect(b.body.ops.map((o: State) => o.body)).not.toContain("already kept");
    }
  });

  // K16: the mock swallows ops, so only the beacon can deliver this note
  test("the server applies the beacon: an unacked note lands in the review file", async ({ page, birta }) => {
    const { served, mock } = await open(page, birta, fixtureDoc("fresh"), fixtureState("fresh"), { acks: "manual" });
    await noteOnDoc(page, "only the beacon carries this");
    await expect.poll(() => opsNamed(mock, "comment")).toHaveLength(1);
    expect(reviewLog(served.root)).toBeNull();
    await hide(page);
    await expect
      .poll(
        () => reviewLog(served.root)?.filter((l) => l.op === "comment" && l.body === "only the beacon carries this").length ?? 0,
        { timeout: 5_000 },
      )
      .toBe(1);
  });

  // K18
  test("one hide sends one beacon, even after the socket reconnects", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", undefined, { acks: "manual" });
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#13", "#14"]]);
    await mock.drop();
    await expect.poll(() => new Set(mock.frames.map((f) => f.socket)).size, { timeout: 10_000 }).toBe(2);
    await hide(page);
    await page.waitForTimeout(SETTLE_MS);
    expect(await beacons(page)).toHaveLength(1);
  });

  // K19
  test("an unacked undo never rides the beacon", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", undefined, { acks: "manual" });
    await noteOnDoc(page, "carry me");
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toHaveLength(1);
    await hide(page);
    await expect.poll(async () => (await beacons(page)).length).toBeGreaterThan(0);
    for (const b of await beacons(page)) expect(b.body.ops.map((o: State) => o.op)).not.toContain("undo_change");
  });

  // K20
  test("a seen the server never acked survives a reload", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", undefined, { acks: "manual" });
    const flushed: string[] = [];
    page.on("request", (r) => r.url().endsWith("/review/flush") && flushed.push(r.postData() ?? ""));
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#13", "#14"]]);
    await page.reload();
    await booted(page);
    const replayed = () => mock.frames.filter((f) => f.socket > 0 && f.msg.op === "seen").flatMap((f) => f.msg.ids);
    await expect.poll(() => replayed().includes("#13") || flushed.some((b) => b.includes('"#13"'))).toBe(true);
  });
});

// --- Real server -----------------------------------------------------------------------------

const ORIG_LINES = [
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
  "- metrics dashboard",
  "",
];
/** The agent's fix of the note, and an unasked change further down; 1-based lines. */
const FIX = { line: 6, to: "S3 answers 503 under load; retrying three times fixes most of these." };
const UNASKED = { line: 11, to: "- metrics dashboard, per region" };

interface Seeded {
  /** The text before the agent's turn, which an undo must give back byte for byte. */
  orig: string;
  /** The text on disk, after the agent's turn. */
  doc: string;
  log: string;
}

/**
 * A review file where you noted "we just fail", handed it back, and the agent claimed it,
 * rewrote the lines in `edits` in one write, said done, and yielded.
 */
function seeded(eol: string, edits: { line: number; to: string }[]): Seeded {
  const orig = ORIG_LINES.join(eol);
  const after = [...ORIG_LINES];
  for (const e of edits) after[e.line - 1] = e.to;
  const doc = after.join(eol);
  const T0 = Date.now() - 600_000;
  const human = { type: "human", via: "browser" };
  const agent = { type: "agent", name: "claude", via: "cli" };
  const lines: State[] = [
    { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
    { by: { type: "birta" }, op: "snap", rev: revOf(orig), text: orig },
    { by: { type: "birta" }, op: "owner", drafted_by: null },
    {
      by: human,
      cid: "c-4",
      op: "comment",
      at: { quote: "we just fail", pre: "", suf: "", lines: [5, 5], rev: revOf(orig), scope: "quote" },
      body: "say why it fails",
    },
    { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
    { by: agent, op: "claim", round: 5 },
    {
      by: { type: "birta" },
      op: "external",
      rev: revOf(orig),
      to_rev: revOf(doc),
      patch: edits.map((e) => ({ a: e.line - 1, d: [ORIG_LINES[e.line - 1] + eol], i: [e.to + eol] })),
    },
    { by: agent, op: "done", to: 4, turn: 6 },
    { by: agent, op: "yield", turn: 6 },
  ];
  const log = lines.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join("");
  return { orig, doc, log };
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

const lastState = (got: State[]) => got.filter((f) => f.type === "review_state" && f.path === "pr.md").at(-1)?.state ?? null;

/** Serves a seeded review with the real socket and checks the server folded it as the spec expects. */
async function openSeeded(page: Page, birta: Start, s: Seeded, changes: string[]) {
  const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
  const got = watchReceived(page);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  // Setup: what the server says, before anything the page does
  await expect.poll(() => lastState(got)?.changes.map((c: State) => c.id)).toEqual(changes);
  const state = lastState(got);
  expect(state.changes[0]).toMatchObject({ reversible: { ok: true }, op_hunks: changes.length, seen: false });
  expect(state.items.find((i: State) => i.n === 4).outcome).toMatchObject({ done: true, changes: ["7.0"] });
  const file = path.join(served.root, "pr.md");
  return { served, got, file, bytes: () => fs.readFileSync(file) };
}

test.describe("real server", () => {
  for (const [name, eol] of [
    ["LF", "\n"],
    ["CRLF", "\r\n"],
  ] as const) {
    test(`u on the agent's fix puts the doc's bytes back, and a ${name} doc keeps its line ends`, async ({ page, birta }) => {
      const s = seeded(eol, [FIX]);
      const { served, bytes } = await openSeeded(page, birta, s, ["7.0"]);
      expect(bytes().equals(Buffer.from(s.doc))).toBe(true);
      await clickDone(page);
      await page.keyboard.press("u");
      await expect.poll(() => bytes().equals(Buffer.from(s.orig)), { timeout: 5_000 }).toBe(true);
      if (eol === "\r\n") expect(/(^|[^\r])\n/.test(bytes().toString("utf-8"))).toBe(false);
      await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "edit", act: "undo", of: "7.0" });
      // The watcher sees its own write and records nothing more
      await page.waitForTimeout(SETTLE_MS * 2);
      const log = reviewLog(served.root)!;
      const undo = log.findIndex((l) => l.op === "edit" && l.act === "undo");
      expect(log.slice(undo + 1).filter((l) => l.op === "external")).toEqual([]);
    });
  }

  test("u on one change of a two-hunk write puts back only that change", async ({ page, birta }) => {
    const s = seeded("\r\n", [FIX, UNASKED]);
    const { served, bytes } = await openSeeded(page, birta, s, ["7.0", "7.1"]);
    await clickDone(page);
    await page.keyboard.press("u");
    const unasked = seeded("\r\n", [UNASKED]).doc;
    await expect.poll(() => bytes().equals(Buffer.from(unasked)), { timeout: 5_000 }).toBe(true);
    await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "edit", act: "undo", of: "7.0" });
  });

  test("an undo a read-only review file refuses says so, and leaves the doc", async ({ page, birta }) => {
    const s = seeded("\n", [FIX]);
    const { served, bytes } = await openSeeded(page, birta, s, ["7.0"]);
    const logFile = path.join(served.root, "pr.md.birta.jsonl");
    await clickDone(page);
    // Whatever the click wrote lands before the file turns read-only
    await page.waitForTimeout(SETTLE_MS);
    const lines = reviewLog(served.root)!.length;
    expect(await stripMessage(page)).not.toBe("review file is read-only");
    fs.chmodSync(logFile, 0o444);
    try {
      await page.keyboard.press("u");
      await expect.poll(() => stripMessage(page)).toBe("review file is read-only");
      await page.waitForTimeout(SETTLE_MS);
      expect(bytes().equals(Buffer.from(s.doc))).toBe(true);
      expect(reviewLog(served.root)!).toHaveLength(lines);
    } finally {
      fs.chmodSync(logFile, 0o644);
    }
  });

  test("n sends no seen; y does, and the server's next state counts one fewer unseen, one fewer to check, and drops the wash", async ({
    page,
    birta,
  }) => {
    const s = seeded("\n", [FIX, UNASKED]);
    const { served, got } = await openSeeded(page, birta, s, ["7.0", "7.1"]);
    const fix = await stopSp(page, "S3 answers 503");
    const unasked = await stopSp(page, "per region");
    expect(lastState(got).counts.unseen).toBe(2);
    expect(lastState(got).counts.to_check).toBe(2);
    await expect.poll(() => washed(page)).toEqual([fix, unasked].sort());
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe(fix);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)!.filter((l) => l.op === "seen")).toEqual([]);
    // The fix and the done on your note share the paragraph: one y checks both
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.at(-1)?.op).toBe("seen");
    expect([...reviewLog(served.root)!.at(-1)!.ids].sort()).toEqual(["#4", "7.0"]);
    await expect.poll(() => lastState(got)?.counts.unseen).toBe(1);
    await expect.poll(() => lastState(got)?.counts.to_check).toBe(1);
    expect(lastState(got).changes.find((c: State) => c.id === "7.0").seen).toBe(true);
    await expect.poll(() => washed(page)).toEqual([unasked]);
  });

  test("on a real server ⌘⏎ goes to the first stop to check, and a later one approves with both still unchecked", async ({
    page,
    birta,
  }) => {
    const s = seeded("\n", [FIX, UNASKED]);
    const { served } = await openSeeded(page, birta, s, ["7.0", "7.1"]);
    await expect.poll(() => turnOf(page)).toEqual({ text: "Approve", kbd: true });
    await expect.poll(() => toCheckCount(page)).toBe(2);
    const key = await hint(page);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => stripMessage(page)).toBe(`2 to check · n next · ${key} again approves as is`);
    expect(await focused(page)).toBe(await stopSp(page, "S3 answers 503"));
    // Within the dwell a second press is a reflex: it neither steps on nor approves
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.waitForTimeout(SETTLE_MS);
    // The jump checks nothing
    expect(reviewLog(served.root)!.filter((l) => l.op === "seen")).toEqual([]);
    expect(await turnOf(page)).toEqual({ text: "Approve", kbd: true });
    expect(await toCheckCount(page)).toBe(2);
    expect(reviewLog(served.root)!.some((l) => l.op === "approve")).toBe(false);
    await page.waitForTimeout(DWELL_MS);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => reviewLog(served.root)!.some((l) => l.op === "approve")).toBe(true);
  });
});

// --- Killers ---------------------------------------------------------------------------------

/** flow.json with 13.0 as one hunk of a two-hunk write. */
function twoHunks(s: State) {
  const c = s.changes.find((x: State) => x.id === "13.0");
  c.op_hunks = 2;
  c.reversible = { ok: true, reason: "none" };
}

test.describe("killers: the walk and seen", () => {
  // K4
  test("n after the server's echo of a y goes on to the next stop, not past it", async ({ page, birta }) => {
    const { mock, state } = await openFixture(page, birta, "unseen");
    await nTo(page, "5:1-6:68");
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["#4", "9.0"]]);
    const before = state.counts.to_check;
    swap(mock, state, echoSeen(state, "9.0", "#4"));
    expect(state.counts.to_check).toBe(before - 1);
    await expect.poll(() => toCheckCount(page)).toBe(state.counts.to_check);
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("10:1-10:16");
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([["#4", "9.0"]]);
  });

  // K12
  test("j or a click onto a change the state marks kept sends nothing, and y there says already kept", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => {
      s.changes.find((c: State) => c.id === "9.0").seen = true;
      s.counts.unseen -= 1;
      check(s, ["9.0", "#4"]);
    });
    await focusOn(page, await stopSp(page, "metrics dashboard"));
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
    await clickIn(page, "5:1-6:68");
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe("already kept · u reverts it");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  // K13
  test("hovering changed blocks and replies sends nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    for (const sp of ["5:1-6:68", "11:1-11:44"]) {
      const box = (await page.locator(`#content [data-sourcepos="${sp}"]`).last().boundingBox())!;
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 4 });
      await page.waitForTimeout(150);
    }
    await reply(page, "Yes: CI runs it").hover();
    await reply(page, "not handled in").hover();
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
  });

  // K14: landing checks nothing, so the y after it is what must send at once
  for (const how of ["click", "j"] as const) {
    test(`seen goes out at once on the y after a ${how}`, async ({ page, birta }) => {
      await openFixture(page, birta, "unseen");
      await page.evaluate(() => {
        const w = window as unknown as { birtaBus: { send(f: State): boolean }; __sent: { t: number; op: string }[]; __at: number };
        w.__sent = [];
        const send = w.birtaBus.send.bind(w.birtaBus);
        w.birtaBus.send = (frame: State) => {
          w.__sent.push({ t: performance.now(), op: frame.op });
          return send(frame);
        };
        for (const ev of ["pointerdown", "keydown"]) addEventListener(ev, () => (w.__at = performance.now()), true);
      });
      if (how === "click") await clickIn(page, "5:1-6:68");
      else await focusOn(page, "5:1-6:68");
      await page.waitForTimeout(SETTLE_MS);
      expect(await page.evaluate(() => (window as unknown as { __sent: { op: string }[] }).__sent.filter((s) => s.op === "seen"))).toEqual([]);
      await page.keyboard.press("y");
      const gap = await page.waitForFunction(() => {
        const w = window as unknown as { __sent: { t: number; op: string }[]; __at: number };
        const seen = w.__sent.find((s) => s.op === "seen");
        return seen ? { gap: seen.t - w.__at } : null;
      });
      expect((await gap.jsonValue())!.gap).toBeLessThan(150);
    });
  }

  // K25
  test("p from a removal walks back into the change above it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen");
    for (let i = 0; i < 5; i++) await page.keyboard.press("n");
    // 11.0 removed a block, so it walks at its own stop, in the gap after the block above
    await expect.poll(() => focusMark(page)).toMatchObject({ target: "removal", change: "11.0", after: "14:1-14:13", sp: null });
    expect(seenIds(mock)).toEqual([]);
    await page.keyboard.press("p");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
  });

  // K26
  test("n with nothing new sends nothing and leaves focus alone", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "fresh");
    await page.keyboard.press("n");
    await page.keyboard.press("p");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBe("doc");
  });

  // K27
  test("j onto a block with two changes checks neither; one y there checks both in one seen", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "lost");
    await focusOn(page, "5:1-6:68");
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock)).toEqual([["14.0", "9.0"]]);
  });
});

test.describe("killers: the approve guard", () => {
  // K5
  test("a real open blank never blocks approve: the first ⌘⏎ goes to it, a later one approves", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      for (const c of s.changes) c.seen = true;
      for (const i of s.items) for (const r of i.replies ?? []) r.seen = true;
      s.counts.unseen = 0;
      s.counts.drafts = 0;
      // Everything else checked: the open question is all there is to check
      check(s, s.news.map((n: State) => n.id).filter((id: string) => id !== "#21"));
    });
    await expect.poll(() => turnOf(page)).toEqual({ text: "Approve", kbd: true });
    expect(await toCheckCount(page)).toBe(1);
    const key = await hint(page);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => blankAt(page)).toBe("is 503 the only code?");
    expect(await stripMessage(page)).toBe(`1 to check · n next · ${key} again approves as is`);
    await page.waitForTimeout(DWELL_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => opsNamed(mock, "approve")).toHaveLength(1);
    expect(seenOps(mock)).toEqual([]);
  });

  // K6: the jump sends no seen, so nothing the server echoes can count as checked
  test("⌘⏎ twice inside the dwell never approves, and the jump checks nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "replied");
    const key = await hint(page);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => stripMessage(page)).toBe(`3 to check · n next · ${key} again approves as is`);
    await page.keyboard.press("ControlOrMeta+Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "approve")).toEqual([]);
    expect(seenIds(mock)).toEqual([]);
    expect(await focused(page)).toBe("doc");
  });

  // K7
  test("the guard skips stops the state already marks checked", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => {
      s.changes.find((c: State) => c.id === "9.0").seen = true;
      s.items.find((i: State) => i.n === 6).replies[0].seen = true;
      s.counts.unseen -= 1;
      // Whole doc, the paragraph and the not-handled bullet are checked; 10.0 and the removal are left
      check(s, ["#13", "#14", "9.0", "#4", "nh:5"]);
    });
    const key = await hint(page);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
    expect(await stripMessage(page)).toBe(`2 to check · n next · ${key} again approves as is`);
    await page.waitForTimeout(SETTLE_MS);
    expect(seenIds(mock)).toEqual([]);
  });
});

test.describe("killers: receipts and undo", () => {
  // K1b
  test("an unasked change has a receipt, and u there reverts it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "14:1-14:21");
    const row = receipt(page, "not asked for: while I was there");
    await expect(row).toHaveCount(1);
    expect(await hostText(page, "receipt", "not asked for: while I was there")).toMatch(/y keep · u revert · c note/);
    await row.click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toHaveLength(1);
    expect(opFields(opsNamed(mock, "undo_change")[0])).toEqual({ ...fixtureOp("undo_change"), change: "19.0" });
  });

  // K1c, on 5.4's outside row, which keeps its kind
  test("an outside write's row can't be undone: u says so, y marks it seen", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "8:1-8:15");
    const row = page.locator('birta-flow[data-kind="fact"]', { hasText: "changed outside birta (git)" });
    await expect(row).toHaveCount(1);
    expect(await hostText(page, "fact", "changed outside birta (git)")).toMatch(/· y keep · c note$/);
    await row.click();
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("can't undo this one · c instead");
    expect(opsNamed(mock, "undo_change")).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => seenIds(mock).at(-1)).toEqual(["26.0"]);
  });

  test("a change your note asked for, with no done reply, has a because-you-said receipt under its block", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      delete s.items.find((i: State) => i.n === 6).outcome;
    });
    await focusOn(page, "11:1-11:44");
    await expect
      .poll(async () => (await changeRows(page)).filter((r) => r.sp === "11:1-11:44").map((r) => [r.lead, r.kbd]))
      .toEqual([['Claude · because you said "cap it"', ["y", "u", "c"]]]);
    expect(await hostText(page, "receipt", 'because you said "cap it"')).toMatch(/· y keep · u revert · c note$/);
    expect(await hostText(page, "receipt", 'because you said "cap it"')).not.toMatch(SIZE_LABEL);
    expect(await slotOf(page, "receipt", 'because you said "cap it"')).toBe("11:1-11:44");
    await receipt(page, 'because you said "cap it"').click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([expect.objectContaining({ change: "15.0" })]);
  });

  // K28
  test("c on the done reply keeps a note on its change, not a reply to your note", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "11:1-11:44");
    await receipt(page, DONE).click();
    await page.keyboard.press("c");
    await page.keyboard.type("why one line?");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment")).toHaveLength(1);
    expect(opFields(opsNamed(mock, "comment")[0])).toEqual({ op: "comment", at: { k: "change", id: "15.0" }, body: "why one line?" });
    expect(opsNamed(mock, "reply")).toEqual([]);
  });

  // K9: each change reverts on its own u
  test("u on one change and then another reverts each, once", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toHaveLength(1);
    await focusOn(page, "11:1-11:44");
    await receipt(page, DONE).click();
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change").map((o) => o.change)).toEqual(["13.0", "15.0"]);
  });

  // K10
  test("holding u down reverts once and never flips the view, and ⌘X or ⌘U on a row is no revert", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow");
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    // Playwright marks every down after the first as a repeat
    await page.keyboard.down("u");
    await page.keyboard.down("u");
    await page.keyboard.down("u");
    await page.keyboard.up("u");
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "undo_change")).toHaveLength(1);
    expect(await page.locator("#source-view").isVisible()).toBe(false);
    await focusOn(page, "11:1-11:44");
    await receipt(page, DONE).click();
    await page.keyboard.press("ControlOrMeta+x");
    await page.keyboard.press("ControlOrMeta+u");
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "undo_change")).toHaveLength(1);
  });

  // K11
  test("u twice before the ack sends one undo", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", undefined, { acks: "manual" });
    await focusOn(page, "5:1-6:68");
    await receipt(page, CITED).click();
    await page.keyboard.press("u");
    await page.keyboard.press("u");
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "undo_change")).toHaveLength(1);
  });
});

/** A review file where your `no` on "metrics dashboard" went out and the agent's turn left it alone. */
function seededNotHandled(): { doc: string; log: string } {
  const doc = ORIG_LINES.join("\n");
  const T0 = Date.now() - 600_000;
  const human = { type: "human", via: "browser" };
  const agent = { type: "agent", name: "claude", via: "cli" };
  const lines: State[] = [
    { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
    { by: { type: "birta" }, op: "snap", rev: revOf(doc), text: doc },
    { by: { type: "birta" }, op: "owner", drafted_by: null },
    {
      by: human,
      cid: "c-4",
      op: "comment",
      verdict: "no",
      at: { quote: "metrics dashboard", pre: "", suf: "", lines: [11, 11], rev: revOf(doc), scope: "quote" },
    },
    { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
    { by: agent, op: "claim", round: 5 },
    { by: agent, op: "yield", turn: 6 },
  ];
  return { doc, log: lines.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join("") };
}

test.describe("killers: real server", () => {
  // K1: the count, the flag the walk reads and the tab's news agree once the server acks
  test("y on a not-handled row: the server counts it seen and flags it; n still walks the round, and it stays seen", async ({
    page,
    birta,
  }) => {
    const s = seededNotHandled();
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    const got = watchReceived(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => lastState(got)?.items.find((i: State) => i.n === 4)?.status).toBe("not_handled");
    expect(lastState(got).counts).toMatchObject({ attention: 1, to_check: 1 });
    await expect(page).toHaveTitle(/^● /);
    await reply(page, "not handled in").click();
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "seen", ids: ["nh:4"] });
    await expect.poll(() => lastState(got)?.counts.to_check).toBe(0);
    expect(lastState(got).counts.attention).toBe(0);
    const item = lastState(got).items.find((i: State) => i.n === 4);
    expect(item.seen).toBe(true);
    expect(item.status).toBe("not_handled");
    await expect(page).not.toHaveTitle(/^● /);
    expect(await page.evaluate(() => {
      const w = window as unknown as { BR: { pure: { attention(s: State): unknown[] } ; currentState(): State } };
      return w.BR.pure.attention(w.BR.currentState()).length;
    })).toBe(0);
    await page.reload();
    await booted(page);
    await expect(page).not.toHaveTitle(/^● /);
    const before = reviewLog(served.root)!.length;
    // Seen is not settled: until a hand-back the row is still a stop of this round
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe(await stopSp(page, "metrics dashboard"));
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)!.slice(before)).toEqual([]);
    expect(lastState(got).items.find((i: State) => i.n === 4).seen).toBe(true);
    await expect(page).not.toHaveTitle(/^● /);
  });

  // K21
  test("u still reverts the fix after an outside edit above it", async ({ page, birta }) => {
    const s = seeded("\n", [FIX]);
    const { served, got, bytes } = await openSeeded(page, birta, s, ["7.0"]);
    const preface = "> draft\n\n";
    served.write("pr.md", preface + s.doc);
    await expect.poll(() => lastState(got)?.changes.find((c: State) => c.id === "7.0")?.range.lines).toEqual([8, 8]);
    await expect(page.locator("#content blockquote")).toHaveCount(1);
    await clickDone(page);
    await page.keyboard.press("u");
    await expect.poll(() => bytes().toString("utf-8"), { timeout: 5_000 }).toBe(preface + s.orig);
  });

  // K22
  test("an edit over the fix takes x undo away, and u then writes nothing", async ({ page, birta }) => {
    const s = seeded("\n", [FIX]);
    const { served, got, bytes } = await openSeeded(page, birta, s, ["7.0"]);
    const over = s.doc.replace(FIX.to, FIX.to + " Mostly.");
    served.write("pr.md", over);
    await expect.poll(() => lastState(got)?.changes.find((c: State) => c.id === "7.0")?.reversible.ok).toBe(false);
    await clickDone(page);
    await expect.poll(() => hostText(page, "receipt", DONE)).not.toMatch(/u revert/);
    expect(lastState(got).changes.find((c: State) => c.id === "7.0").reversible.reason).toBe("later_overlap");
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("can't revert this one: a later change overlaps · c to ask Claude");
    expect(bytes().toString("utf-8")).toBe(over);
    expect(reviewLog(served.root)!.filter((l) => l.op === "edit")).toEqual([]);
  });

  // K23: content is held back so the page's rev goes stale, while ops still reach the server
  test("an undo the server finds stale says changed since · try again, and keeps the outside bytes", async ({ page, birta }) => {
    const s = seeded("\n", [FIX]);
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    let hold = false;
    await page.routeWebSocket(/\/ws$/, (ws) => {
      const server = ws.connectToServer();
      server.onMessage((raw) => {
        const t = JSON.parse(raw.toString()).type;
        if (hold && ["content", "review_state", "theme_update"].includes(t)) return;
        ws.send(raw);
      });
    });
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await clickDone(page);
    await page.waitForTimeout(SETTLE_MS);
    hold = true;
    const outside = s.doc + "\nA line from elsewhere.\n";
    served.write("pr.md", outside);
    await page.waitForTimeout(SETTLE_MS * 2);
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe("changed since · try again");
    expect(fs.readFileSync(path.join(served.root, "pr.md"), "utf-8")).toBe(outside);
  });
});

test.describe("the walk and the tab agree", () => {
  /** Your own ask the turn left unanswered, the agent's open question, and its unasked fix. */
  function seededMix(): { doc: string; log: string } {
    const orig = ORIG_LINES.join("\n");
    const lines = [...ORIG_LINES];
    lines[FIX.line - 1] = FIX.to;
    const doc = lines.join("\n");
    const T0 = Date.now() - 600_000;
    const human = { type: "human", via: "browser" };
    const agent = { type: "agent", name: "claude", via: "cli" };
    const ops: State[] = [
      { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
      { by: { type: "birta" }, op: "snap", rev: revOf(orig), text: orig },
      { by: { type: "birta" }, op: "owner", drafted_by: null },
      {
        by: human,
        cid: "c-4",
        op: "comment",
        verdict: "ask",
        body: "why a dashboard?",
        at: { quote: "metrics dashboard", pre: "", suf: "", lines: [11, 11], rev: revOf(orig), scope: "quote" },
      },
      { by: human, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
      { by: agent, op: "claim", round: 5 },
      {
        by: agent,
        op: "comment",
        at: { quote: "- retries on 503", pre: "", suf: "", lines: [10, 10], rev: revOf(orig), scope: "block" },
        verdict: "ask",
        body: "only 503?",
        turn: 6,
      },
      {
        by: { type: "birta" },
        op: "external",
        rev: revOf(orig),
        to_rev: revOf(doc),
        patch: [{ a: FIX.line - 1, d: [ORIG_LINES[FIX.line - 1] + "\n"], i: [FIX.to + "\n"] }],
      },
      { by: agent, op: "yield", turn: 6 },
    ];
    return { doc, log: ops.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join("") };
  }

  const walkLength = (page: Page) =>
    page.evaluate(() => {
      const w = window as unknown as { BR: { pure: { attention(s: State): unknown[] }; currentState(): State } };
      const s = w.BR.currentState();
      return s ? w.BR.pure.attention(s).length : null;
    });

  test("on a real server the walk's length is the count behind the tab's news, at every step", async ({ page, birta }) => {
    const s = seededMix();
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    const got = watchReceived(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    // Setup: your ask went unanswered, the agent's question is open, its fix unseen
    await expect.poll(() => lastState(got)?.items.map((i: State) => [i.n, i.status])).toEqual([
      [4, "not_handled"],
      [7, "open"],
    ]);
    expect(lastState(got).counts).toMatchObject({ attention: 3, to_check: 3, open_asks: 1, unseen: 1 });
    expect(await walkLength(page)).toBe(3);
    await expect(page).toHaveTitle(/^● /);

    await reply(page, "not handled in").click();
    await page.keyboard.press("y");
    await expect.poll(() => lastState(got)?.counts.to_check).toBe(2);
    await expect.poll(() => walkLength(page)).toBe(2);

    // Focus on the fix's block checks nothing; y there does
    await focusOn(page, lastState(got).changes[0].range.sp);
    await page.waitForTimeout(SETTLE_MS);
    expect(lastState(got).counts.to_check).toBe(2);
    expect(await walkLength(page)).toBe(2);
    await page.keyboard.press("y");
    await expect.poll(() => lastState(got)?.counts.to_check).toBe(1);
    await expect.poll(() => walkLength(page)).toBe(1);
    // The agent's question is all that is left, and it is still news
    await expect(page).toHaveTitle(/^● /);
  });
});
