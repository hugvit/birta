import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  SIZE_LABEL,
  State,
  booted,
  changeRows,
  fixtureDoc,
  fixtureState,
  focusMark,
  focusTo,
  focused,
  markKeys,
  mockReview,
  revOf,
  strip,
  withRev,
} from "./harness";

// The journey's first reply: Claude's write over the draft, folded from the same log as the journey's
const NAME = "journey/round1";
const DOC = fixtureDoc(NAME);
const DRAFT = fixtureDoc("journey/pr-draft");
const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;

// Stops of the handed-back text, as comrak numbers them
const WHY = "5:1-7:54";
const MERGED = "11:1-11:80";
const HONOR = "12:1-13:0";
const UNIT = "16:1-16:62";
// The removals: the Respect bullet after the merged bullet, then the Log bullet and the
// Stacked on section, both in the gap after Honor
const RESPECT = "10.2";
const LOG = "10.4";
const STACKED = "10.5";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for the state. */
async function open(page: Page, birta: Start, states: State[], doc = DOC) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { states });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
  return { served, mock };
}

interface W {
  BR: { currentState(): State };
}

/** The journey's reply with `edit` applied. */
function bent(edit?: (s: State) => void): State {
  const s = fixtureState(NAME);
  edit?.(s);
  return s;
}

const changeOf = (s: State, id: string) => s.changes.find((c: State) => c.id === id);

/** Presses j or k until focus is on the removal stop of change `id`. */
async function focusRemoval(page: Page, id: string, key: "j" | "k" = "j") {
  for (let i = 0; i < 40; i++) {
    const m = await focusMark(page);
    if (m?.target === "removal" && m.change === id) return;
    await page.keyboard.press(key);
  }
  throw new Error(`40 presses of ${key} never reached the removal stop of ${id}: ${JSON.stringify(await focusMark(page))}`);
}

/** Where focus is as `target change after`, or the block's sp. */
async function at(page: Page): Promise<string | null> {
  const m = await focusMark(page);
  if (!m) return null;
  return m.target === "removal" ? `removal ${m.change} after ${m.after}` : (m.sp ?? m.target);
}

/** The change rows under block `sp`. */
async function rowsAt(page: Page, sp: string) {
  return (await changeRows(page)).filter((r) => r.sp === sp);
}

/** The one change row on show: a removal's opens at its stop, and no other is open. */
async function onlyRow(page: Page) {
  await expect.poll(async () => (await changeRows(page)).length).toBe(1);
  return (await changeRows(page))[0];
}

const joined = (xs: string[]) => xs.join(" | ");

// --- Leads, word for word -----------------------------------------------------------------------

