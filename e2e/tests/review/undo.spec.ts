import { BrowserContext, Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  GATE_ON,
  OPEN_ROOTS,
  REPO,
  STOPS_DOC,
  State,
  agent,
  booted,
  cacheCdn,
  changeRows,
  fixtureDoc,
  fixtureState,
  focusTo,
  linesNamed,
  marks,
  mockReview,
  phraseOf,
  reviewLog,
  seededRound,
  spAt,
  stopSp,
  stripMessage,
  turnOf,
} from "./harness";

// z on a real server: what each z takes back and says, the history across a reload, and the hand-back.

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;
// Short enough to pass in a test, long enough to press inside
const LONG_HOLD = { BIRTA_REVIEW_HOLD_MS: "6000" };
const SHORT_HOLD = { BIRTA_REVIEW_HOLD_MS: "300" };

const WITHDRAWN = "hand-back withdrawn · your notes are back";
const WITH_CLAUDE = "already with Claude · c adds a note";
const NOTHING = "nothing to undo";
const KEPT = "kept · z to undo";
const UNKEPT = "kept mark removed";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"] textarea');
const turn = (page: Page) => page.keyboard.press("ControlOrMeta+Enter");

/** Serves STOPS_DOC as pr.md on a real socket, and waits for review and its first state. */
async function openReal(page: Page, birta: Start, env: Record<string, string> = {}) {
  const served = await birta({ "pr.md": STOPS_DOC }, { ...GATE_ON, ...env });
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await ready(page);
  return served;
}

/** Review booted and holding a state. */
async function ready(page: Page) {
  await booted(page);
  await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
}

interface W {
  BR: { currentState(): State; op(f: State): string; pending: { unacked(): boolean } };
}

/** Writes a note on the focused stop with c, and waits for the composer to close. */
async function keep(page: Page, body: string) {
  await page.keyboard.press("c");
  await expect(composer(page)).toBeFocused();
  // A new note, never an edit of one already on the stop
  await expect(composer(page)).toHaveValue("");
  await page.keyboard.type(body);
  await page.keyboard.press("Enter");
  await expect(composer(page)).toHaveCount(0);
}

/** Waits until the page's state holds your draft `body`, so c edits it rather than starting a note. */
async function draftShown(page: Page, body: string) {
  await expect
    .poll(() =>
      page.evaluate(
        (body) => ((window as unknown as W).BR.currentState()?.items ?? []).some((i: State) => i.body === body && i.status === "draft"),
        body,
      ),
    )
    .toBe(true);
}

/** Moves focus to the block holding `text`. */
async function onBlock(page: Page, text: string) {
  await focusTo(page, await stopSp(page, text));
}

const lines = (root: string) => reviewLog(root) ?? [];
/** Every retract, amend and withdraw line of the log: what a z can write. */
const takebacks = (root: string) =>
  lines(root)
    .map((l, i) => ({ ...l, n: i + 1 }))
    .filter((l) => ["retract", "amend", "withdraw"].includes(l.op))
    .map((l) => ({ op: l.op, to: l.to, body: l.body, verdict: l.verdict }));

/** The note lines of the log, `{n, body}`. */
const notes = (root: string) => linesNamed(root, "comment").map((l) => ({ n: l.n, body: l.body ?? null }));

/** Presses z, and waits for the strip to say `want`. */
async function z(page: Page, want: string) {
  await page.keyboard.press("z");
  await expect.poll(() => stripMessage(page)).toBe(want);
}

/**
 * Routes this page's sockets to the real server, holding back each `review_state` by `ms`, so a
 * key can land before the page knows the state. Returns every op frame the page sent. Call before `goto`.
 */
async function slowState(page: Page, ms: number): Promise<State[]> {
  const sent: State[] = [];
  await page.routeWebSocket(/\/ws$/, (ws) => {
    const server = ws.connectToServer();
    ws.onMessage((raw) => {
      try {
        const msg = JSON.parse(raw.toString());
        if (msg.type === "op") sent.push(msg);
      } catch {
        // not a JSON frame
      }
      server.send(raw);
    });
    server.onMessage((raw) => {
      let msg: State = null;
      try {
        msg = JSON.parse(raw.toString());
      } catch {
        // not a JSON frame
      }
      if (msg && msg.type === "review_state") setTimeout(() => ws.send(raw), ms);
      else ws.send(raw);
    });
  });
  return sent;
}

