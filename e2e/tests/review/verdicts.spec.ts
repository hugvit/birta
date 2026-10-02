import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  Mock,
  STOPS_DOC,
  State,
  booted,
  fixtureDoc,
  fixtureState,
  focused,
  longDoc,
  marks,
  mockReview,
  revOf,
  reviewLog,
  stopSp,
  stops,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
/** How long the first x on an open change row waits for the second. */
const ARM_MS = 3_000;

const ROW_X = "x = no to this text · u reverts Claude's change · x again for no";
const BLANK_PLACEHOLDER = "type an answer · y yes · x no · . your call · l later";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, state?: State, acks: "auto" | "manual" = "auto") {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state, acks });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

/** Serves committed fixture `name` with its state, bent by `edit`. */
async function openFixture(page: Page, birta: Start, name: string, edit?: (s: State) => void) {
  const state = fixtureState(name);
  edit?.(state);
  return { ...(await open(page, birta, fixtureDoc(name), state)), state };
}

/** Serves `doc` with the real socket and a review file of `lines`, timed ten minutes back. */
async function openReal(page: Page, birta: Start, doc: string, lines: State[] = [], env: Record<string, string> = {}) {
  const T0 = Date.now() - 600_000;
  const files: Record<string, string> = { "pr.md": doc };
  if (lines.length) {
    files["pr.md.birta.jsonl"] = lines.map((l, i) => JSON.stringify(i ? { ...l, ts: T0 + (i + 1) * 1_000 } : l) + "\n").join("");
  }
  const served = await birta(files, { ...GATE_ON, ...env });
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return served;
}

const HUMAN = { type: "human", via: "browser" };
const AGENT = { type: "agent", name: "claude", via: "cli" };

/** A review file in which the agent took a hand-back and asked about "Detail words.". */
function askedLog(doc: string): State[] {
  const T0 = Date.now() - 600_000;
  return [
    { birta: "review/1", doc: "pr.md", about: "ops after the last hand-back are drafts; don't act on them" },
    { by: { type: "birta" }, op: "snap", rev: revOf(doc), text: doc },
    { by: { type: "birta" }, op: "owner", drafted_by: null },
    { by: HUMAN, cid: "c-4", op: "comment", at: "doc", body: "look it over" },
    { by: HUMAN, op: "submit", upto: 4, not_before: T0 + 5_000, hold_ms: 0, to: "*" },
    { by: AGENT, op: "claim", round: 5 },
    {
      by: AGENT,
      op: "comment",
      at: { quote: "Detail words.", pre: "", suf: "", lines: [29, 29], rev: revOf(doc), scope: "block" },
      verdict: "ask",
      body: "only here?",
      turn: 6,
    },
    { by: AGENT, op: "done", to: 4, turn: 6 },
    { by: AGENT, op: "yield", turn: 6 },
  ];
}

/** The pill's text while it is open in the top layer and painted, else null. */
function pillOf(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    for (const host of document.querySelectorAll("birta-pop")) {
      const root = host.shadowRoot ?? host;
      for (const el of root.querySelectorAll('[role="status"]')) {
        let open = false;
        try {
          open = el.matches(":popover-open") && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
        } catch {
          open = false;
        }
        const text = (el.textContent ?? "").replace(/\s+/g, " ").trim();
        if (open && text) return text;
      }
    }
    return null;
  });
}

/** The fields of an op frame, without the envelope the core adds. */
function opFields(frame: State): State {
  const out = { ...frame };
  for (const k of ["type", "cid", "path", "rev"]) delete out[k];
  return out;
}

const opsNamed = (mock: Mock, name: string) => mock.ops().filter((o) => o.op === name).map(opFields);
/** Every op but seen, which moving onto a changed block sends. */
const writes = (mock: Mock) => mock.ops().filter((o) => o.op !== "seen").map(opFields);

/** Drops focus to nothing with Esc: k stops at a whole-doc H1, so it can't reach the doc. */
async function unfocus(page: Page) {
  for (let i = 0; i < 4 && (await focused(page)) !== null; i++) await page.keyboard.press("Escape");
  expect(await focused(page)).toBeNull();
}

/** Presses j from nothing until `sp` has focus. */
async function focusOn(page: Page, sp: string) {
  await unfocus(page);
  const all = await stops(page);
  for (let i = 0; i <= all.length + 4 && (await focused(page)) !== sp; i++) await page.keyboard.press("j");
  await expect.poll(() => focused(page)).toBe(sp);
}

/** The question of the blank that holds the keyboard, or null. */
function blankFocus(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const a = document.activeElement;
    if (!a || a.localName !== "birta-flow" || a.getAttribute("data-kind") !== "blank") return null;
    return (a.shadowRoot?.textContent ?? "").replace(/\s+/g, " ").trim();
  });
}

/** Presses j from nothing until the blank asking `question` holds the keyboard. */
async function focusBlank(page: Page, question: string) {
  await unfocus(page);
  const all = await stops(page);
  for (let i = 0; i <= all.length + 8 && !((await blankFocus(page)) ?? "").includes(question); i++) await page.keyboard.press("j");
  await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(question);
}

/** Lands on the blank asking `question` with n, the walk's own way there. */
async function walkToBlank(page: Page, question: string) {
  // The walk has stops once the state is drawn
  await page.locator('birta-flow[data-kind="blank"]', { hasText: question }).waitFor();
  // The walk stops on every blank in doc order, answered ones too
  for (let i = 0; i < 6 && !((await blankFocus(page)) ?? "").includes(question); i++) await page.keyboard.press("n");
  await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(question);
}

/** flow.json with only its two blanks: n has the open one as its one stop. */
function blanksOnly(s: State) {
  s.changes = [];
  s.deletions = [];
  s.lost = [];
  s.items = s.items.filter((i: State) => i.n === 21 || i.n === 22);
  s.counts = { ...s.counts, unseen: 0, attention: 1, not_handled: 0, lost: 0 };
}