test.describe("leads", () => {
  const BLOCK_LEADS: [what: string, sp: string, lead: string, edit?: (s: State) => void][] = [
    ["a change answering a note", MERGED, 'Claude · because you said "way too verbose"'],
    [
      "one edit answering two notes",
      HONOR,
      'Claude · because you said "way too verbose" and "say why"',
      (s) => (changeOf(s, "10.3").asked = { kind: "cited", notes: [4, 5], answers: [] }),
    ],
    [
      "a change answering your answer",
      HONOR,
      'Claude · because you answered "Unreleased"',
      (s) => {
        // Your answer, line 30, which `#30 covers` cited for the change
        s.items.find((i: State) => i.n === 7).replies.push({
          n: 30,
          id: "#30",
          by: { type: "human", via: "browser" },
          kind: "reply",
          body: "Unreleased",
          ts: s.server_now - 1_000,
          seen: true,
        });
        changeOf(s, "10.3").asked = { kind: "cited", notes: [], answers: [30] };
      },
    ],
    [
      "a change flagged as unasked",
      HONOR,
      "Claude · not asked for: out of scope for the --since change",
      (s) => {
        Object.assign(changeOf(s, "10.3"), { asked: { kind: "unasked", notes: [] }, unasked_why: "out of scope for the --since change" });
      },
    ],
    [
      "a change that cites nothing",
      HONOR,
      "Claude · without a note",
      (s) => {
        changeOf(s, "10.3").asked = { kind: "unasked", notes: [] };
        delete changeOf(s, "10.3").unasked_why;
      },
    ],
  ];

  for (const [what, sp, lead, edit] of BLOCK_LEADS) {
    test(`${what}: ${lead}`, async ({ page, birta }) => {
      await open(page, birta, [bent(edit)]);
      await focusTo(page, sp);
      await expect.poll(async () => (await rowsAt(page, sp)).map((r) => r.lead)).toEqual([lead]);
      const row = (await rowsAt(page, sp))[0];
      expect(row.kind).toBe("receipt");
      expect(row.text).not.toMatch(SIZE_LABEL);
    });
  }

  const REMOVAL_LEADS: [id: string, lead: string][] = [
    [RESPECT, "Claude · not asked for: folded into the defaults bullet"],
    [LOG, "Claude · not asked for: too much detail for a PR description"],
    [STACKED, "Claude · because you said no to ## Stacked on"],
  ];

  for (const [id, lead] of REMOVAL_LEADS) {
    test(`a removal's lead at its stop: ${lead}`, async ({ page, birta }) => {
      await open(page, birta, [bent()]);
      await focusRemoval(page, id);
      const row = await onlyRow(page);
      expect(row.lead).toBe(lead);
      expect(row.kind).toBe("receipt");
      expect(row.text).not.toMatch(SIZE_LABEL);
    });
  }

  test("every change of the turn has its own row by substance, and none says how big it is", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    for (const sp of [MERGED, HONOR, UNIT]) {
      await focusTo(page, sp);
      await expect.poll(async () => (await rowsAt(page, sp)).length, sp).toBe(1);
      const row = (await rowsAt(page, sp))[0];
      expect(row.ins.length, `${sp}: something underlined`).toBeGreaterThan(0);
      expect(row.text, sp).not.toMatch(SIZE_LABEL);
    }
  });
});

// --- Substance --------------------------------------------------------------------------------