/** A second tab on the same doc, with its own sessionStorage. */
async function otherTab(context: BrowserContext, url: string, storage?: Record<string, string>): Promise<Page> {
  const p = await context.newPage();
  await cacheCdn(p);
  await p.addInitScript(OPEN_ROOTS);
  if (storage) {
    await p.addInitScript((entries) => {
      for (const [k, v] of Object.entries(entries)) sessionStorage.setItem(k, v);
    }, storage);
  }
  await p.setViewportSize(WIDE);
  await p.goto(url);
  await ready(p);
  return p;
}

/** Hands back from `page` and waits for the hold to pass: the phrase says sent. */
async function handBack(page: Page) {
  await turn(page);
  await expect.poll(() => phraseOf(page), { timeout: 8_000 }).toMatch(/^sent /);
}

// --- What each z says ---------------------------------------------------------------------------

test.describe("each z names what it took back", () => {
  test("a note saved and taken back: note saved, then note removed, which replaces it", async ({ page, birta }) => {
    const served = await openReal(page, birta);
    await onBlock(page, "When the upload hits");
    await keep(page, "way too verbose");
    await expect.poll(() => stripMessage(page)).toBe("note saved · z to undo");
    await expect.poll(() => notes(served.root)).toHaveLength(1);
    const [note] = notes(served.root);
    await z(page, "note removed");
    await expect.poll(() => takebacks(served.root)).toEqual([{ op: "retract", to: note.n }]);
  });

  test("an edit undone: z puts the note's old words back, then a second z removes the note", async ({ page, birta }) => {
    const served = await openReal(page, birta);
    await onBlock(page, "When the upload hits");
    await keep(page, "say why");
    await expect.poll(() => notes(served.root)).toHaveLength(1);
    const [note] = notes(served.root);
    // c on a stop holding your unsent note edits it
    await draftShown(page, "say why");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("say why");
    await composer(page).fill("say why it fails");
    await page.keyboard.press("Enter");
    await expect.poll(() => stripMessage(page)).toBe("note updated · z to undo");
    await expect.poll(() => takebacks(served.root)).toEqual([{ op: "amend", to: note.n, body: "say why it fails" }]);

    await z(page, "edit undone");
    await expect.poll(() => takebacks(served.root).slice(1)).toEqual([{ op: "amend", to: note.n, body: "say why" }]);
    await expect(page.locator('birta-flow[data-kind="note"]', { hasText: "say why it fails" })).toHaveCount(0);
    await expect(page.locator('birta-flow[data-kind="note"]', { hasText: "say why" })).toHaveCount(1);

    await z(page, "note removed");
    await expect.poll(() => takebacks(served.root).slice(2)).toEqual([{ op: "retract", to: note.n }]);
    await z(page, NOTHING);
    await page.waitForTimeout(SETTLE_MS);
    expect(takebacks(served.root)).toHaveLength(3);
  });

  test('a verdict on a section: z says "no" removed from ## Stacked on', async ({ page, birta }) => {
    const served = await openReal(page, birta);
    await onBlock(page, "Stacked on");
    await page.keyboard.press("x");
    await expect.poll(() => stripMessage(page)).toBe("no · ## Stacked on (section) · z to undo");
    await expect.poll(() => notes(served.root)).toHaveLength(1);
    const [v] = notes(served.root);
    await z(page, '"no" removed from ## Stacked on');
    await expect.poll(() => takebacks(served.root)).toEqual([{ op: "retract", to: v.n }]);
  });

  test("z with nothing to take back says so for 4 s and writes nothing", async ({ page, birta }) => {
    const served = await openReal(page, birta);
    await page.keyboard.press("z");
    await expect.poll(() => stripMessage(page), { intervals: [50] }).toBe(NOTHING);
    const t0 = Date.now();
    await page.waitForTimeout(Math.max(0, t0 + 2_500 - Date.now()));
    expect(await stripMessage(page)).toBe(NOTHING);
    await expect.poll(() => stripMessage(page), { timeout: Math.max(1, t0 + 5_500 - Date.now()) }).not.toBe(NOTHING);
    expect(reviewLog(served.root)).toBeNull();
  });
});

// --- The history survives a reload ---------------------------------------------------------------