const blank = (page: Page, has: string) => page.locator('birta-flow[data-kind="blank"]', { hasText: has });
const answerField = (page: Page) => page.locator('birta-flow[data-kind="blank"] textarea');
const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"] textarea');

/** The key hint the turn button shows: ⌘⏎ on a Mac, Ctrl⏎ elsewhere. */
async function turnKey(page: Page): Promise<string> {
  return (await page.evaluate(() => /Mac/.test(navigator.platform))) ? "⌘⏎" : "Ctrl⏎";
}

/** Keeps only change `id` from flow.json, and the note it cites, so `n` has one stop. */
function onlyChange(id: string, sp?: string) {
  return (s: State) => {
    s.changes = s.changes.filter((c: State) => c.id === id);
    if (sp) s.changes[0].range.sp = sp;
    s.deletions = [];
    s.lost = [];
    s.items = s.items.filter((i: State) => (s.changes[0].asked.notes ?? []).includes(i.n));
    s.counts = { ...s.counts, unseen: 1, attention: 1, open_asks: 0, not_handled: 0, lost: 0, drafts: 0 };
  };
}

const sourceShown = (page: Page) => page.locator("#source-view").isVisible();

// --- C1: u reverts the open row's change, and is the source view everywhere else --------------

test.describe("u", () => {
  test("on an open change row u reverts that change, with no confirm, and the page stays rendered", async ({
    page,
    birta,
  }) => {
    const { mock } = await openFixture(page, birta, "flow", onlyChange("13.0"));
    await page.keyboard.press("n");
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([{ op: "undo_change", change: "13.0" }]);
    expect(await sourceShown(page)).toBe(false);
    expect(opsNamed(mock, "comment")).toEqual([]);
  });

  test("u on a change birta can't revert says so, writes nothing and keeps the page rendered", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      onlyChange("13.0")(s);
      s.changes[0].reversible = { ok: false, reason: "later_overlap" };
    });
    await page.keyboard.press("n");
    await page.keyboard.press("u");
    await expect.poll(() => pillOf(page)).toBe("can't undo this one · c instead");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
    expect(await sourceShown(page)).toBe(false);
  });

  test("with no row open u shows the source and says how back, and u again returns to the focused block", async ({
    page,
    birta,
  }) => {
    await open(page, birta, longDoc());
    // Focus a block well down the page, so the source view's height change would scroll it away
    const target = await stopSp(page, "Filler paragraph 20.");
    const header = (await page.locator(".file-header").boundingBox())!;
    await page.evaluate(
      ({ sp, below }) => {
        const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
        window.scrollBy(0, el.getBoundingClientRect().top - below);
      },
      { sp: target, below: header.y + header.height + 10 },
    );
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(target);

    await page.keyboard.press("u");
    await expect(page.locator("#source-view")).toBeVisible();
    await expect.poll(() => pillOf(page)).toBe("source view · u back");
    await page.keyboard.press("u");
    await expect(page.locator("#lane-frame")).toBeVisible();
    await expect.poll(() => pillOf(page)).toBe("rendered view");
    await expect.poll(() => focused(page)).toBe(target);
    const box = (await page.locator(`#content [data-sourcepos="${target}"]`).boundingBox())!;
    expect(box.y).toBeGreaterThanOrEqual(header.y + header.height - 1);
    expect(box.y + box.height).toBeLessThanOrEqual(WIDE.height + 1);
  });

  test("after the source view round trip the lane's bars are drawn again, also when a state lands meanwhile", async ({
    page,
    birta,
  }) => {
    const state = fixtureState("unseen");
    const { mock } = await open(page, birta, fixtureDoc("unseen"), state);
    const bars = async () => (await marks(page)).filter((m) => m.mark === "bar" && m.height > 0).map((m) => m.sp).sort();
    await expect.poll(async () => (await bars()).length).toBeGreaterThan(0);
    const before = await bars();
    await page.keyboard.press("u");
    await expect(page.locator("#source-view")).toBeVisible();
    mock.pushState(state);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("u");
    await expect(page.locator("#lane-frame")).toBeVisible();
    await expect.poll(bars).toEqual(before);
  });

  test("after approve, u with no row open still toggles the source view", async ({ page, birta }) => {
    await openFixture(page, birta, "approved");
    await page.keyboard.press("j");
    await page.keyboard.press("u");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("u");
    await expect(page.locator("#lane-frame")).toBeVisible();
  });
});

// --- x on an open change row asks first ---------------------------------------------------------

test.describe("x on an open change row", () => {
  test("the first x only asks; a second x writes no on the changed block", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", onlyChange("13.0"));
    await page.keyboard.press("n");
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ROW_X);
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
    await page.keyboard.press("x");
    await expect
      .poll(() => writes(mock))
      .toEqual([{ op: "comment", at: { k: "block", sp: "5:1-6:68", scope: "block" }, verdict: "no" }]);
  });

  test("a second x after the arm window asks again instead of writing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", onlyChange("13.0"));
    await page.keyboard.press("n");
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ROW_X);
    await page.waitForTimeout(ARM_MS + 300);
    await page.keyboard.press("x");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
    await page.keyboard.press("x");
    await expect.poll(() => opsNamed(mock, "comment")).toHaveLength(1);
    expect(opsNamed(mock, "undo_change")).toEqual([]);
  });
});

test.describe("rows that can't take a no or a revert", () => {
  test("x on a removal's row says there is nothing to say no to, and writes nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "unseen", (s) => {
      s.changes = s.changes.filter((c: State) => c.id === "11.0");
      s.deletions = s.deletions.filter((d: State) => d.change === "11.0");
      s.items = [];
      s.lost = [];
      s.counts = { ...s.counts, unseen: 1, attention: 1, open_asks: 0, not_handled: 0, lost: 0, drafts: 0 };
    });
    await page.keyboard.press("n");
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe("nothing to say no to · u reverts");
    await page.keyboard.press("x");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("u on a muted row from an earlier round toggles the source view and reverts nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      onlyChange("13.0")(s);
      // A later round the agent has answered: 13.0 is from the one before
      s.rounds.push({ submit: 30, ts: s.server_now - 30_000, by: "", to: "*", items: [], claim: 31, claimed_by: "claude", yield: 35, released: false });
    });
    await page.keyboard.press("n");
    await page.keyboard.press("u");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("u");
    await expect(page.locator("#lane-frame")).toBeVisible();
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "undo_change")).toEqual([]);
  });
});