test.describe("substance", () => {
  test("merged bullets: both old items struck, the new one underlined, code rendered", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusTo(page, MERGED);
    await expect.poll(async () => (await rowsAt(page, MERGED)).length).toBe(1);
    const row = (await rowsAt(page, MERGED))[0];
    expect(row.del.length).toBe(2);
    expect(row.del[0]).toContain("Wrap UploadClient.put in a retry loop with exponential backoff and full jitter");
    expect(row.del[1]).toContain("Retry only on idempotent failure modes: network errors, 502, 503, 504, and 429");
    expect(joined(row.ins)).toContain("Retry on network errors, 429 and 502–504 with exponential backoff and jitter");
    expect(joined(row.ins)).not.toContain("Wrap");
    // Text is rendered, not raw: the code span is code, and no backtick shows
    expect(row.code).toContain("UploadClient.put");
    expect(row.substance ?? "").not.toContain("`");
    // List markers render as •
    expect(row.substance ?? "").toContain("•");
    expect(row.substance ?? "").not.toMatch(/(^|\s)- /);
  });

  test("under your note: ↳ Claude · done, also for the other note, with the why sentence underlined among the cuts", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusTo(page, WHY);
    // Your note, then Claude's row under it
    await expect
      .poll(async () => (await changeRows(page, ["note", "receipt"])).filter((r) => r.sp === WHY).map((r) => r.kind))
      .toEqual(["note", "receipt"]);
    const [note, row] = (await changeRows(page, ["note", "receipt"])).filter((r) => r.sp === WHY);
    expect(note.text).toContain("you · say why");
    expect(row.lead).toBe('↳ Claude · done · also for "way too verbose"');
    expect(joined(row.ins)).toContain("UploadClient.put makes a single attempt with no retry logic");
    expect(joined(row.del)).toContain("Uploads to the storage service occasionally fail with transient errors");
    expect(joined(row.del)).toContain("Today, when that happens, we just fail");
    expect(joined(row.del)).not.toContain("makes a single attempt");
    expect(joined(row.ins)).not.toContain("Uploads to the storage service");
    expect(row.text).not.toMatch(SIZE_LABEL);
  });

  test("under your note with no other note: just ↳ Claude · done", async ({ page, birta }) => {
    await open(page, birta, [bent((s) => (s.items.find((i: State) => i.n === 4).outcome.changes = ["10.1", "10.3", "10.6"]))]);
    await focusTo(page, WHY);
    await expect.poll(async () => (await rowsAt(page, WHY)).map((r) => r.lead)).toEqual(["↳ Claude · done"]);
  });

  test("a removal: removed: then its heading and first block, struck, then and 1 more block", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusRemoval(page, STACKED);
    const row = await onlyRow(page);
    expect(row.substance ?? "").toMatch(/^removed: ## Stacked on /);
    expect(joined(row.del)).toContain("## Stacked on");
    expect(joined(row.del)).toContain("#482 — extract UploadClient from the sync worker");
    expect(row.code).toContain("UploadClient");
    expect(row.substance ?? "").toMatch(/ and 1 more block$/);
    // The second block stays folded into the count
    expect(row.substance ?? "").not.toContain("#479");
    expect(joined(row.del)).not.toMatch(/Log each retry|Respect/);
  });

  test("a removal of more blocks counts them: and 2 more blocks", async ({ page, birta }) => {
    await open(page, birta, [bent((s) => (s.deletions.find((d: State) => d.change === STACKED).more = 2))]);
    await focusRemoval(page, STACKED);
    expect((await onlyRow(page)).substance ?? "").toMatch(/ and 2 more blocks$/);
  });

  test("a one-block removal is removed: and the block, struck, with no more line", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusRemoval(page, RESPECT);
    const row = await onlyRow(page);
    expect(row.substance).toBe("removed: • Respect Retry-After when the server sends it");
    expect(joined(row.del)).toContain("Respect Retry-After when the server sends it");
    expect(row.code).toEqual(["Retry-After"]);
  });

  test("an added block reads added; the block itself is the substance", async ({ page, birta }) => {
    // Honor as a bullet Claude added on its own
    const state = bent((s) => {
      Object.assign(changeOf(s, "10.3"), { old_text: "", asked: { kind: "unasked", notes: [] } });
      delete changeOf(s, "10.3").unasked_why;
    });
    await open(page, birta, [state]);
    await focusTo(page, HONOR);
    await expect.poll(async () => (await rowsAt(page, HONOR)).map((r) => [r.lead, r.substance])).toEqual([["Claude · without a note", "added"]]);
  });

  test("a block with two changes shows both in one row", async ({ page, birta }) => {
    const state = bent((s) => {
      s.changes.push({
        ...JSON.parse(JSON.stringify(changeOf(s, "10.3"))),
        id: "10.7",
        idx: 7,
        old_text: "- Honor `Retry-After`; defaults: 4 attempts, 200 ms base, 5 s cap\n",
        new_text: "- Honor `Retry-After`; defaults: four attempts, 200 ms base, 5 s cap\n",
      });
    });
    await open(page, birta, [state]);
    await focusTo(page, HONOR);
    await expect.poll(async () => (await rowsAt(page, HONOR)).length).toBe(1);
    await page.waitForTimeout(SETTLE_MS);
    const rows = await rowsAt(page, HONOR);
    expect(rows.length).toBe(1);
    expect(joined(rows[0].ins)).toContain("Honor Retry-After");
    expect(joined(rows[0].ins)).toContain("four");
    expect(joined(rows[0].del)).toContain("Defaults");
  });

  test("the rewrite row is gone: a big turn draws no rewrite row, and each change keeps its own", async ({ page, birta }) => {
    // Even a state that still names a rewrite gets no row for it
    const state = bent((s) => (s.rewrite = { turn: 9, pct: 0.65, blocks_changed: 11, blocks_total: 17 }));
    await open(page, birta, [state]);
    await page.waitForTimeout(SETTLE_MS);
    expect(await page.locator('birta-flow[data-kind="rewrite"]').count()).toBe(0);
    expect((await changeRows(page, ["rewrite", "reply", "turn_note", "receipt", "note"])).map((r) => r.text).join("\n")).not.toContain(
      "rewrote most of it",
    );
    await focusTo(page, MERGED);
    await expect.poll(async () => (await rowsAt(page, MERGED)).map((r) => r.lead)).toEqual(['Claude · because you said "way too verbose"']);
  });
});