test.describe("the undo history survives a reload of the same tab", () => {
  test("reload, then z takes back your last note, then the one before it, then says nothing to undo", async ({ page, birta }) => {
    const served = await openReal(page, birta);
    await onBlock(page, "When the upload hits");
    await keep(page, "first");
    await onBlock(page, "Nothing yet.");
    await keep(page, "second");
    await expect.poll(() => notes(served.root)).toHaveLength(2);
    const [first, second] = notes(served.root);
    await page.reload();
    await ready(page);

    await z(page, "note removed");
    await expect.poll(() => takebacks(served.root)).toEqual([{ op: "retract", to: second.n }]);
    await z(page, "note removed");
    await expect.poll(() => takebacks(served.root)).toEqual([
      { op: "retract", to: second.n },
      { op: "retract", to: first.n },
    ]);
    await z(page, NOTHING);
    await page.waitForTimeout(SETTLE_MS);
    expect(takebacks(served.root)).toHaveLength(2);
  });

  test("reload, then z undoes an edit with the words from before it", async ({ page, birta }) => {
    const served = await openReal(page, birta);
    await onBlock(page, "When the upload hits");
    await keep(page, "say why");
    await expect.poll(() => notes(served.root)).toHaveLength(1);
    await draftShown(page, "say why");
    await page.keyboard.press("c");
    await expect(composer(page)).toHaveValue("say why");
    await composer(page).fill("say why it fails");
    await page.keyboard.press("Enter");
    await expect.poll(() => takebacks(served.root)).toHaveLength(1);
    const [note] = notes(served.root);
    await page.reload();
    await ready(page);
    await z(page, "edit undone");
    await expect.poll(() => takebacks(served.root).slice(1)).toEqual([{ op: "amend", to: note.n, body: "say why" }]);
  });

  test("another tab has none of it, even one that copied this tab's storage", async ({ page, birta, context }) => {
    const served = await openReal(page, birta);
    await onBlock(page, "When the upload hits");
    await keep(page, "mine");
    await expect.poll(() => notes(served.root)).toHaveLength(1);
    // A duplicated tab starts with a copy of the original's sessionStorage, but its own tab id
    const storage = await page.evaluate(() => {
      const out: Record<string, string> = {};
      for (let i = 0; i < sessionStorage.length; i++) {
        const k = sessionStorage.key(i)!;
        out[k] = sessionStorage.getItem(k)!;
      }
      return out;
    });
    const twin = await otherTab(context, `${served.url}/view/pr.md`, storage);
    await z(twin, NOTHING);
    await page.waitForTimeout(SETTLE_MS);
    expect(takebacks(served.root)).toEqual([]);
    // This tab still has it
    await page.bringToFront();
    await z(page, "note removed");
    await expect.poll(() => takebacks(served.root)).toEqual([{ op: "retract", to: notes(served.root)[0].n }]);
  });
});

test("in a folder each doc keeps its own history: z on another doc in the same tab reaches nothing of this one", async ({
  page,
  birta,
}) => {
  const served = await birta({ "pr.md": STOPS_DOC, "b.md": "# B\n\nOther words.\n" }, GATE_ON, ["."]);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await ready(page);
  await onBlock(page, "When the upload hits");
  await keep(page, "on pr");
  await expect.poll(() => notes(served.root)).toHaveLength(1);
  await page.goto(`${served.url}/view/b.md`);
  await ready(page);
  await z(page, NOTHING);
  await page.waitForTimeout(SETTLE_MS);
  expect(takebacks(served.root)).toEqual([]);
  await page.goto(`${served.url}/view/pr.md`);
  await ready(page);
  await z(page, "note removed");
  await expect.poll(() => takebacks(served.root)).toEqual([{ op: "retract", to: notes(served.root)[0].n }]);
});

// --- z and the hand-back ------------------------------------------------------------------------