// --- C4: the whole-doc stop ----------------------------------------------------------------------

test.describe("the whole-doc stop", () => {
  test("a top H1 that spans the doc is the doc stop: k from below lands on it and goes no further", async ({
    page,
    birta,
  }) => {
    await open(page, birta, STOPS_DOC);
    const [h1, why] = (await stops(page)).map((s) => s.sp);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(h1);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(why);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe(h1);
    await page.keyboard.press("k");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(h1);
    // The file chip is no stop of its own here
    expect(await page.locator(".file-header-name[data-review-focus]").count()).toBe(0);
  });

  test("there l, ., x and y say what to do instead and write nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "fresh");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    const said: Record<string, string | null> = {};
    for (const key of ["l", ".", "x", "y"]) {
      await page.keyboard.press(key);
      await expect.poll(() => pillOf(page)).not.toBe(said[Object.keys(said).at(-1) ?? ""] ?? null);
      said[key] = await pillOf(page);
    }
    expect(said).toEqual({
      l: "later needs a block · j",
      ".": "your call needs a block · j",
      x: "whole doc: c to say what's wrong",
      y: `${await turnKey(page)} approves`,
    });
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("there y names what the button would do: hand back while notes wait", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    await page.keyboard.press("y");
    await expect.poll(() => pillOf(page)).toBe(`${await turnKey(page)} hands back`);
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("there c and q write about the whole doc", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("j");
    await page.keyboard.press("c");
    await expect(composer(page)).toBeFocused();
    await page.keyboard.type("overall fine");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment")).toEqual([{ op: "comment", at: { k: "doc" }, body: "overall fine" }]);
    await focusOn(page, (await stops(page))[0].sp);
    await page.keyboard.press("q");
    await expect(composer(page)).toBeFocused();
    await page.keyboard.type("who reads this?");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment")).toHaveLength(2);
    expect(opsNamed(mock, "comment")[1]).toEqual({ op: "comment", at: { k: "doc" }, body: "who reads this?", verdict: "ask" });
  });

  test("with no such H1 the file chip is the doc stop, and there too verdict keys only say what to do", async ({
    page,
    birta,
  }) => {
    const { mock } = await open(page, birta, "Intro words.\n\n# Title\n\nBody words.\n");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).not.toBeNull();
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBeNull();
    await expect(page.locator(".file-header-name[data-review-focus]")).toHaveCount(1);
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe("whole doc: c to say what's wrong");
    await page.keyboard.press("l");
    await expect.poll(() => pillOf(page)).toBe("later needs a block · j");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });

  test("a first H1 that a second H1 ends is an ordinary stop: x there is no on its section", async ({ page, birta }) => {
    const { mock } = await open(page, birta, "# One\n\nBody words.\n\n# Two\n\nMore words.\n");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:5");
    await page.keyboard.press("x");
    await expect
      .poll(() => writes(mock))
      .toEqual([{ op: "comment", at: { k: "block", sp: "1:1-1:5", scope: "section" }, verdict: "no" }]);
    await expect.poll(() => pillOf(page)).toBe("no · # One (section) · z to undo");
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBeNull();
  });

  test("with a change row open on the whole-doc H1, x, y, u and c act on the row", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", onlyChange("13.0", "1:1-1:16"));
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([{ op: "undo_change", change: "13.0" }]);
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ROW_X);
    const seenBefore = opsNamed(mock, "seen").length;
    await page.keyboard.press("y");
    await expect.poll(() => opsNamed(mock, "seen").length).toBe(seenBefore + 1);
    expect(opsNamed(mock, "seen").at(-1)).toEqual({ op: "seen", ids: ["13.0"] });
    await page.keyboard.press("c");
    await expect(composer(page)).toBeFocused();
    await page.keyboard.type("why here?");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "comment")).toEqual([{ op: "comment", at: { k: "change", id: "13.0" }, body: "why here?" }]);
  });
});

// --- #16: every verdict pill names its scope ----------------------------------------------------

/** drafts.json with its list item's verdict carrying no words, so the same key retracts it. */
function bareVerdict(s: State) {
  delete s.items.find((i: State) => i.n === 5).body;
}

test.describe("verdict pills", () => {
  test("a verdict on a heading names its section, and one on a block names its first words and kind", async ({
    page,
    birta,
  }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await focusOn(page, "3:1-3:6");
    await page.keyboard.press("l");
    await expect.poll(() => pillOf(page)).toBe("later · ## Why (section) · z to undo");
    await focusOn(page, await stopSp(page, "Nothing yet."));
    await page.keyboard.press(".");
    await expect.poll(() => pillOf(page)).toBe('your call · "Nothing yet." (paragraph) · z to undo');
    await focusOn(page, await stopSp(page, "backoff of 1, 2 and 4 seconds"));
    await page.keyboard.press("y");
    await expect.poll(() => pillOf(page)).toBe('yes · "backoff of 1, 2 and 4…" (list item) · z to undo');
    expect(writes(mock).map((o) => o.verdict)).toEqual(["later", "your_call", "yes"]);
  });

  test("a long block's words are cut at six", async ({ page, birta }) => {
    await openFixture(page, birta, "drafts");
    await focusOn(page, "5:1-6:54");
    await page.keyboard.press("y");
    await expect.poll(() => pillOf(page)).toBe('yes · "When the upload hits a 503…" (paragraph) · z to undo');
  });

  test("a verdict on a selection names the selected words", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await page.evaluate((sp) => {
      const p = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      const r = document.createRange();
      r.setStart(p.firstChild!, 0);
      r.setEnd(p.firstChild!, 6);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    }, sp);
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe('no · "Detail" · z to undo');
  });

  test("replacing a verdict names the new one", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", bareVerdict);
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("y");
    await expect.poll(() => pillOf(page)).toBe('yes · "retries on 503" (list item) · z to undo');
    expect(writes(mock)).toEqual([{ op: "amend", to: 5, verdict: "yes" }]);
  });

  test("the same key on a bare verdict retracts it and says later removed", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", (s) => {
      bareVerdict(s);
      s.items.find((i: State) => i.n === 5).verdict = "later";
    });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("l");
    await expect.poll(() => pillOf(page)).toBe("later removed · z to undo");
    expect(writes(mock)).toEqual([{ op: "retract", to: 5 }]);
  });

  test("the same key on a verdict with words drops it and says no removed", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe("no removed · z to undo");
    expect(writes(mock)).toEqual([{ op: "amend", to: 5, drop_verdict: true }]);
  });

  test("on a fresh doc the first verdict's pill names its scope and stays, not review file created", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC);
    const why = await stopSp(page, "Why");
    await focusOn(page, why);
    await page.keyboard.press("x");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    await page.waitForTimeout(SETTLE_MS);
    expect(await pillOf(page)).toBe("no · ## Why (section) · z to undo");
  });
});