// --- The whole-doc reply ------------------------------------------------------------------------

test.describe("whole-doc reply", () => {
  const doneRow = async (page: Page) =>
    (await changeRows(page, ["reply", "receipt"])).filter((r) => r.sp === null && r.text.startsWith("↳ Claude · done")).map((r) => r.text);

  test("whole doc's done counts its changes and says n walks them", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    expect(await focused(page)).toBe("doc");
    await expect.poll(() => doneRow(page)).toEqual(["↳ Claude · done · 4 changes · n walks them"]);
  });

  test("after a revert it counts from the current changes: 4 changes, 1 reverted by you", async ({ page, birta }) => {
    const first = bent();
    const { mock } = await open(page, birta, [first]);
    await expect.poll(() => doneRow(page)).toEqual(["↳ Claude · done · 4 changes · n walks them"]);
    // Your u on the merged bullet, line 30, as the server folds it
    mock.pushState(
      bent((s) => {
        Object.assign(changeOf(s, "10.1"), {
          undone_by: 30,
          check: "reverted",
          settled: true,
          seen: true,
          reversible: { ok: false, reason: "later_overlap" },
        });
      }),
    );
    await expect.poll(() => doneRow(page)).toEqual(["↳ Claude · done · 4 changes, 1 reverted by you"]);
  });
});

// --- Removals are stops -------------------------------------------------------------------------