test.describe("z and the hand-back", () => {
  test("z in the hold withdraws it, even after a note written during the hold, which stays", async ({ page, birta }) => {
    const served = await openReal(page, birta, LONG_HOLD);
    await onBlock(page, "When the upload hits");
    await keep(page, "before the hold");
    await turn(page);
    await expect.poll(() => stripMessage(page)).toMatch(/^handing back · /);
    await expect.poll(() => linesNamed(served.root, "submit")).toHaveLength(1);
    const submit = linesNamed(served.root, "submit")[0];
    // The hold is the top of the history until it ends
    await onBlock(page, "Nothing yet.");
    await keep(page, "during the hold");
    await expect.poll(() => notes(served.root)).toHaveLength(2);
    await page.keyboard.press("z");
    await expect.poll(() => takebacks(served.root)).toEqual([{ op: "withdraw", to: submit.n }]);
    // The copy for a z inside the hold is not in the spec: once the hold is gone it must say something, and not this
    await expect
      .poll(async () => {
        const m = await stripMessage(page);
        return m !== null && m !== NOTHING && !/^handing back/.test(m);
      })
      .toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    expect(takebacks(served.root)).toEqual([{ op: "withdraw", to: submit.n }]);
  });

  test("z after the hold and before pickup withdraws the hand-back, and your notes are back", async ({ page, birta }) => {
    const served = await openReal(page, birta, SHORT_HOLD);
    await onBlock(page, "When the upload hits");
    await keep(page, "way too verbose");
    await handBack(page);
    const submit = linesNamed(served.root, "submit")[0];
    await z(page, WITHDRAWN);
    await expect.poll(() => takebacks(served.root)).toEqual([{ op: "withdraw", to: submit.n }]);
    await expect.poll(() => turnOf(page)).toMatch(/^Hand back · 1( |$)/);
    expect(await phraseOf(page)).not.toMatch(/^sent /);
  });

  test("z after pickup says it is already with Claude and takes nothing back, before and after a reload", async ({
    page,
    birta,
  }) => {
    const served = await openReal(page, birta, SHORT_HOLD);
    await onBlock(page, "When the upload hits");
    await keep(page, "way too verbose");
    await handBack(page);
    const picked = agent(served.root, ["review", "pr.md", "--as", "claude"]);
    expect(picked.status, picked.stderr).toBe(0);
    await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toMatch(/^Claude picked up /);
    await z(page, WITH_CLAUDE);
    // A restored hand-back entry must not withdraw before the page knows of the pickup
    const sent = await slowState(page, 1_500);
    await page.reload();
    await booted(page);
    await page.keyboard.press("z");
    await ready(page);
    await z(page, WITH_CLAUDE);
    await page.waitForTimeout(SETTLE_MS);
    expect(sent.filter((f) => ["withdraw", "retract", "amend"].includes(f.op))).toEqual([]);
    expect(takebacks(served.root)).toEqual([]);
  });

  test("the history clears at pickup: after Claude's reply z reaches nothing it sent, only what you wrote since", async ({
    page,
    birta,
  }) => {
    const served = await openReal(page, birta, SHORT_HOLD);
    await onBlock(page, "When the upload hits");
    await keep(page, "say why");
    await handBack(page);
    const [sentNote] = notes(served.root);
    expect(agent(served.root, ["review", "pr.md", "--as", "claude"]).status).toBe(0);
    const responded = agent(served.root, ["respond", "pr.md", "--as", "claude"], `#${sentNote.n} done\n`);
    expect(responded.status, responded.stdout + responded.stderr).toBe(0);
    await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toMatch(/^Claude replied /);
    await z(page, NOTHING);
    await onBlock(page, "Nothing yet.");
    await keep(page, "one more");
    await expect.poll(() => notes(served.root)).toHaveLength(2);
    const fresh = notes(served.root)[1];
    await z(page, "note removed");
    await z(page, NOTHING);
    await page.waitForTimeout(SETTLE_MS);
    expect(takebacks(served.root)).toEqual([{ op: "retract", to: fresh.n }]);
  });
});

// --- The risk: a restored history must never take back what a hand-back took ---------------------