// --- #1: what c writes ----------------------------------------------------------------------

test.describe("c", () => {
  /** drafts.json with the list item's draft a plain note: words, no verdict. */
  const plainNote = (s: State) => delete s.items.find((i: State) => i.n === 5).verdict;

  test("on a block with your draft note c opens it to edit, and ⏎ sends the edit", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", plainNote);
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet");
    await composer(page).fill("drop it");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 5, body: "drop it" }]);
  });

  test("on a block whose draft is a quote note, c edits that note", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await focusOn(page, "5:1-6:54");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("say why");
    await composer(page).fill("say why, please");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 4, body: "say why, please" }]);
  });

  test("with two drafts on the block, c edits the newer", async ({ page, birta }) => {
    await openFixture(page, birta, "drafts", (s) => {
      plainNote(s);
      const newer = JSON.parse(JSON.stringify(s.items.find((i: State) => i.n === 5)));
      s.items.push({ ...newer, n: 7, cid: "c-7", body: "second thought" });
    });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("second thought");
  });

  test("⏎ on an emptied edit takes the note back", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", plainNote);
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet");
    await composer(page).fill("");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "retract", to: 5 }]);
  });

  test("Esc saves an edit, as ⏎ does", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", plainNote);
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet");
    await composer(page).fill("drop it now");
    await page.keyboard.press("Escape");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 5, body: "drop it now" }]);
    await expect(composer(page)).toHaveCount(0);
  });

  test("a selection inside your draft's quote edits that draft", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await page.evaluate(() => {
      const p = document.querySelector('#content [data-sourcepos="5:1-6:54"]')!;
      const t = p.firstChild as Text;
      const at = t.data.indexOf("just");
      const r = document.createRange();
      r.setStart(t, at);
      r.setEnd(t, at + 4);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    });
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("say why");
    await composer(page).fill("say why it fails");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 4, body: "say why it fails" }]);
  });

  test("a fresh selection outside your quotes makes a new quote note, even on a block with a draft", async ({
    page,
    birta,
  }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await page.evaluate(() => {
      const p = document.querySelector('#content [data-sourcepos="5:1-6:54"]')!;
      const t = p.firstChild as Text;
      const at = t.data.indexOf("upload");
      const r = document.createRange();
      r.setStart(t, at);
      r.setEnd(t, at + 6);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    });
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("");
    await page.keyboard.type("which upload?");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toHaveLength(1);
    expect(writes(mock)[0]).toMatchObject({ op: "comment", at: { k: "quote", q: "upload" }, body: "which upload?" });
  });

  test("a note already handed back is never edited: c makes a new note", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", (s) => {
      plainNote(s);
      s.items.find((i: State) => i.n === 5).status = "sent";
    });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("");
    await page.keyboard.type("one more");
    await page.keyboard.press("Enter");
    await expect
      .poll(() => writes(mock))
      .toEqual([{ op: "comment", at: { k: "block", sp: "10:1-10:16", scope: "block" }, body: "one more" }]);
  });

  test("an open change row wins over your draft on its block: c notes the change", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      const note = JSON.parse(JSON.stringify(fixtureState("drafts").items.find((i: State) => i.n === 5)));
      onlyChange("13.0")(s);
      delete note.verdict;
      s.items.push({ ...note, n: 40, cid: "c-40", body: "mine", at: { ...note.at, loc: { sp: "5:1-6:68" } } });
    });
    await page.keyboard.press("n");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("");
    await page.keyboard.type("why?");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "comment", at: { k: "change", id: "13.0" }, body: "why?" }]);
  });

  test("at the doc stop c edits your whole-doc draft", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await page.keyboard.press("j");
    await unfocus(page);
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("is the test ladder still needed?");
    await composer(page).fill("is the ladder needed?");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 6, body: "is the ladder needed?" }]);
  });
});

// --- C6: blanks ----------------------------------------------------------------------------------

const OPEN_Q = "is 503 the only code?";
const ANSWERED_Q = "keep this title?";