test.describe("removal stops", () => {
  test("j reaches the removal in its gap, a stop of its own between the blocks", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusTo(page, MERGED);
    await page.keyboard.press("j");
    await expect.poll(() => at(page)).toBe(`removal ${RESPECT} after ${MERGED}`);
    const m = (await focusMark(page))!;
    expect(m.sp).toBeNull();
    await page.keyboard.press("j");
    await expect.poll(() => at(page)).toBe(HONOR);
    await page.keyboard.press("k");
    await expect.poll(() => at(page)).toBe(`removal ${RESPECT} after ${MERGED}`);
    await page.keyboard.press("k");
    await expect.poll(() => at(page)).toBe(MERGED);
  });

  test("two removals in one gap are two stops, reached by two j presses, in news order", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusTo(page, HONOR);
    await page.keyboard.press("j");
    await expect.poll(() => at(page)).toBe(`removal ${LOG} after ${HONOR}`);
    expect((await onlyRow(page)).lead).toBe("Claude · not asked for: too much detail for a PR description");
    await page.keyboard.press("j");
    await expect.poll(() => at(page)).toBe(`removal ${STACKED} after ${HONOR}`);
    expect((await onlyRow(page)).lead).toBe("Claude · because you said no to ## Stacked on");
    // Past the gap, the next block: ## Testing
    await page.keyboard.press("j");
    await expect.poll(async () => (await focusMark(page))?.target ?? "block").toBe("block");
    expect(await focused(page)).toBe("14:1-14:10");
    await page.keyboard.press("k");
    await expect.poll(() => at(page)).toBe(`removal ${STACKED} after ${HONOR}`);
    await page.keyboard.press("k");
    await expect.poll(() => at(page)).toBe(`removal ${LOG} after ${HONOR}`);
    await page.keyboard.press("k");
    await expect.poll(() => at(page)).toBe(HONOR);
  });

  test("the block above a gap opens only its own change, never the removals after it", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusTo(page, HONOR);
    await expect.poll(async () => (await changeRows(page)).map((r) => r.lead)).toEqual(['Claude · because you said "way too verbose"']);
    await page.waitForTimeout(SETTLE_MS);
    expect((await changeRows(page)).map((r) => r.lead)).toEqual(['Claude · because you said "way too verbose"']);
  });

  test("the strip names a removal stop: N of M · ## What › removed list item, and its keys", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusTo(page, MERGED);
    await expect.poll(async () => (await strip(page))?.where ?? "").toMatch(/^\d+ of \d+ · ## What › list item · Claude's change$/);
    const [n, m] = (await strip(page))!.where!.match(/^(\d+) of (\d+)/)!.slice(1).map(Number);
    await page.keyboard.press("j");
    await expect.poll(() => at(page)).toBe(`removal ${RESPECT} after ${MERGED}`);
    // The walk's stop after the merged bullet, counted among the same stops
    await expect.poll(async () => (await strip(page))?.where).toBe(`${n + 1} of ${m} · ## What › removed list item`);
    expect((await strip(page))!.keys).toBe("y keep the cut · u put it back · c reply · n next");
  });

  test("n walks onto a removal at its gap, not onto the block above it", async ({ page, birta }) => {
    await open(page, birta, [bent()]);
    await focusTo(page, MERGED);
    await page.keyboard.press("n");
    await expect.poll(() => at(page)).toBe(`removal ${RESPECT} after ${MERGED}`);
    await page.keyboard.press("n");
    await expect.poll(() => at(page)).toBe(HONOR);
    await page.keyboard.press("n");
    await expect.poll(() => at(page)).toBe(`removal ${LOG} after ${HONOR}`);
    await page.keyboard.press("n");
    await expect.poll(() => at(page)).toBe(`removal ${STACKED} after ${HONOR}`);
  });

  test("once settled a removal is no stop, and its tick stays", async ({ page, birta }) => {
    const state = bent((s) => {
      Object.assign(changeOf(s, RESPECT), { seen: true, settled: true });
      s.news = s.news.filter((n: State) => n.id !== RESPECT);
    });
    await open(page, birta, [state]);
    await focusTo(page, MERGED);
    await page.keyboard.press("j");
    await expect.poll(() => at(page)).toBe(HONOR);
    expect(await markKeys(page)).toContain(`tick after ${MERGED}`);
  });

  for (const [cut, id] of [
    ["- Respect `Retry-After` when the server sends it", RESPECT],
    ["- Log each retry at `debug` with attempt number and delay", LOG],
    ["## Stacked on", STACKED],
  ] as const) {
    test(`a focused block Claude cut hands focus to its own removal stop: ${id}`, async ({ page, birta }) => {
      // The draft's state first, then Claude's write and its reply
      const before = withRev(fixtureState("fresh"), revOf(DRAFT));
      const { served } = await open(page, birta, [before, bent()], DRAFT);
      const line = DRAFT.split("\n").findIndex((l) => l === cut) + 1;
      const sp = await page.evaluate(
        (line) => Array.from(document.querySelectorAll("#content [data-sourcepos]")).map((e) => e.getAttribute("data-sourcepos")!).find((p) => p.startsWith(`${line}:`)) ?? null,
        line,
      );
      expect(sp, cut).not.toBeNull();
      await focusTo(page, sp!);
      served.write("pr.md", DOC);
      await expect.poll(() => at(page), { timeout: 10_000 }).toBe(`removal ${id} after ${id === RESPECT ? MERGED : HONOR}`);
    });
  }
});

// --- Adversary --------------------------------------------------------------------------------