test.describe("a restored history and a hand-back made while the tab was away", () => {
  /**
   * Tab A writes a note (and edits it when `edit`), goes away, and a second tab hands it back.
   * `picked` lets an agent claim it, `replied` respond too. Tab A then comes back with its state
   * held back, so its first z lands before it knows the hand-back.
   */
  async function awayAndBack(page: Page, context: BrowserContext, birta: Start, how: { edit?: boolean; picked?: boolean; replied?: boolean }) {
    const served = await openReal(page, birta, SHORT_HOLD);
    const url = `${served.url}/view/pr.md`;
    await onBlock(page, "When the upload hits");
    await keep(page, "say why");
    await expect.poll(() => notes(served.root)).toHaveLength(1);
    const [note] = notes(served.root);
    if (how.edit) {
      await draftShown(page, "say why");
      await page.keyboard.press("c");
      await expect(composer(page)).toHaveValue("say why");
      await composer(page).fill("say why it fails");
      await page.keyboard.press("Enter");
      await expect.poll(() => takebacks(served.root)).toHaveLength(1);
    }
    // Every op acked, so the outbox replays nothing after the return and each frame then is a z's
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.pending.unacked())).toBe(false);
    await page.goto("about:blank");

    const other = await otherTab(context, url);
    await expect.poll(() => turnOf(other)).toMatch(/^Hand back · 1( |$)/);
    await handBack(other);
    await other.close();
    if (how.picked || how.replied) expect(agent(served.root, ["review", "pr.md", "--as", "claude"]).status).toBe(0);
    if (how.replied) {
      const r = agent(served.root, ["respond", "pr.md", "--as", "claude"], `#${note.n} done\n`);
      expect(r.status, r.stdout + r.stderr).toBe(0);
    }
    const before = takebacks(served.root);

    const sent = await slowState(page, 1_500);
    await page.goto(url);
    await booted(page);
    // Before the state lands
    await page.keyboard.press("z");
    await page.waitForTimeout(200);
    await page.keyboard.press("z");
    await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState()), { timeout: 5_000 }).toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    // And after it
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    return { served, note, before, sent };
  }

  const reaching = (sent: State[], n: number) =>
    sent.filter((f) => (f.op === "retract" || f.op === "amend") && f.to === n).map((f) => ({ op: f.op, to: f.to }));

  test("sent but not picked up: no z retracts the sent note, from the page or in the log", async ({ page, birta, context }) => {
    const { served, note, before, sent } = await awayAndBack(page, context, birta, {});
    expect(reaching(sent, note.n)).toEqual([]);
    expect(takebacks(served.root).slice(before.length).filter((l) => l.op !== "withdraw")).toEqual([]);
    expect(await stripMessage(page)).not.toBe("note removed");
    // The server would have taken that retract: only the page's check stood in the way
    await page.evaluate((n) => (window as unknown as W).BR.op({ op: "retract", to: n }), note.n);
    await expect.poll(() => linesNamed(served.root, "retract").map((l) => l.to)).toEqual([note.n]);
  });

  test("sent but not picked up: an edit's entry is no way back to the sent note either", async ({ page, birta, context }) => {
    const { served, note, before, sent } = await awayAndBack(page, context, birta, { edit: true });
    expect(reaching(sent, note.n)).toEqual([]);
    expect(takebacks(served.root).slice(before.length).filter((l) => l.op !== "withdraw")).toEqual([]);
  });

  test("picked up: z sends nothing that reaches the note", async ({ page, birta, context }) => {
    const { served, note, before, sent } = await awayAndBack(page, context, birta, { picked: true });
    expect(reaching(sent, note.n)).toEqual([]);
    expect(takebacks(served.root).slice(before.length)).toEqual([]);
    expect(await stripMessage(page)).toBe(WITH_CLAUDE);
  });

  test("replied: z sends nothing that reaches the note, and says nothing to undo", async ({ page, birta, context }) => {
    const { served, note, before, sent } = await awayAndBack(page, context, birta, { replied: true, edit: true });
    expect(reaching(sent, note.n)).toEqual([]);
    expect(takebacks(served.root).slice(before.length)).toEqual([]);
    expect(await stripMessage(page)).toBe(NOTHING);
  });
});

// --- A keep is part of the history too: z takes it back with an unkeep -------------------------

/** `state` with Claude's change `id` kept, as the fold shows it after a seen. */
function keptIn(state: State, id: string): State {
  const news = state.news.find((n: State) => n.id === id);
  news.check = "kept";
  const c = state.changes.find((x: State) => x.id === id);
  Object.assign(c, { check: "kept", seen: true });
  return state;
}

/** Writes a note on `## Why` and the state shows it as line 101, your draft; then y on the unit-test bullet. */
async function noteThenKeep(page: Page, birta: Start) {
  const NAME = "journey/round1";
  const served = await birta({ "pr.md": fixtureDoc(NAME) }, GATE_ON);
  await page.setViewportSize(WIDE);
  // The mock sends this object on every load, so the reload's first state holds what is pushed into it
  const withNote = fixtureState(NAME);
  const mock = await mockReview(page, { state: withNote });
  await page.goto(`${served.url}/view/pr.md`);
  await ready(page);
  const why = spAt(fixtureDoc(NAME), "## Why");
  await focusTo(page, why);
  await keep(page, "shorter");
  await expect.poll(() => mock.ops().filter((o) => o.op === "comment")).toHaveLength(1);
  // The state shows the note as the server would: line 101, your draft
  withNote.items.push({
    n: 101,
    op: "comment",
    by: { type: "human", via: "browser" },
    cid: mock.ops().find((o) => o.op === "comment")!.cid,
    ts: withNote.server_now,
    // Where it sits doesn't matter here; whether it is still yours does
    at: { kind: "doc", loc: null },
    body: "shorter",
    status: "draft",
    amended: [],
    replies: [],
  });
  withNote.counts.drafts = 1;
  mock.pushState(withNote);
  await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().items.some((i: State) => i.n === 101))).toBe(true);
  await focusTo(page, "16:1-16:62");
  await page.keyboard.press("y");
  await expect.poll(() => mock.ops().filter((o) => o.op === "seen").map((o) => o.ids)).toEqual([["10.6"]]);
  await expect.poll(() => stripMessage(page)).toBe(KEPT);
  await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.pending.unacked())).toBe(false);
  return { mock, withNote };
}