test.describe("blanks", () => {
  test("j and k stop on a blank like on a block", async ({ page, birta }) => {
    await openFixture(page, birta, "flow");
    await unfocus(page);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    await page.keyboard.press("j");
    await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(ANSWERED_Q);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("3:1-3:6");
    expect(await blankFocus(page)).toBeNull();
    await page.keyboard.press("j");
    await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(OPEN_Q);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("5:1-6:68");
    await page.keyboard.press("k");
    await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(OPEN_Q);
  });

  for (const [key, from, to] of [
    ["n", ANSWERED_Q, OPEN_Q],
    ["p", OPEN_Q, ANSWERED_Q],
  ]) {
    test(`${key} on a closed blank walks to the next stop and types nothing`, async ({ page, birta }) => {
      // Both blanks open, so the walk has a stop on either side
      const { mock } = await openFixture(page, birta, "flow", (s) => {
        blanksOnly(s);
        const asked = s.items.find((i: State) => i.n === 22);
        asked.status = "open";
        asked.replies = [];
      });
      await walkToBlank(page, ANSWERED_Q);
      if (from === OPEN_Q) await walkToBlank(page, OPEN_Q);
      await page.keyboard.press(key);
      await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(to);
      expect(await answerField(page).count()).toBe(0);
      expect(writes(mock)).toEqual([]);
    });
  }

  for (const key of ["j", "k"]) {
    test(`${key} on a closed blank moves on and types nothing`, async ({ page, birta }) => {
      const { mock } = await openFixture(page, birta, "flow", blanksOnly);
      await walkToBlank(page, OPEN_Q);
      await page.keyboard.press(key);
      await page.waitForTimeout(SETTLE_MS);
      expect(await answerField(page).count()).toBe(0);
      expect(((await blankFocus(page)) ?? "").includes(OPEN_Q)).toBe(false);
      expect(writes(mock)).toEqual([]);
    });
  }

  for (const [key, verdict, word] of [
    ["y", "yes", "yes"],
    ["x", "no", "no"],
    [".", "your_call", "your call"],
    ["l", "later", "later"],
  ] as const) {
    test(`${key} on a focused blank answers ${word}, and the pill says so`, async ({ page, birta }) => {
      const { mock } = await openFixture(page, birta, "flow", blanksOnly);
      await walkToBlank(page, OPEN_Q);
      await page.keyboard.press(key);
      await expect.poll(() => opsNamed(mock, "reply")).toEqual([{ op: "reply", to: 21, verdict }]);
      await expect.poll(() => pillOf(page)).toBe(`answered ${word} · z to undo`);
    });
  }

  test("z takes back the answer you just gave", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", blanksOnly);
    await walkToBlank(page, OPEN_Q);
    await page.keyboard.press("y");
    await expect.poll(() => opsNamed(mock, "reply")).toHaveLength(1);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("z");
    // The mock acks the reply as line 101
    await expect.poll(() => opsNamed(mock, "retract")).toEqual([{ op: "retract", to: 101 }]);
  });

  for (const key of ["Enter", "c"]) {
    test(`${key} on a focused blank opens its field, labelled and as wide as the doc, where a leading y is text`, async ({
      page,
      birta,
    }) => {
      const { mock } = await openFixture(page, birta, "flow", blanksOnly);
      await walkToBlank(page, OPEN_Q);
      await page.keyboard.press(key);
      await expect(answerField(page)).toBeFocused();
      await expect(answerField(page)).toHaveAttribute("placeholder", BLANK_PLACEHOLDER);
      await expect(blank(page, OPEN_Q)).toContainText("answer to Claude");
      const field = (await answerField(page).boundingBox())!;
      const doc = (await page.locator("#content").boundingBox())!;
      expect(field.width).toBeGreaterThan(doc.width * 0.9);
      await page.keyboard.type("yes, 15 minutes");
      await page.keyboard.press("Enter");
      await expect.poll(() => opsNamed(mock, "reply")).toEqual([{ op: "reply", to: 21, body: "yes, 15 minutes" }]);
    });
  }

  test("a click on the blank opens its field, where typing is text", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", blanksOnly);
    await blank(page, OPEN_Q).click();
    await expect(answerField(page)).toBeFocused();
    await page.keyboard.type("yes it is");
    await page.keyboard.press("Enter");
    await expect.poll(() => opsNamed(mock, "reply")).toEqual([{ op: "reply", to: 21, body: "yes it is" }]);
  });

  test("another letter on a closed blank says how to type, and writes nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", blanksOnly);
    await walkToBlank(page, OPEN_Q);
    await page.keyboard.press("h");
    await expect.poll(() => pillOf(page)).toBe("⏎ to type an answer");
    await page.waitForTimeout(SETTLE_MS);
    expect(await answerField(page).count()).toBe(0);
    expect(writes(mock)).toEqual([]);
  });

  test("Esc saves a blank's typed answer, and an empty field writes nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", blanksOnly);
    await walkToBlank(page, OPEN_Q);
    await page.keyboard.press("Enter");
    await expect(answerField(page)).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(answerField(page)).toHaveCount(0);
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
    // Closing the field hands the keyboard back to its blank
    expect(await blankFocus(page)).toContain(OPEN_Q);
    await page.keyboard.press("Enter");
    await page.keyboard.type("maybe");
    await page.keyboard.press("Escape");
    await expect.poll(() => writes(mock)).toEqual([{ op: "reply", to: 21, body: "maybe" }]);
  });

  test("an answered blank hides its empty slot, and a second answer amends the first", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      const r = s.items.find((i: State) => i.n === 22).replies[0];
      delete r.body;
      r.verdict = "yes";
    });
    await expect(blank(page, ANSWERED_Q)).toBeVisible();
    const slotShown = () =>
      page.evaluate((q) => {
        for (const h of document.querySelectorAll('birta-flow[data-kind="blank"]')) {
          if (!(h.shadowRoot?.textContent ?? "").includes(q)) continue;
          const slot = h.shadowRoot!.querySelector(".blank");
          return !!slot && slot.getClientRects().length > 0;
        }
        return null;
      }, ANSWERED_Q);
    expect(await slotShown()).toBe(false);
    await focusBlank(page, ANSWERED_Q);
    await page.keyboard.press("x");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 25, verdict: "no" }]);
  });

  test("⏎ on a blank answered in words opens them to edit, and ⏎ amends them", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", blanksOnly);
    await focusBlank(page, ANSWERED_Q);
    await page.keyboard.press("Enter");
    await expect(answerField(page)).toHaveValue("yes, keep it");
    await answerField(page).fill("yes, keep it for now");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 25, body: "yes, keep it for now" }]);
  });

  test("on a real server a second answer amends the first, and z takes an answer back", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, askedLog(STOPS_DOC));
    const log = () => reviewLog(served.root)!;
    await walkToBlank(page, "only here?");
    await page.keyboard.press("y");
    await expect.poll(() => log().at(-1)).toMatchObject({ op: "reply", to: 7, verdict: "yes" });
    const answer = log().length;
    const shown = (word: string) => page.locator('birta-flow[data-kind="reply"]', { hasText: `↳ you · ${word}` });
    await expect(shown("yes")).toBeVisible();
    // The blank keeps the keyboard once answered
    expect(await blankFocus(page)).toContain("only here?");
    await page.keyboard.press("x");
    await expect.poll(() => log().at(-1)).toMatchObject({ op: "amend", to: answer, verdict: "no" });
    expect(log().filter((l) => l.op === "reply")).toHaveLength(1);
    await expect(page.locator(".file-header birta-turn")).toContainText("Hand back · 1");
    // z after a second answer puts the first back
    await expect(shown("no")).toBeVisible();
    await page.keyboard.press("z");
    await expect.poll(() => log().at(-1)).toMatchObject({ op: "amend", to: answer, verdict: "yes" });
    expect(log().filter((l) => l.op === "retract")).toEqual([]);

    // A fresh answer, taken back with z
    const served2 = await openReal(page, birta, STOPS_DOC, askedLog(STOPS_DOC));
    await walkToBlank(page, "only here?");
    await page.keyboard.press("l");
    await expect.poll(() => reviewLog(served2.root)!.at(-1)).toMatchObject({ op: "reply", to: 7, verdict: "later" });
    const n = reviewLog(served2.root)!.length;
    await page.keyboard.press("z");
    await expect.poll(() => reviewLog(served2.root)!.at(-1)).toMatchObject({ op: "retract", to: n });
  });

  test("the hand-back pill counts answers beside notes", async ({ page, birta }) => {
    await openReal(page, birta, STOPS_DOC, askedLog(STOPS_DOC));
    await walkToBlank(page, "only here?");
    await page.keyboard.press("y");
    await focusOn(page, await stopSp(page, "Nothing yet."));
    await page.keyboard.press("c");
    await page.keyboard.type("fine");
    await page.keyboard.press("Enter");
    await expect(page.locator(".file-header birta-turn")).toContainText("Hand back · 2");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => pillOf(page)).toBe("handing back · 1 note · 1 answer · z to undo");
  });
});