test.describe("repair", () => {
  test("a note with words on a block Claude cut draws no removal row at the block above, and its stop shows one row", async ({
    page,
    birta,
  }) => {
    const state = bent((s) => {
      // Your note on the Respect bullet, which Claude cut: it follows the change to its gap
      s.items.push({
        ...JSON.parse(JSON.stringify(s.items.find((i: State) => i.n === 5))),
        n: 30,
        cid: "c-30",
        body: "cut this",
        at: { kind: "anchor", status: "orphaned", scope: "block", quote: "- Respect `Retry-After` when the server sends it", loc: null, change: RESPECT },
        outcome: { done: true, no_change: false, changes: [RESPECT] },
      });
    });
    await open(page, birta, [state]);
    await focusTo(page, MERGED);
    await expect.poll(async () => (await changeRows(page)).map((r) => r.lead)).toEqual(['Claude · because you said "way too verbose"']);
    await page.waitForTimeout(SETTLE_MS);
    expect((await changeRows(page)).map((r) => r.lead)).toEqual(['Claude · because you said "way too verbose"']);
    await page.keyboard.press("j");
    await expect.poll(() => at(page)).toBe(`removal ${RESPECT} after ${MERGED}`);
    await page.waitForTimeout(SETTLE_MS);
    expect((await changeRows(page)).map((r) => r.substance)).toEqual(["removed: • Respect Retry-After when the server sends it"]);
  });

  test("two changes on one block with different reasons show both leads in one row", async ({ page, birta }) => {
    const state = bent((s) => {
      s.changes.push({
        ...JSON.parse(JSON.stringify(changeOf(s, "10.3"))),
        id: "10.7",
        idx: 7,
        asked: { kind: "unasked", notes: [] },
        unasked_why: "tidied the units",
        old_text: "- Honor `Retry-After`; defaults: 4 attempts, 200 ms base, 5 s cap\n",
        new_text: "- Honor `Retry-After`; defaults: four attempts, 200 ms base, 5 s cap\n",
      });
    });
    await open(page, birta, [state]);
    await focusTo(page, HONOR);
    await expect.poll(async () => (await rowsAt(page, HONOR)).length).toBe(1);
    const leads = () =>
      page.evaluate(() =>
        Array.from(document.querySelectorAll('birta-flow[data-kind="receipt"]'))
          .filter((h) => h.getClientRects().length)
          .flatMap((h) => Array.from(h.shadowRoot!.querySelectorAll('[data-part="lead"]')).map((l) => (l.textContent ?? "").replace(/\s+/g, " ").trim())),
      );
    await expect.poll(leads).toEqual(['Claude · because you said "way too verbose"', "Claude · not asked for: tidied the units"]);
    const row = (await rowsAt(page, HONOR))[0];
    expect(row.text.indexOf("four")).toBeGreaterThan(row.text.indexOf("not asked for: tidied the units"));
  });
});