const unkeeps = (ops: State[]) => ops.filter((o) => o.op === "unkeep").map((o) => [...o.ids].sort());

test("a keep survives the reload: z unkeeps it, kept mark removed, and the next z takes back the note before it", async ({ page, birta }) => {
  const { mock, withNote } = await noteThenKeep(page, birta);
  // The keep landed, so the state the reload gets shows it
  keptIn(withNote, "10.6");
  await page.reload();
  await ready(page);
  mock.pushState(withNote);
  await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().items.some((i: State) => i.n === 101))).toBe(true);
  await z(page, UNKEPT);
  await expect.poll(() => unkeeps(mock.ops())).toEqual([["10.6"]]);
  await page.waitForTimeout(SETTLE_MS);
  expect(mock.ops().filter((o) => o.op === "retract")).toEqual([]);
  await z(page, "note removed");
  // The mock acked the note first, as line 101
  await expect.poll(() => mock.ops().filter((o) => o.op === "retract").map((o) => o.to)).toEqual([101]);
  expect(unkeeps(mock.ops())).toEqual([["10.6"]]);
});

test("a restored keep the state no longer shows as kept is dropped: z sends no unkeep and reaches the note", async ({ page, birta }) => {
  const { mock, withNote } = await noteThenKeep(page, birta);
  // Another tab took the keep back, or the log was reset: the state shows 10.6 unchecked
  await page.reload();
  await ready(page);
  mock.pushState(withNote);
  await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().items.some((i: State) => i.n === 101))).toBe(true);
  await z(page, "note removed");
  await page.waitForTimeout(SETTLE_MS);
  expect(unkeeps(mock.ops())).toEqual([]);
  expect(mock.ops().filter((o) => o.op === "retract").map((o) => o.to)).toEqual([101]);
});

/** The seeded round on a real server: Claude's change 7.0 on the paragraph, and its done on your note. */
async function openSeeded(page: Page, birta: Start, env: Record<string, string> = {}) {
  const s = seededRound(1);
  const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, { ...GATE_ON, ...env });
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await ready(page);
  await expect.poll(() => toCheck(page)).toBe(2);
  return { served, seeded: lines(served.root).length };
}

const CHANGED = "we just fail, and the user";
const toCheck = (page: Page) => page.evaluate(() => (window as unknown as W).BR.currentState()?.counts.to_check);
const checkOf = (page: Page, id: string) =>
  page.evaluate((id) => (window as unknown as W).BR.currentState()?.changes.find((c: State) => c.id === id)?.check ?? null, id);
/** The keeps and unkeeps written since the seeded lines, with their ids. */
const keeps = (root: string, seeded: number) =>
  lines(root)
    .slice(seeded)
    .filter((l) => l.op === "seen" || l.op === "unkeep")
    .map((l) => [l.op, l.ids]);