// --- The adversary's killers and the lead's rulings ---------------------------------------------

/** Selects `len` characters at `word` in the block `sp`'s first text node. */
async function select(page: Page, sp: string, word: string, len = word.length) {
  await page.evaluate(
    ({ sp, word, len }) => {
      const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
      const el = all[all.length - 1]!;
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode() as Text | null; n; n = walker.nextNode() as Text | null) {
        const at = n.data.indexOf(word);
        if (at < 0) continue;
        const r = document.createRange();
        r.setStart(n, at);
        r.setEnd(n, at + len);
        getSelection()!.removeAllRanges();
        getSelection()!.addRange(r);
        return;
      }
      throw new Error(`no text in ${sp} holds ${word}`);
    },
    { sp, word, len },
  );
}

/** flow.json with changes 13.0 and 15.0 only, so `n` reaches both. */
function twoChanges(s: State) {
  s.changes = s.changes.filter((c: State) => c.id === "13.0" || c.id === "15.0");
  s.deletions = [];
  s.lost = [];
  s.items = s.items.filter((i: State) => i.n === 10 || i.n === 6);
  s.counts = { ...s.counts, unseen: 2, attention: 2, open_asks: 0, not_handled: 0, lost: 0, drafts: 0 };
}

/** Your draft quote note on "we just fail" in flow.json's paragraph. */
function quoteDraftOnFlow(s: State) {
  const note = JSON.parse(JSON.stringify(fixtureState("drafts").items.find((i: State) => i.n === 4)));
  s.items.push({ ...note, n: 40, cid: "c-40", body: "say why", at: { ...note.at, loc: { sp: "5:1-6:68", off: [27, 39] } } });
}