test.describe("adversary", () => {
  test("a checked removal still in news stays a stop until it settles", async ({ page, birta }) => {
    // Kept with y, no hand-back since: walkable, though seen
    const state = bent((s) => {
      Object.assign(changeOf(s, RESPECT), { seen: true, check: "kept" });
      s.news.find((n: State) => n.id === RESPECT).check = "kept";
    });
    await open(page, birta, [state]);
    await focusTo(page, MERGED);
    await page.keyboard.press("j");
    await expect.poll(() => at(page)).toBe(`removal ${RESPECT} after ${MERGED}`);
  });

  test("whole doc's count is the note's own changes: reverting a change it didn't make leaves it", async ({ page, birta }) => {
    const { mock } = await open(page, birta, [bent()]);
    const doneRow = async () =>
      (await changeRows(page, ["reply", "receipt"])).filter((r) => r.sp === null && r.text.startsWith("↳ Claude · done")).map((r) => r.text);
    await expect.poll(doneRow).toEqual(["↳ Claude · done · 4 changes · n walks them"]);
    // 10.2 is the turn's, not one of note #4's four; folded as the server does after u
    mock.pushState(
      bent((s) => {
        Object.assign(changeOf(s, RESPECT), { undone_by: 30, check: "reverted", settled: true, seen: true, reversible: { ok: false, reason: "later_overlap" } });
        s.news.find((n: State) => n.id === RESPECT).check = "reverted";
      }),
    );
    // The revert has landed before the row is read again
    await expect.poll(() => page.evaluate(() => (window as unknown as W).BR.currentState().changes.find((c: State) => c.id === "10.2").undone_by)).toBe(30);
    await expect.poll(doneRow).toEqual(["↳ Claude · done · 4 changes · n walks them"]);
    // and one of its own, with the fields the server sets together
    mock.pushState(
      bent((s) => {
        Object.assign(changeOf(s, "10.6"), { undone_by: 31, check: "reverted", settled: true, seen: true });
        s.news.find((n: State) => n.id === "10.6").check = "reverted";
      }),
    );
    await expect.poll(doneRow).toEqual(["↳ Claude · done · 4 changes, 1 reverted by you"]);
  });

  for (const [what, line] of [
    ["the section's second block, folded into and 1 more block", "- #479 — add a storage proxy for local runs"],
    ["the section's first block, under its heading", "- #482 — extract `UploadClient` from the sync worker"],
  ] as const) {
    test(`a focused block Claude cut hands focus to its removal stop: ${what}`, async ({ page, birta }) => {
      const before = withRev(fixtureState("fresh"), revOf(DRAFT));
      const { served } = await open(page, birta, [before, bent()], DRAFT);
      const n = DRAFT.split("\n").findIndex((l) => l === line) + 1;
      expect(n, line).toBeGreaterThan(0);
      // The li itself: the list's first item shares its line with the ul
      const sp = await page.evaluate((n) => document.querySelector(`#content li[data-sourcepos^="${n}:"]`)?.getAttribute("data-sourcepos") ?? null, n);
      expect(sp, line).not.toBeNull();
      await focusTo(page, sp!);
      served.write("pr.md", DOC);
      await expect.poll(() => at(page), { timeout: 10_000 }).toBe(`removal ${STACKED} after ${HONOR}`);
    });
  }

  test("text from the log is only ever text in a row: no element comes of it", async ({ page, birta }) => {
    const X = '<img src=x onerror="window.__rowXss=1">';
    const state = bent((s) => {
      // the lead's note quote, the unasked why, the removal's head and Claude's new text
      s.items.find((i: State) => i.n === 4).body = `too long ${X}`;
      Object.assign(changeOf(s, "10.3"), { asked: { kind: "unasked", notes: [] }, unasked_why: `scope ${X}` });
      changeOf(s, RESPECT).unasked_why = `folded ${X}`;
      s.deletions.find((d: State) => d.change === STACKED).head = `## Stacked on ${X}\n\n- #482 — extract \`UploadClient\` from the sync worker`;
      changeOf(s, "10.6").new_text = `- [x] Unit tests for backoff timing ${X} and retryable status codes\n`;
    });
    await open(page, birta, [state]);
    const noElements = () =>
      page.evaluate(() => {
        const found: string[] = [];
        const walk = (root: ParentNode) =>
          root.querySelectorAll("*").forEach((e) => {
            if (["img", "script", "iframe", "svg", "object"].includes(e.localName)) found.push(e.localName);
            if (e.shadowRoot) walk(e.shadowRoot);
          });
        document.querySelectorAll("birta-flow").forEach((h) => walk(h.shadowRoot ?? h));
        return { found, fired: (window as unknown as { __rowXss?: number }).__rowXss ?? null };
      });
    const visits: [string, () => Promise<void>, string][] = [
      ["merged lead", () => focusTo(page, MERGED), MERGED],
      ["honor lead", () => focusTo(page, HONOR), HONOR],
      ["unit substance", () => focusTo(page, UNIT), UNIT],
    ];
    for (const [what, go, sp] of visits) {
      await go();
      await expect.poll(async () => (await rowsAt(page, sp)).length, what).toBe(1);
      expect((await rowsAt(page, sp))[0].text, what).toContain("<img src=x onerror=");
      expect(await noElements(), what).toEqual({ found: [], fired: null });
    }
    // From the top, as both removals sit above the last block visited
    await page.keyboard.press("Home");
    for (const id of [RESPECT, STACKED]) {
      await focusRemoval(page, id);
      const row = await onlyRow(page);
      expect(row.text, id).toContain("<img src=x onerror=");
      expect(await noElements(), id).toEqual({ found: [], fired: null });
    }
  });
});