test.describe("z on a keep, on a real server", () => {
  test("y then z: kept mark removed, the row loses its kept tail, the bar is new again, and the file is review/2", async ({ page, birta }) => {
    const { served, seeded } = await openSeeded(page, birta);
    await onBlock(page, CHANGED);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe(KEPT);
    await expect.poll(() => checkOf(page, "7.0")).toBe("kept");
    await expect.poll(async () => (await changeRows(page)).map((r) => r.tail)).toContain("kept");
    expect(lines(served.root)[0].birta).toBe("review/1");
    await z(page, UNKEPT);
    await expect.poll(() => keeps(served.root, seeded)).toEqual([
      ["seen", ["7.0"]],
      ["unkeep", ["7.0"]],
    ]);
    expect(lines(served.root)[0].birta).toBe("review/2");
    await expect.poll(() => checkOf(page, "7.0")).toBe(null);
    await expect.poll(() => toCheck(page)).toBe(2);
    await expect.poll(async () => (await changeRows(page)).map((r) => r.tail)).not.toContain("kept");
    const sp = await stopSp(page, CHANGED);
    await expect.poll(async () => (await marks(page)).find((m) => m.mark === "bar" && m.sp === sp)?.check).toBe("new");
    // The unkeep is spent
    await z(page, NOTHING);
    await page.waitForTimeout(SETTLE_MS);
    expect(keeps(served.root, seeded)).toHaveLength(2);
  });

  test("y, z, y keeps it again, and a second z takes that keep back too", async ({ page, birta }) => {
    const { served, seeded } = await openSeeded(page, birta);
    await onBlock(page, CHANGED);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe(KEPT);
    await z(page, UNKEPT);
    await expect.poll(() => checkOf(page, "7.0")).toBe(null);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe(KEPT);
    await expect.poll(() => checkOf(page, "7.0")).toBe("kept");
    await z(page, UNKEPT);
    await expect.poll(() => keeps(served.root, seeded)).toEqual([
      ["seen", ["7.0"]],
      ["unkeep", ["7.0"]],
      ["seen", ["7.0"]],
      ["unkeep", ["7.0"]],
    ]);
    await expect.poll(() => checkOf(page, "7.0")).toBe(null);
  });

  test("in the hand-back hold z withdraws it first; the next z takes the keep back", async ({ page, birta }) => {
    const { served, seeded } = await openSeeded(page, birta, LONG_HOLD);
    await onBlock(page, "metrics dashboard");
    await keep(page, "shorter");
    await onBlock(page, CHANGED);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe(KEPT);
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.pending.unacked())).toBe(false);
    await turn(page);
    const since = (op: string) => lines(served.root).slice(seeded).filter((l) => l.op === op).length;
    await expect.poll(() => since("submit")).toBe(1);
    await expect.poll(() => stripMessage(page)).toMatch(/^handing back · /);
    await page.keyboard.press("z");
    await expect.poll(() => since("withdraw")).toBe(1);
    await expect.poll(async () => !/^handing back/.test((await stripMessage(page)) ?? "")).toBe(true);
    expect(since("unkeep")).toBe(0);
    await z(page, UNKEPT);
    await expect.poll(() => keeps(served.root, seeded)).toEqual([
      ["seen", ["7.0"]],
      ["unkeep", ["7.0"]],
    ]);
    // The note is still yours to take back
    await z(page, "note removed");
  });

  test("once Claude picked up and replied, z takes no keep back", async ({ page, birta }) => {
    const { served, seeded } = await openSeeded(page, birta, SHORT_HOLD);
    await onBlock(page, CHANGED);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe(KEPT);
    await onBlock(page, "metrics dashboard");
    await keep(page, "say why");
    await handBack(page);
    const sent = notes(served.root).at(-1)!;
    expect(agent(served.root, ["review", "pr.md", "--as", "claude"]).status).toBe(0);
    const responded = agent(served.root, ["respond", "pr.md", "--as", "claude"], `#${sent.n} done\n`);
    expect(responded.status, responded.stdout + responded.stderr).toBe(0);
    await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toMatch(/^Claude replied /);
    await z(page, NOTHING);
    await z(page, NOTHING);
    await page.waitForTimeout(SETTLE_MS);
    expect(keeps(served.root, seeded)).toEqual([["seen", ["7.0"]]]);
  });

  test("once an approve lands, z takes no keep back", async ({ page, birta }) => {
    const { served, seeded } = await openSeeded(page, birta, SHORT_HOLD);
    await onBlock(page, CHANGED);
    await page.keyboard.press("y");
    await expect.poll(() => stripMessage(page)).toBe(KEPT);
    // whole doc's reply too, so nothing is left to check and ⌘⏎ approves at once
    await page.keyboard.press("Home");
    await page.keyboard.press("y");
    await expect.poll(() => toCheck(page)).toBe(0);
    await turn(page);
    await expect.poll(() => lines(served.root).slice(seeded).filter((l) => l.op === "approve").length).toBe(1);
    await expect.poll(() => phraseOf(page), { timeout: 8_000 }).toMatch(/^approved /);
    await z(page, NOTHING);
    await page.waitForTimeout(SETTLE_MS);
    expect(keeps(served.root, seeded).filter(([op]) => op === "unkeep")).toEqual([]);
  });
});

test("no review script keeps the interim copy a keep stays kept", () => {
  const dir = path.join(REPO, "assets", "review");
  const hits = fs
    .readdirSync(dir)
    .filter((f) => f.endsWith(".js"))
    .filter((f) => fs.readFileSync(path.join(dir, f), "utf-8").includes("a keep stays kept"));
  expect(hits).toEqual([]);
});