test.describe("killers: u and the row", () => {
  test("K5: after approve, u on a changed block toggles the source view and reverts nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved");
    await focusOn(page, "5:1-6:68");
    await page.keyboard.press("u");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "undo_change")).toEqual([]);
  });

  test("K6: a click away from the open row leaves u the source view", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", onlyChange("13.0"));
    await page.keyboard.press("n");
    await page.locator('#content [data-sourcepos="16:1-16:19"]').click();
    await page.keyboard.press("u");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.waitForTimeout(SETTLE_MS);
    expect(opsNamed(mock, "undo_change")).toEqual([]);
  });

  test("K7: the x guard belongs to its row: another row, j and k, or a held x never confirm it", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", twoChanges);
    await page.keyboard.press("n");
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ROW_X);
    await page.keyboard.press("n");
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe(ROW_X);
    await page.keyboard.press("j");
    await page.keyboard.press("k");
    await page.keyboard.press("x");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
    await page.keyboard.down("x");
    await page.keyboard.down("x");
    await page.keyboard.up("x");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toHaveLength(1);
  });

  test("K8: the second x pills its scope and z takes it back; x then u reverts and writes no verdict", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", onlyChange("13.0"));
    await page.keyboard.press("n");
    await page.keyboard.press("x");
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe('no · "When the upload hits a 503…" (paragraph) · z to undo');
    await page.keyboard.press("z");
    // The mock acks every op in turn from line 101, the walk's seen first
    const line = 101 + mock.ops().findIndex((o) => o.op === "comment");
    await expect.poll(() => opsNamed(mock, "retract")).toEqual([{ op: "retract", to: line }]);

    await page.reload();
    await booted(page);
    const before = mock.ops().length;
    await page.keyboard.press("n");
    await page.keyboard.press("x");
    await page.keyboard.press("u");
    await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([{ op: "undo_change", change: "13.0" }]);
    expect(mock.ops().slice(before).filter((o) => o.op === "comment")).toEqual([]);
  });

  test("K17: u on a reverted row says so and writes nothing", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      onlyChange("13.0")(s);
      s.changes[0].undone_by = 40;
      s.changes[0].reversible = { ok: false, reason: "later_overlap" };
    });
    await page.keyboard.press("n");
    await page.keyboard.press("u");
    await expect.poll(() => pillOf(page)).toBe("already reverted · c note to ask for it back");
    await page.waitForTimeout(SETTLE_MS);
    expect(await sourceShown(page)).toBe(false);
    expect(writes(mock)).toEqual([]);
  });

  test("Q7: x on a row whose change can't be reverted leaves the u part out", async ({ page, birta }) => {
    await openFixture(page, birta, "flow", (s) => {
      onlyChange("13.0")(s);
      s.changes[0].reversible = { ok: false, reason: "later_overlap" };
    });
    await page.keyboard.press("n");
    await page.keyboard.press("x");
    await expect.poll(() => pillOf(page)).toBe("x = no to this text · x again for no");
  });

  test("Q6: a selection beats the open row: x is no on the words", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", onlyChange("13.0"));
    await page.keyboard.press("n");
    await select(page, "5:1-6:68", "upload");
    await page.keyboard.press("x");
    await expect.poll(() => writes(mock)).toHaveLength(1);
    expect(writes(mock)[0]).toMatchObject({ op: "comment", at: { k: "quote", q: "upload" }, verdict: "no" });
  });

  for (const [bind, revert, raw] of [
    ["revert_change=s", "s", "u"],
    ["toggle_raw=s", "u", "s"],
  ]) {
    test(`K20 / Q8: with --bind ${bind}, ${revert} reverts the open row and ${raw} is the source view, and pills say so`, async ({
      page,
      birta,
    }) => {
      const state = fixtureState("flow");
      onlyChange("13.0")(state);
      const served = await birta({ "pr.md": fixtureDoc("flow") }, GATE_ON, ["pr.md", "--bind", bind]);
      await page.setViewportSize(WIDE);
      const mock = await mockReview(page, { state });
      await page.goto(`${served.url}/view/pr.md`);
      await booted(page);
      await page.keyboard.press("n");
      await page.keyboard.press("x");
      await expect.poll(() => pillOf(page)).toBe(`x = no to this text · ${revert} reverts Claude's change · x again for no`);
      if (raw === "u") {
        // u is only the source view here, row or not
        await page.keyboard.press("u");
        await expect(page.locator("#source-view")).toBeVisible();
        await expect.poll(() => pillOf(page)).toBe("source view · u back");
        await page.keyboard.press("u");
        await expect(page.locator("#lane-frame")).toBeVisible();
      } else {
        await page.keyboard.press("Escape");
        await expect.poll(() => focused(page)).toBeNull();
        // With no row open the revert's own key does nothing
        await page.keyboard.press("u");
        await page.waitForTimeout(SETTLE_MS);
        expect(await sourceShown(page)).toBe(false);
        await page.keyboard.press("s");
        await expect(page.locator("#source-view")).toBeVisible();
        await expect.poll(() => pillOf(page)).toBe("source view · s back");
        await page.keyboard.press("s");
        await expect(page.locator("#lane-frame")).toBeVisible();
        // Focus on the changed block opens its row again
        await focusOn(page, "5:1-6:68");
      }
      expect(opsNamed(mock, "undo_change")).toEqual([]);
      await page.keyboard.press(revert);
      await expect.poll(() => opsNamed(mock, "undo_change")).toEqual([{ op: "undo_change", change: "13.0" }]);
    });
  }
});

test.describe("killers: the whole-doc stop", () => {
  test("K9: a selection inside the whole-doc H1 is a quote, so x writes no on it", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await select(page, "1:1-1:16", "Upload");
    await page.keyboard.press("x");
    await expect.poll(() => writes(mock)).toHaveLength(1);
    expect(writes(mock)[0]).toMatchObject({ op: "comment", at: { k: "quote", q: "Upload" }, verdict: "no" });
  });

  test("K10: c on the whole-doc H1 edits your whole-doc draft", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("is the test ladder still needed?");
    await composer(page).fill("still needed?");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 6, body: "still needed?" }]);
  });

  test("K21: k from below a blank on the H1 reaches the blank, then the H1, and stops", async ({ page, birta }) => {
    await openFixture(page, birta, "flow");
    await focusOn(page, "3:1-3:6");
    await page.keyboard.press("k");
    await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(ANSWERED_Q);
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    await page.keyboard.press("k");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe("1:1-1:16");
    expect(await page.locator(".file-header-name[data-review-focus]").count()).toBe(0);
  });

  test("Q3: a doc-level blank sits above the whole-doc H1 and is the topmost stop", async ({ page, birta }) => {
    await openFixture(page, birta, "flow", (s) => {
      blanksOnly(s);
      s.items = s.items.filter((i: State) => i.n === 21);
      s.items[0].at = { kind: "doc", loc: null };
    });
    await page.keyboard.press("j");
    await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(OPEN_Q);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    await page.keyboard.press("k");
    await expect.poll(async () => (await blankFocus(page)) ?? "").toContain(OPEN_Q);
    await page.keyboard.press("k");
    await page.waitForTimeout(SETTLE_MS);
    expect(await blankFocus(page)).toContain(OPEN_Q);
  });

  test("Q2: after approve, y at the whole-doc stop says approved · c to reopen", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "approved");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe("1:1-1:16");
    await page.keyboard.press("y");
    await expect.poll(() => pillOf(page)).toBe("approved · c to reopen");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });
});

test.describe("killers: c and Esc", () => {
  test("K11: with a row open, a selection inside your draft's quote edits it, and one outside makes a quote note", async ({
    page,
    birta,
  }) => {
    const { mock } = await openFixture(page, birta, "flow", (s) => {
      onlyChange("13.0")(s);
      quoteDraftOnFlow(s);
    });
    await page.keyboard.press("n");
    await select(page, "5:1-6:68", "just");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("say why");
    await composer(page).fill("say why, please");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 40, body: "say why, please" }]);
    await select(page, "5:1-6:68", "upload");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("");
    await page.keyboard.type("which?");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toHaveLength(2);
    expect(writes(mock)[1]).toMatchObject({ op: "comment", at: { k: "quote", q: "upload" }, body: "which?" });
  });

  test("K12: Esc on an emptied edit changes nothing, in the composer and in a blank", async ({ page, birta }) => {
    const plain = await openFixture(page, birta, "drafts", (s) => delete s.items.find((i: State) => i.n === 5).verdict);
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet");
    await composer(page).fill("");
    await page.keyboard.press("Escape");
    await expect(composer(page)).toHaveCount(0);
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(plain.mock)).toEqual([]);
    await expect(page.locator('birta-flow[data-kind="note"]', { hasText: "drop this bullet" })).toBeVisible();

    const answered = await openFixture(page, birta, "flow", blanksOnly);
    await focusBlank(page, ANSWERED_Q);
    await page.keyboard.press("Enter");
    await expect(answerField(page)).toHaveValue("yes, keep it");
    await answerField(page).fill("");
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(answered.mock)).toEqual([]);
  });

  test("K13: Esc saves a new note, as ⏎ does", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Nothing yet.");
    await focusOn(page, sp);
    await page.keyboard.press("c");
    await page.keyboard.type("fine");
    await page.keyboard.press("Escape");
    await expect(composer(page)).toHaveCount(0);
    await expect.poll(() => writes(mock)).toEqual([{ op: "comment", at: { k: "block", sp, scope: "block" }, body: "fine" }]);
  });

  test("Q4: c on your draft ask edits its words and keeps it an ask", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", (s) => (s.items.find((i: State) => i.n === 5).verdict = "ask"));
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet");
    await composer(page).fill("why this bullet?");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 5, body: "why this bullet?" }]);
  });

  test("Q8 (ruling): ⏎ on an emptied edit of a verdict with words keeps the verdict and drops the words", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts");
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await composer(page).fill("");
    await page.keyboard.press("Enter");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 5, body: "" }]);
  });

  test("Q5: an unchanged edit sends nothing, and whitespace is empty", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", (s) => delete s.items.find((i: State) => i.n === 5).verdict);
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("drop this bullet");
    await page.keyboard.press("Enter");
    await expect(composer(page)).toHaveCount(0);
    await page.keyboard.press("c");
    await composer(page).fill("   ");
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(writes(mock)).toEqual([]);
  });
});

test.describe("killers: blanks", () => {
  test("K14: a verdict key over a typed answer replaces it whole", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", blanksOnly);
    await focusBlank(page, ANSWERED_Q);
    await page.keyboard.press("y");
    await expect.poll(() => writes(mock)).toEqual([{ op: "amend", to: 25, verdict: "yes", body: "" }]);
  });

  test("K16: once the hand-back's hold is over, z takes no answer back", async ({ page, birta }) => {
    const served = await openReal(page, birta, STOPS_DOC, askedLog(STOPS_DOC), { BIRTA_REVIEW_HOLD_MS: "300" });
    await walkToBlank(page, "only here?");
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "reply", verdict: "yes" });
    await page.locator('birta-flow[data-kind="reply"]', { hasText: "↳ you · yes" }).waitFor();
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => reviewLog(served.root)!.at(-1)).toMatchObject({ op: "submit" });
    await page.waitForTimeout(1_000);
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)!.filter((l) => l.op === "retract" || l.op === "withdraw")).toEqual([]);
  });

  test("K22: Esc mid-composition in a blank's field belongs to the IME", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", blanksOnly);
    await walkToBlank(page, OPEN_Q);
    await page.keyboard.press("Enter");
    await expect(answerField(page)).toBeFocused();
    await page.evaluate(() => {
      for (const h of document.querySelectorAll('birta-flow[data-kind="blank"]')) {
        const area = h.shadowRoot!.querySelector("textarea");
        if (!area) continue;
        area.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, composed: true }));
        area.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", isComposing: true, bubbles: true, composed: true, cancelable: true }));
      }
    });
    await page.waitForTimeout(SETTLE_MS);
    await expect(answerField(page)).toHaveCount(1);
    expect(writes(mock)).toEqual([]);
  });

  test("Q5: leaving a blank's field saves its words, as Esc does", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "flow", blanksOnly);
    await walkToBlank(page, OPEN_Q);
    await page.keyboard.press("Enter");
    await page.keyboard.type("only 503");
    await page.locator('#content [data-sourcepos="16:1-16:19"]').click();
    await expect.poll(() => writes(mock)).toEqual([{ op: "reply", to: 21, body: "only 503" }]);
    await expect(answerField(page)).toHaveCount(0);
  });
});

test.describe("killers: pill words", () => {
  test("K19: the same . again says your call removed", async ({ page, birta }) => {
    const { mock } = await openFixture(page, birta, "drafts", (s) => {
      bareVerdict(s);
      s.items.find((i: State) => i.n === 5).verdict = "your_call";
    });
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press(".");
    await expect.poll(() => pillOf(page)).toBe("your call removed · z to undo");
    expect(writes(mock)).toEqual([{ op: "retract", to: 5 }]);
  });

  test("a section under a subheading, a table row, a code block and inline markup name themselves", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC + "\n**Bold** words in six or so here.\n");
    const cases: [string, string][] = [
      ["Limits", "later · ### Limits (section) · z to undo"],
      ["404", 'later · "404 no" (table row) · z to undo'],
      ["retry(upload, 3);", 'later · "retry(upload, 3);" (code block) · z to undo'],
      ["Bold", 'later · "Bold words in six or so…" (paragraph) · z to undo'],
    ];
    for (const [text, pill] of cases) {
      await focusOn(page, await stopSp(page, text));
      await page.keyboard.press("l");
      await expect.poll(() => pillOf(page), { message: text }).toBe(pill);
    }
  });
});