// --- Repairs: notes while Claude works, folder notes, z before the state, a changed log ----------

test("a note written while Claude has the doc is yours to take back; then z says it's with Claude", async ({ page, birta }) => {
  const served = await openReal(page, birta, SHORT_HOLD);
  await onBlock(page, "When the upload hits");
  await keep(page, "say why");
  await handBack(page);
  expect(agent(served.root, ["review", "pr.md", "--as", "claude"]).status).toBe(0);
  await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toMatch(/^Claude picked up /);
  await onBlock(page, "Nothing yet.");
  await keep(page, "late thought");
  await expect.poll(() => notes(served.root)).toHaveLength(2);
  const [sentNote, late] = notes(served.root);
  await z(page, "note removed");
  await expect.poll(() => takebacks(served.root)).toEqual([{ op: "retract", to: late.n }]);
  await z(page, WITH_CLAUDE);
  await page.waitForTimeout(SETTLE_MS);
  expect(takebacks(served.root).filter((t) => t.to === sentNote.n)).toEqual([]);
});

test("a folder note is undone like any note: z retracts it in the folder's file, and no older note", async ({ page, birta }) => {
  const served = await birta({ "pr.md": STOPS_DOC, "b.md": "# B\n" }, GATE_ON, ["."]);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await ready(page);
  await onBlock(page, "When the upload hits");
  await keep(page, "on the doc");
  await expect.poll(() => notes(served.root)).toHaveLength(1);
  await page.locator(".file-header birta-phrase").click();
  await page.locator("birta-pop [role=menuitem]", { hasText: "note on this folder" }).click();
  await expect(composer(page)).toBeFocused();
  await composer(page).fill("on the folder");
  await composer(page).press("Enter");
  await expect.poll(() => linesNamed(served.root, "comment", "").length).toBe(1);
  const folderNote = linesNamed(served.root, "comment", "")[0];
  await expect.poll(() => stripMessage(page)).toBe("note saved · z to undo");
  await z(page, "note removed");
  await expect.poll(() => linesNamed(served.root, "retract", "").map((l) => l.to)).toEqual([folderNote.n]);
  await page.waitForTimeout(SETTLE_MS);
  expect(takebacks(served.root)).toEqual([]);
  // The next z reaches the doc's note
  await z(page, "note removed");
  await expect.poll(() => takebacks(served.root)).toEqual([{ op: "retract", to: notes(served.root)[0].n }]);
});

test("z pressed many times before a reload's state lands takes back one act, not one per press", async ({ page, birta }) => {
  const served = await openReal(page, birta);
  for (const [block, body] of [["When the upload hits", "one"], ["Nothing yet.", "two"], ["Detail words.", "three"]]) {
    await onBlock(page, block);
    await keep(page, body);
  }
  await expect.poll(() => notes(served.root)).toHaveLength(3);
  await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.pending.unacked())).toBe(false);
  const three = notes(served.root)[2];
  await slowState(page, 1_500);
  await page.reload();
  await booted(page);
  for (let i = 0; i < 3; i++) await page.keyboard.press("z");
  await ready(page);
  await expect.poll(() => takebacks(served.root)).toEqual([{ op: "retract", to: three.n }]);
  await page.waitForTimeout(1_000);
  expect(takebacks(served.root)).toEqual([{ op: "retract", to: three.n }]);
});

test("a restored entry whose line now holds another note is dropped, never taken back", async ({ page, birta }) => {
  const NAME = "drafts";
  const served = await birta({ "pr.md": fixtureDoc(NAME) }, GATE_ON);
  // Line 101 holds a note this tab never wrote, as after the log was reset and rewritten
  const state = fixtureState(NAME);
  state.items.push({ ...state.items[0], n: 101, cid: "another-tab", body: "not yours", status: "draft" });
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state });
  await page.goto(`${served.url}/view/pr.md`);
  await ready(page);
  await focusTo(page, "3:1-3:6");
  await keep(page, "mine");
  // The mock acks it as line 101
  await expect.poll(() => mock.ops().filter((o) => o.op === "comment")).toHaveLength(1);
  await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.pending.unacked())).toBe(false);
  await page.reload();
  await ready(page);
  await z(page, NOTHING);
  await page.waitForTimeout(SETTLE_MS);
  expect(mock.ops().filter((o) => o.op === "retract")).toEqual([]);
});
