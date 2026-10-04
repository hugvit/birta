import { Page } from "@playwright/test";
import {
  test,
  expect,
  Box,
  GATE_ON,
  Rgb,
  STOPS_DOC,
  State,
  blockBox,
  booted,
  distance,
  fixtureDoc,
  fixtureState,
  focusTo,
  focusTop,
  focused,
  frameBox,
  hex,
  ink,
  laneState,
  marks,
  mockReview,
  near,
  pixelsOf,
  reviewLog,
  seededRound,
  shot,
  spAt,
  stopSp,
  stops,
  stripMessage,
  stripPlace,
  SIZE_LABEL,
  changeRows,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const NARROW = { width: 500, height: 900 };
const SETTLE_MS = 400;
const WHITE: Rgb = [255, 255, 255];
const MUTED = "rgb(89, 99, 110)";
const ACCENT = "rgb(9, 105, 218)";
const DANGER = "rgb(209, 36, 47)";
const SUCCESS = "rgb(26, 127, 55)";
const CARD = "rgb(246, 248, 250)";
const NEW_BAR = hex("#bf8700");
const AGENT = hex("#59636e");
/** The row keys, as their text reads with the chips' own words. */
const KEYS = "y keep · u revert · c note";
const KEYS_NO_REVERT = "y keep · c note";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

const FLOW = fixtureDoc("flow");
const UNSEEN = fixtureDoc("unseen");
/** flow.json's 19.0, flagged as unasked with its why. */
const UNASKED = "Claude · not asked for: while I was there";

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(
  page: Page,
  birta: Start,
  doc: string,
  state?: State,
  viewport: { width: number; height: number } = WIDE,
) {
  const served = await birta({ "pr.md": doc }, GATE_ON);
  await page.setViewportSize(viewport);
  const mock = await mockReview(page, { state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  if (state) await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
  return { served, mock };
}

/** A copy of fixture `name` with `edit` applied. */
function bent(name: string, edit: (s: State) => void): State {
  const s = fixtureState(name);
  edit(s);
  return s;
}

interface W {
  BR: {
    currentState(): State;
    flow: {
      openRow(): { id: string } | null;
      focusedRow(): { id: string } | null;
      focusStop(stop: { kind: string; id: string; sp: string | null }): boolean;
      open(kind: string, at: object, own: object | null, label?: string): void;
    };
  };
}

interface Host {
  kind: string | null;
  text: string;
  /** The words of each `kbd` chip, in order. */
  kbd: string[];
  /** The block the host follows (or the item it closes), null in the doc slot. */
  sp: string | null;
}

/** Every `birta-flow` host in document order, read through its (forced open) root. */
function hosts(page: Page): Promise<Host[]> {
  return page.evaluate(() =>
    // The whole doc stop's own line is no row
    Array.from(document.querySelectorAll('birta-flow:not([data-kind="doc-stop"])')).map((h) => {
      const root = h.shadowRoot!;
      const text = Array.from(root.children)
        .filter((c) => c.localName !== "style")
        .map((c) => (c as HTMLElement).innerText ?? c.textContent ?? "")
        .join("\n")
        .replace(/[ \t]+/g, " ")
        .trim();
      let sp: string | null = null;
      if (h.parentElement!.localName === "li") sp = h.parentElement!.getAttribute("data-sourcepos");
      else {
        let prev = h.previousElementSibling;
        while (prev && prev.localName === "birta-flow") prev = prev.previousElementSibling;
        sp = prev && document.getElementById("content")!.contains(prev) ? prev.getAttribute("data-sourcepos") : null;
      }
      return {
        kind: h.getAttribute("data-kind"),
        text,
        kbd: Array.from(root.querySelectorAll("kbd")).map((k) => k.textContent ?? ""),
        sp,
      };
    }),
  );
}

async function hostWith(page: Page, has: string): Promise<Host | null> {
  return (await hosts(page)).find((h) => h.text.includes(has)) ?? null;
}

/** The shown change rows whose lead is `lead`. */
async function rowsLed(page: Page, lead: string) {
  return (await changeRows(page)).filter((r) => r.lead === lead);
}

const openRow = (page: Page) => page.evaluate(() => (window as unknown as W).BR.flow.openRow()?.id ?? null);
/** The row on show for the focus, muted or not. */
const focusedRow = (page: Page) => page.evaluate(() => (window as unknown as W).BR.flow.focusedRow()?.id ?? null);

/** Presses j or k until `sp` has focus. */
async function focusOn(page: Page, sp: string) {
  if (!(await stops(page)).some((s) => s.sp === sp)) throw new Error(`${sp} is not a stop`);
  await focusTo(page, sp);
}

/** Presses n until the open row is change `id`. */
async function walkTo(page: Page, id: string) {
  for (let i = 0; i < 24 && (await openRow(page)) !== id; i++) await page.keyboard.press("n");
  await expect.poll(() => openRow(page)).toBe(id);
}

/** Computed styles of the element holding the first text node, in any flow host, that contains `text`. */
function styleOf(page: Page, text: string, props: string[]): Promise<Record<string, string> | null> {
  return page.evaluate(
    ({ text, props }) => {
      for (const h of document.querySelectorAll("birta-flow")) {
        const walker = document.createTreeWalker(h.shadowRoot!, NodeFilter.SHOW_TEXT);
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
          if (n.parentElement?.closest("style") || !(n.textContent ?? "").includes(text)) continue;
          const cs = getComputedStyle(n.parentElement!);
          return Object.fromEntries(props.map((p) => [p, cs.getPropertyValue(p)]));
        }
      }
      return null;
    },
    { text, props },
  );
}

const dots = async (page: Page) => (await marks(page)).filter((m) => m.mark === "dot");

// --- Rows only on focus (C2, C9) --------------------------------------------------------------

test.describe("rows on focus", () => {
  test("a change's row shows only while its block has focus, and reads why it wasn't asked for", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await page.waitForTimeout(SETTLE_MS);
    expect(await rowsLed(page, UNASKED)).toEqual([]);
    await focusOn(page, "14:1-14:21");
    await expect.poll(async () => (await rowsLed(page, UNASKED)).map((r) => [r.sp, r.kbd])).toEqual([["14:1-14:21", ["y", "u", "c"]]]);
    const row = (await rowsLed(page, UNASKED))[0];
    expect(row.text.endsWith(KEYS)).toBe(true);
    expect(row.text).not.toMatch(SIZE_LABEL);
    // `## Stacked on` → `## Stacked on nothing`: the added word is underlined
    expect(row.ins.join(" ")).toContain("nothing");
    // Only the lead is muted on a row you can act on
    expect((await styleOf(page, "nothing", ["color"]))?.color).not.toBe(MUTED);
    await page.keyboard.press("j");
    await expect.poll(() => rowsLed(page, UNASKED)).toEqual([]);
  });

  test("the row's keys name the keys as rebound, and a state naming a rewrite draws no rewrite row", async ({ page, birta }) => {
    await open(page, birta, FLOW, bent("flow", (s) => (s.rewrite = { turn: 12, pct: 0.8, blocks_changed: 8, blocks_total: 10 })));
    await page.evaluate(() => {
      const kb = (window as unknown as { KEYBINDINGS: Record<string, string> }).KEYBINDINGS;
      Object.assign(kb, { yes: "g", revert_change: "v", note: "b" });
    });
    await focusOn(page, "14:1-14:21");
    await expect.poll(async () => (await rowsLed(page, UNASKED)).map((r) => r.kbd)).toEqual([["g", "v", "b"]]);
    const row = (await rowsLed(page, UNASKED))[0];
    expect(row.text.endsWith("g keep · v revert · b note")).toBe(true);
    expect(row.text).not.toMatch(SIZE_LABEL);
    expect(await hostWith(page, "rewrote most of it")).toBeNull();
    expect(await page.locator('birta-flow[data-kind="rewrite"]').count()).toBe(0);
  });

  test("a change that cites your answer says what you answered", async ({ page, birta }) => {
    // #25 is your answer to Claude's question 22; `#25 covers` credits change 19.0
    await open(page, birta, FLOW, bent("flow", (s) => (s.changes.find((c: State) => c.id === "19.0").asked = { kind: "cited", notes: [], answers: [25] })));
    await focusOn(page, "14:1-14:21");
    const lead = 'Claude · because you answered "yes, keep it"';
    await expect.poll(async () => (await rowsLed(page, lead)).map((r) => [r.sp, r.kbd])).toEqual([["14:1-14:21", ["y", "u", "c"]]]);
    expect((await rowsLed(page, lead))[0].text).not.toMatch(SIZE_LABEL);
  });

  test("a cited change's row says what you said, and shows the words that changed", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await focusOn(page, "5:1-6:68");
    const lead = 'Claude · because you said "say why it fails"';
    await expect.poll(async () => (await rowsLed(page, lead)).map((r) => [r.sp, r.kbd])).toEqual([["5:1-6:68", ["y", "u", "c"]]]);
    const row = (await rowsLed(page, lead))[0];
    expect(row.text.endsWith(KEYS)).toBe(true);
    expect(row.text).not.toMatch(SIZE_LABEL);
    expect(row.ins.join(" ")).toContain("S3 answers 503 under load");
    expect(row.del.join(" ")).toContain("with backoff");
  });

  test("Claude's done row under your note opens with its block's focus, its substance and keys with it", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    // Your note shows; Claude's row under it opens only on focus
    await expect.poll(async () => (await hostWith(page, "cap it"))?.sp).toBe("11:1-11:44");
    await page.waitForTimeout(SETTLE_MS);
    expect(await rowsLed(page, "↳ Claude · done")).toEqual([]);
    await focusOn(page, "11:1-11:44");
    await expect.poll(async () => (await rowsLed(page, "↳ Claude · done")).map((r) => [r.sp, r.kbd])).toEqual([["11:1-11:44", ["y", "u", "c"]]]);
    const row = (await rowsLed(page, "↳ Claude · done"))[0];
    expect(row.text.endsWith(KEYS)).toBe(true);
    expect(row.text).not.toMatch(SIZE_LABEL);
    expect(row.ins.join(" ")).toContain("capped at 8");
  });

  test("a change that can't be reverted offers y keep and c note", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await focusOn(page, "8:1-8:15");
    await expect.poll(() => hostWith(page, "changed outside birta")).toMatchObject({ kbd: ["y", "c"], sp: "8:1-8:15" });
    const text = (await hostWith(page, "changed outside birta"))!.text;
    expect(text).toMatch(/^changed outside birta \(git\)/);
    expect(text.endsWith(KEYS_NO_REVERT)).toBe(true);
    expect(text).not.toMatch(SIZE_LABEL);
  });

  test("keys are kbd chips on the open row only, in one group that never wraps", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"), NARROW);
    await focusOn(page, "5:1-6:68");
    await expect.poll(async () => (await hostWith(page, "because you said"))?.kbd).toEqual(["y", "u", "c"]);
    expect((await hosts(page)).filter((h) => h.kbd.length && !h.text.includes("because you said"))).toEqual([]);
    const tops = await page.evaluate(() => {
      for (const h of document.querySelectorAll("birta-flow")) {
        const ks = Array.from(h.shadowRoot!.querySelectorAll("kbd"));
        if (ks.length) return ks.map((k) => Math.round(k.getBoundingClientRect().top));
      }
      return [];
    });
    expect(new Set(tops).size, `${tops}`).toBe(1);
  });

  test("openRow names the open row's change, follows j and k, and is null with none open", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    expect(await openRow(page)).toBeNull();
    await focusOn(page, "14:1-14:21");
    await expect.poll(() => openRow(page)).toBe("19.0");
    await page.keyboard.press("j");
    await expect.poll(() => openRow(page)).toBe("27.0");
    await page.keyboard.press("k");
    await expect.poll(() => openRow(page)).toBe("19.0");
    await expect.poll(async () => (await hostWith(page, "not asked for"))?.kbd).toEqual(["y", "u", "c"]);
    // Esc keeps the focus and its row; whole doc holds no change
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(await openRow(page)).toBe("19.0");
    await focusTop(page);
    expect(await focused(page)).toBe("doc");
    await expect.poll(() => openRow(page)).toBeNull();
    await expect.poll(() => hostWith(page, "not asked for")).toBeNull();
  });

  test("n opens the row of the change it lands on, and only that row", async ({ page, birta }) => {
    const doc = fixtureDoc("fresh");
    const a = spAt(doc, "- retries on 503");
    const b = spAt(doc, "# Upload retries");
    await open(page, birta, doc, laneState(doc, { bars: [{ sp: a, asked: "unasked" }, { sp: b, asked: "unasked" }] }));
    await page.keyboard.press("n");
    await expect.poll(() => openRow(page)).not.toBeNull();
    const id = await openRow(page);
    const rows = (await hosts(page)).filter((h) => h.text.includes("without a note"));
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ sp: id === "20.0" ? a : b, kbd: ["y", "u", "c"] });
    expect(rows[0].text).not.toMatch(SIZE_LABEL);
  });

  test("n on a block with two changes goes on to the next stop, not to the block's second change", async ({ page, birta }) => {
    const state = bent("flow", (s) => {
      const c = s.changes.find((c: State) => c.id === "19.0");
      s.changes.push({ ...JSON.parse(JSON.stringify(c)), id: "19.1", idx: 1, label: "1 word changed" });
      const at = s.news.findIndex((n: State) => n.id === "19.0");
      s.news.splice(at + 1, 0, { id: "19.1", kind: "change", sp: "14:1-14:21", check: null });
    });
    await open(page, birta, FLOW, state);
    await walkTo(page, "19.0");
    // Both changes on the block are one row, with one set of keys
    const keyed = async () => (await changeRows(page)).filter((r) => r.sp === "14:1-14:21" && r.kbd.length).map((r) => r.lead);
    await expect.poll(keyed).toEqual([UNASKED]);
    expect((await changeRows(page)).filter((r) => r.sp === "14:1-14:21")).toHaveLength(1);
    // One stop, one landing: the next n leaves the block for 27.0's
    await page.keyboard.press("n");
    await expect.poll(() => openRow(page)).toBe("27.0");
    expect(await focused(page)).toBe("16:1-16:19");
    await expect.poll(keyed).toEqual([]);
  });

  test("j after n closes the row n opened", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await walkTo(page, "19.0");
    await page.keyboard.press("j");
    await expect.poll(() => openRow(page)).toBe("27.0");
    await expect.poll(() => hostWith(page, "without a note")).toBeNull();
  });

  test("an open row stays open, with its keys, across a state push", async ({ page, birta }) => {
    const state = fixtureState("flow");
    const { mock } = await open(page, birta, FLOW, state);
    await focusOn(page, "14:1-14:21");
    await expect.poll(() => openRow(page)).toBe("19.0");
    mock.pushState({ ...JSON.parse(JSON.stringify(state)), server_now: state.server_now + 1_000 });
    await page.waitForTimeout(SETTLE_MS);
    expect(await openRow(page)).toBe("19.0");
    expect((await hostWith(page, "not asked for"))?.kbd).toEqual(["y", "u", "c"]);
  });

  test("an earlier round's settled change opens muted, with no keys, and is no open row for keys", async ({ page, birta }) => {
    // Kept, then settled by a hand-back: it has left Claude's news
    const state = bent("flow", (s) => {
      Object.assign(s.changes.find((c: State) => c.id === "19.0"), { seen: true, settled: true, check: "kept" });
      s.news = s.news.filter((n: State) => n.id !== "19.0");
      s.counts.to_check -= 1;
    });
    await open(page, birta, FLOW, state);
    await focusOn(page, "14:1-14:21");
    await expect.poll(async () => (await rowsLed(page, UNASKED)).map((r) => r.kbd)).toEqual([[]]);
    expect((await rowsLed(page, UNASKED))[0].text).not.toMatch(SIZE_LABEL);
    // The whole row is muted, its substance too
    expect((await styleOf(page, "nothing", ["color"]))?.color).toBe(MUTED);
    expect(await openRow(page)).toBeNull();
    expect(await focusedRow(page)).toBe("19.0");
  });
});

// --- A mouse opens rows too -------------------------------------------------------------------

/** Clicks the middle of the first text in block `sp`. */
async function clickText(page: Page, sp: string) {
  const at = await page.evaluate((sp) => {
    const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
    const el = all[all.length - 1]!;
    el.scrollIntoView({ block: "center" });
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => ((n.textContent ?? "").trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP),
    });
    const r = document.createRange();
    r.selectNodeContents(walker.nextNode()!);
    const b = r.getClientRects()[0];
    return { x: b.left + Math.min(20, b.width / 2), y: b.top + b.height / 2 };
  }, sp);
  await page.mouse.click(at.x, at.y);
}

test.describe("click", () => {
  test("a click on a block focuses it and opens its rows, and a click on another moves them", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await clickText(page, "14:1-14:21");
    await expect.poll(() => focused(page)).toBe("14:1-14:21");
    await expect.poll(async () => (await rowsLed(page, UNASKED)).map((r) => [r.sp, r.kbd])).toEqual([["14:1-14:21", ["y", "u", "c"]]]);
    expect((await rowsLed(page, UNASKED))[0].text).not.toMatch(SIZE_LABEL);
    expect(await openRow(page)).toBe("19.0");
    await clickText(page, "16:1-16:19");
    await expect.poll(() => focused(page)).toBe("16:1-16:19");
    await expect.poll(() => rowsLed(page, UNASKED)).toEqual([]);
    await expect.poll(async () => (await hostWith(page, "changed outside birta"))?.sp).toBe("16:1-16:19");
  });

  test("a click on a list item focuses the item it lands in, not its list", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await clickText(page, "11:1-11:44");
    await expect.poll(() => focused(page)).toBe("11:1-11:44");
  });

  test("a click on a link moves no focus, and a drag that selects words focuses their block", async ({ page, birta }) => {
    const doc = "# T\n\nSee [the end](#end) here.\n\nOther words to select.\n\n## End\n\nDone.\n";
    await open(page, birta, doc, laneState(doc, {}));
    const link = page.locator("#content a", { hasText: "the end" });
    await link.click();
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe("doc");
    expect(await page.evaluate(() => location.hash)).toBe("#end");
    const words = (await page.locator("#content p", { hasText: "Other words" }).boundingBox())!;
    await page.mouse.move(words.x + 2, words.y + words.height / 2);
    await page.mouse.down();
    await page.mouse.move(words.x + words.width - 4, words.y + words.height / 2, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(SETTLE_MS);
    expect(await page.evaluate(() => getSelection()!.toString())).toContain("words");
    // A selection outranks the stop
    await expect.poll(() => focused(page)).toBe("5:1-5:22");
  });

  test("before review starts in the tab, a click on a doc with no review file moves no focus", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    await clickText(page, await stopSp(page, "When the upload hits"));
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe("doc");
  });
});

// --- The reverted row (C2, #7) ----------------------------------------------------------------

test.describe("reverted row", () => {
  /** flow.json after `u` on 13.0: the change undone by your write 40, which put the words back. */
  function reverted(kept: boolean): State {
    return bent("flow", (s) => {
      const c = s.changes.find((c: State) => c.id === "13.0");
      Object.assign(c, { undone_by: 40, settled: true, seen: true, check: "reverted", reversible: { ok: false, reason: "later_overlap" } });
      if (!kept) c.range = { ...c.range, status: "superseded", sp: null };
      // Reverted is a check: the change stays news, on the block your revert put back, until a hand-back
      s.news.find((n: State) => n.id === "13.0").check = "reverted";
      s.counts.to_check -= 1;
      s.changes.push({
        ...JSON.parse(JSON.stringify(c)),
        id: "40.0",
        op: 40,
        turn: null,
        attr: "human",
        by: { type: "human", via: "browser" },
        asked: { kind: "human", notes: [] },
        undone_by: undefined,
        check: null,
        settled: false,
        seen: false,
        range: { status: "ok", lines: [5, 6], bytes: [99, 167], sp: "5:1-6:68" },
        reversible: { ok: true, reason: "none" },
      });
      // A sent yes on the paragraph, so a glyph sits beside the reverted row's rule
      const like = JSON.parse(JSON.stringify(s.items.find((i: State) => i.n === 7)));
      s.items.push({
        ...like,
        n: 41,
        cid: "c-41",
        verdict: "yes",
        body: undefined,
        status: "sent",
        at: { kind: "anchor", status: "ok", scope: "block", quote: "When", loc: { sp: "5:1-6:68" } },
      });
    });
  }

  for (const kept of [false, true]) {
    test(`a reverted change keeps its lead and its words, Claude's underlined, ends reverted by you, with no size and no keys${kept ? ", where it still stands" : ""}`, async ({
      page,
      birta,
    }) => {
      await open(page, birta, FLOW, reverted(kept));
      await focusOn(page, "5:1-6:68");
      await expect.poll(() => hostWith(page, "reverted by you")).toMatchObject({ kbd: [], sp: "5:1-6:68" });
      const row = (await changeRows(page)).find((r) => /^reverted by you\b/.test(r.tail ?? ""))!;
      expect(row.lead).toBe('Claude · because you said "say why it fails"');
      expect(row.tail).toBe("reverted by you");
      expect(row.ins.join(" ")).toContain("S3 answers 503 under load; retrying three times fixes most of these.");
      expect(row.text).not.toContain("Claude's version");
      expect(row.text).not.toMatch(SIZE_LABEL);
      // Your own write that reverted it is no row of its own
      expect((await hosts(page)).filter((h) => h.sp === "5:1-6:68")).toHaveLength(1);
      expect(await openRow(page)).toBeNull();
      expect(await focusedRow(page)).toBe("13.0");
      // The glyph on its block keeps 4 px from the row's rule
      await page.waitForTimeout(SETTLE_MS);
      const ms = await marks(page);
      const g = ms.find((m) => m.mark === "glyph" && m.sp === "5:1-6:68")!;
      const rules = ms.filter((m) => m.mark === "rule");
      expect(rules.length).toBeGreaterThan(0);
      for (const r of rules)
        expect(r.left < g.right + 4 && r.right > g.left - 4 && r.top < g.bottom + 4 && r.bottom > g.top - 4, `${r.top} vs ${g.top}`).toBe(false);
    });
  }

  test("z of the revert brings the change's own row back, keys and all", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW, reverted(false));
    await focusOn(page, "5:1-6:68");
    await expect.poll(async () => (await hostWith(page, "reverted by you"))?.text).toMatch(/\breverted by you\b/);
    mock.pushState(fixtureState("flow"));
    const lead = 'Claude · because you said "say why it fails"';
    await expect.poll(async () => (await rowsLed(page, lead)).map((r) => r.kbd)).toEqual([["y", "u", "c"]]);
    expect((await rowsLed(page, lead))[0].text.endsWith(KEYS)).toBe(true);
    expect(await hostWith(page, "reverted by you")).toBeNull();
    expect(await openRow(page)).toBe("13.0");
  });
});

test.describe("reverted row on a real server", () => {
  /** Serves a round whose agent rewrote one line with no note, and walks onto that change. */
  async function onChange(page: Page, birta: Start) {
    const s = seededRound(1);
    const served = await birta({ "pr.md": s.doc, "pr.md.birta.jsonl": s.log }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    // The state comes after the page boots
    await expect.poll(() => page.evaluate(() => !!(window as unknown as W).BR.currentState())).toBe(true);
    const c = await page.evaluate(() => (window as unknown as W).BR.currentState().changes[0]);
    await walkTo(page, c.id);
    const sp = (await focused(page))!;
    expect(sp).toMatch(/^5:1-/);
    return { served, c, sp };
  }

  test("u keeps focus on the block, shows the reverted row at once, and pills what it did", async ({ page, birta }) => {
    const { served, c } = await onChange(page, birta);
    await page.keyboard.press("u");
    await expect.poll(() => reviewLog(served.root)!.filter((l) => l.op === "edit").length).toBe(1);
    await expect.poll(() => hostWith(page, "reverted by you")).toMatchObject({ kbd: [] });
    const row = (await changeRows(page)).find((r) => /^reverted by you\b/.test(r.tail ?? ""))!;
    expect(row.tail).toBe("reverted by you");
    // Claude's words, underlined where they differ, are its version
    expect(row.ins.join(" ")).toContain("starts");
    expect(row.text).not.toContain("Claude's version");
    expect(row.text).not.toMatch(SIZE_LABEL);
    expect(row.sp).toMatch(/^5:1-/);
    expect(await focused(page)).toMatch(/^5:1-/);
    await expect.poll(() => stripMessage(page)).toBe("reverted · u restores Claude's text");
  });

  test("the reverted change stays a walk stop until the round ends, and its wash clears once reverted", async ({ page, birta }) => {
    const { served, c } = await onChange(page, birta);
    await page.keyboard.press("u");
    await expect.poll(() => reviewLog(served.root)!.filter((l) => l.op === "edit").length).toBe(1);
    await expect.poll(async () => (await page.evaluate(() => (window as unknown as W).BR.currentState().changes)).length).toBe(2);
    // Off the walk's last landing, so n goes on from the focus
    await focusTop(page);
    await page.keyboard.press("n");
    await expect.poll(() => focused(page)).toMatch(/^5:1-/);
    await expect.poll(async () => (await hostWith(page, "reverted by you"))?.text).toMatch(/\breverted by you\b/);
    expect((await hostWith(page, "reverted by you"))!.text).not.toContain(c.label);
    // Whole doc holds Claude's done on your note, so the change is the second stop
    await expect.poll(() => stripPlace(page)).toBe("2 of 2");
    await page.waitForTimeout(SETTLE_MS);
    expect((await marks(page)).filter((m) => m.mark === "wash" && m.width > 0)).toEqual([]);
  });
});

// --- Tombstones (C7, N10) ---------------------------------------------------------------------

test.describe("tombstones", () => {
  /** unseen.json where a bare x on ## Stacked on asked for the section's removal, and change 11.0 did it. */
  function bareSectionNo(): State {
    return bent("unseen", (s) => {
      Object.assign(s.changes.find((c: State) => c.id === "11.0"), { asked: { kind: "location", notes: [30] }, label: "section removed · 2 blocks" });
      Object.assign(s.deletions.find((d: State) => d.change === "11.0"), {
        first_line: "## Stacked on",
        label: "section · 2 blocks",
        head: "## Stacked on\n\nNothing yet.",
        more: 0,
      });
      s.items.push({
        n: 30,
        op: "comment",
        by: { type: "human", via: "browser" },
        cid: "c-30",
        ts: s.items[0].ts,
        at: { kind: "anchor", status: "orphaned", scope: "section", quote: "## Stacked on", lines: [14, 14], section: "Stacked on", change: "11.0", loc: null },
        verdict: "no",
        status: "handled",
        sent_in: 7,
        turn: 8,
        amended: [],
        replies: [],
        outcome: { done: true, no_change: false, changes: ["11.0"] },
      });
    });
  }

  test("a section a bare x asked to remove is one row at its removal stop: your no, what went, with no done row and no empty quote", async ({
    page,
    birta,
  }) => {
    await open(page, birta, UNSEEN, bareSectionNo());
    // Rows open only on focus: the block above the gap opens none of the removal's
    await page.waitForTimeout(SETTLE_MS);
    expect((await changeRows(page)).filter((h) => h.sp === "14:1-14:13")).toEqual([]);
    expect((await hosts(page)).map((h) => h.text).filter((t) => /done · section|""/.test(t))).toEqual([]);
    await walkTo(page, "11.0");
    await expect
      .poll(async () => (await changeRows(page)).filter((h) => h.sp === "14:1-14:13").map((h) => [h.lead, h.substance, h.kbd]))
      .toEqual([["Claude · because you said no to ## Stacked on", "removed: ## Stacked on Nothing yet.", ["y", "u", "c"]]]);
    expect((await hosts(page)).map((h) => h.text).filter((t) => /done · section|""/.test(t))).toEqual([]);
  });

  test("a change a bare verdict asked for says which verdict on what, never an empty quote", async ({ page, birta }) => {
    await open(
      page,
      birta,
      FLOW,
      bent("flow", (s) => {
        // Note 10 as a bare x on the paragraph, which change 13.0 cites
        const i = s.items.find((i: State) => i.n === 10);
        delete i.body;
        i.verdict = "no";
        i.at = {
          kind: "anchor",
          status: "ok",
          scope: "block",
          quote: "When the upload hits a 503 we just fail and the user has to start again.",
          loc: { sp: "5:1-6:68" },
        };
      }),
    );
    await focusOn(page, "5:1-6:68");
    const lead = 'Claude · because you said no to "When the upload hits a 503…"';
    await expect.poll(async () => (await rowsLed(page, lead)).map((r) => r.kbd)).toEqual([["y", "u", "c"]]);
    expect((await rowsLed(page, lead))[0].text).not.toMatch(SIZE_LABEL);
  });

  test("a removal's row opens where n lands on its stop, and shows what went struck", async ({ page, birta }) => {
    await open(page, birta, UNSEEN, fixtureState("unseen"));
    await page.waitForTimeout(SETTLE_MS);
    expect(await hostWith(page, "removed:")).toBeNull();
    await walkTo(page, "11.0");
    await expect
      .poll(async () => (await changeRows(page)).map((r) => [r.lead, r.substance, r.sp, r.kbd]))
      .toEqual([["Claude · without a note", "removed: Nothing yet.", "14:1-14:13", ["y", "u", "c"]]]);
    expect((await changeRows(page))[0].text.endsWith(KEYS)).toBe(true);
    expect((await changeRows(page))[0].del).toEqual(["Nothing yet."]);
    const struck = await styleOf(page, "Nothing yet.", ["text-decoration-line", "color"]);
    expect(struck).toEqual({ "text-decoration-line": "line-through", color: MUTED });
  });

  test("a section's removal keeps its heading marker, shows its first block and counts the rest", async ({ page, birta }) => {
    const state = bent("unseen", (s) => {
      Object.assign(s.deletions.find((d: State) => d.change === "11.0"), {
        first_line: "## Stacked on",
        label: "section · 3 blocks",
        head: "## Stacked on\n\nNothing yet.",
        more: 1,
      });
    });
    await open(page, birta, UNSEEN, state);
    await walkTo(page, "11.0");
    await expect.poll(async () => (await changeRows(page)).map((r) => r.substance)).toEqual(["removed: ## Stacked on Nothing yet. and 1 more block"]);
    expect((await changeRows(page))[0].text.endsWith(KEYS)).toBe(true);
  });

  test("a one-block removal has no more line", async ({ page, birta }) => {
    const state = bent("unseen", (s) => delete s.deletions.find((d: State) => d.change === "11.0").label);
    await open(page, birta, UNSEEN, state);
    await walkTo(page, "11.0");
    await expect.poll(async () => (await changeRows(page)).map((r) => r.substance)).toEqual(["removed: Nothing yet."]);
  });

  test("j onto the block above a tick opens no tombstone", async ({ page, birta }) => {
    await open(page, birta, UNSEEN, fixtureState("unseen"));
    await focusOn(page, "14:1-14:13");
    await page.waitForTimeout(SETTLE_MS);
    expect(await hostWith(page, "removed:")).toBeNull();
  });

  test("a removal shows its head, not every line it removed", async ({ page, birta }) => {
    const state = bent("unseen", (s) => {
      s.deletions.find((d: State) => d.change === "11.0").removed = "Nothing yet.\nAnd a second line.\n";
    });
    await open(page, birta, UNSEEN, state);
    await walkTo(page, "11.0");
    await expect.poll(async () => (await changeRows(page)).map((r) => r.substance)).toEqual(["removed: Nothing yet."]);
    expect((await changeRows(page))[0].text).not.toContain("second line");
  });

  test("an outside removal's tombstone offers no revert", async ({ page, birta }) => {
    const state = bent("unseen", (s) => {
      Object.assign(s.changes.find((c: State) => c.id === "11.0"), {
        attr: "outside",
        turn: null,
        by: { type: "birta" },
        asked: { kind: "outside", notes: [] },
        reversible: { ok: false, reason: "outside" },
      });
    });
    await open(page, birta, UNSEEN, state);
    await walkTo(page, "11.0");
    await expect.poll(() => hostWith(page, "removed:")).toMatchObject({ kbd: ["y", "c"] });
    expect((await changeRows(page)).map((r) => r.substance)).toEqual(["removed: Nothing yet."]);
    expect((await hostWith(page, "removed:"))!.text).toMatch(/removed: Nothing yet\. · y keep · c note$/);
  });

  test("a removal at the very top opens its row above the doc's first block", async ({ page, birta }) => {
    const state = bent("unseen", (s) => {
      Object.assign(s.deletions.find((d: State) => d.change === "11.0"), {
        after_sp: null,
        first_line: "- gone item",
        label: "list item",
        removed: "- gone item\n",
        head: "- gone item",
        more: 0,
      });
    });
    await open(page, birta, UNSEEN, state);
    await walkTo(page, "11.0");
    await expect.poll(async () => (await changeRows(page)).map((r) => [r.lead, r.substance])).toEqual([["Claude · without a note", "removed: • gone item"]]);
    expect((await hostWith(page, "removed:"))!.text.endsWith(KEYS)).toBe(true);
    const above = await page.evaluate(() => {
      const content = document.getElementById("content")!;
      for (const h of document.querySelectorAll("birta-flow")) {
        if ((h.shadowRoot!.textContent ?? "").includes("removed:"))
          return !!(h.compareDocumentPosition(content) & Node.DOCUMENT_POSITION_FOLLOWING) && !content.contains(h);
      }
      return null;
    });
    expect(above).toBe(true);
  });
});

// --- Dots (#9) and rounds (#10, C5) -----------------------------------------------------------

/** flow.json with note 6 seen, so it folds to a dot on its item. */
function folded(): State {
  return bent("flow", (s) => (s.items.find((i: State) => i.n === 6).seen = true));
}

test.describe("dots", () => {
  test("a folded note's unseen change still opens its own row, keys and all, where n lands", async ({ page, birta }) => {
    await open(page, birta, FLOW, folded());
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    await walkTo(page, "15.0");
    await expect
      .poll(async () => (await changeRows(page)).filter((r) => r.sp === "11:1-11:44" && r.ins.join(" ").includes("capped at 8")))
      .toEqual([expect.objectContaining({ kbd: ["y", "u", "c"] })]);
  });

  test("a dot says whose settled note it is when hovered", async ({ page, birta }) => {
    // 22 is the agent's question, answered, and the answer handed back
    const state = bent("flow", (s) => {
      s.items.find((i: State) => i.n === 6).seen = true;
      s.items.find((i: State) => i.n === 22).seen = true;
      s.rounds.push({ ...s.rounds[0], submit: 26, items: [], claim: null, claimed_by: null, yield: null });
    });
    await open(page, birta, FLOW, state);
    await expect.poll(async () => (await dots(page)).length).toBe(2);
    const got = await page.evaluate(() =>
      Array.from(document.querySelector("#lane-frame > birta-over")!.shadowRoot!.querySelectorAll('[data-mark="dot"]'))
        .map((el) => ({ role: el.getAttribute("data-role"), title: el.getAttribute("title"), events: getComputedStyle(el).pointerEvents }))
        .sort((a, b) => (a.role! < b.role! ? -1 : 1)),
    );
    expect(got).toEqual([
      { role: "agent", title: "Claude's note · settled · ⏎ opens it", events: "auto" },
      { role: "you", title: "your note · settled · ⏎ opens it", events: "auto" },
    ]);
  });

  test("the agent's dot names the agent that took the turn", async ({ page, birta }) => {
    const state = bent("flow", (s) => {
      s.items.find((i: State) => i.n === 22).seen = true;
      s.items.find((i: State) => i.n === 22).by = { type: "agent", name: "codex", via: "cli" };
      s.rounds.push({ ...s.rounds[0], submit: 26, items: [], claim: null, claimed_by: null, yield: null });
    });
    await open(page, birta, FLOW, state);
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    const title = await page.evaluate(
      () => document.querySelector("#lane-frame > birta-over")!.shadowRoot!.querySelector('[data-mark="dot"]')!.getAttribute("title"),
    );
    expect(title).toBe("Codex's note · settled · ⏎ opens it");
  });

  test("⏎ on a dotted block opens its note, and ⏎ again folds it", async ({ page, birta }) => {
    await open(page, birta, FLOW, folded());
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    await focusOn(page, "11:1-11:44");
    // The change's own row quotes the note too, so only the note's host counts
    const note = async () => (await hosts(page)).find((h) => h.kind === "note" && h.text.includes("cap it")) ?? null;
    await page.keyboard.press("Enter");
    await expect.poll(note).not.toBeNull();
    await expect.poll(async () => (await dots(page)).length).toBe(0);
    await page.keyboard.press("Enter");
    await expect.poll(note).toBeNull();
    await expect.poll(async () => (await dots(page)).length).toBe(1);
  });

  test("the tip waits for focus to land on a dot, shows once, and not again after a reload", async ({ page, birta }) => {
    await open(page, birta, FLOW, folded());
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    // A dot drawn is no tip; focus landing on it is
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
    await focusOn(page, "11:1-11:44");
    await expect.poll(() => stripMessage(page)).toBe("• a settled note · ⏎ opens it");
    await page.reload();
    await booted(page);
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    await focusOn(page, "11:1-11:44");
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).not.toBe("• a settled note · ⏎ opens it");
  });

  test("a dot a push brings shows no tip until focus lands on it", async ({ page, birta }) => {
    const { mock } = await open(page, birta, FLOW, fixtureState("flow"));
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
    mock.pushState(folded());
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    await page.waitForTimeout(SETTLE_MS);
    expect(await stripMessage(page)).toBeNull();
    await focusOn(page, "11:1-11:44");
    await expect.poll(() => stripMessage(page)).toBe("• a settled note · ⏎ opens it");
  });

  test("storage that refuses writes still boots and draws the dot", async ({ page, birta }) => {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.stack ?? e.message));
    await page.addInitScript(() => {
      Storage.prototype.setItem = () => {
        throw new Error("quota");
      };
    });
    await open(page, birta, FLOW, folded());
    await expect.poll(async () => (await dots(page)).length).toBe(1);
    expect(errors).toEqual([]);
  });
});

/** replied.json after a second round: its first round's notes are an earlier round's. */
function twoRounds(edit: (s: State) => void = () => {}): State {
  return bent("replied", (s) => {
    s.rounds.push({ ...s.rounds[0], submit: 20, ts: s.server_now - 60_000, items: [], claim: 21, yield: 25 });
    edit(s);
  });
}

test.describe("rounds", () => {
  test("unchecked bars are 3 px and fresh; a kept one thins and mutes; an earlier round's are 1 px", async ({ page, birta }) => {
    const doc = fixtureDoc("fresh");
    const sp = (t: string) => spAt(doc, t);
    const state = laneState(doc, {
      bars: [{ sp: sp("- retries on 503") }, { sp: sp("# Upload retries"), seen: true }, { sp: sp("- backoff"), seen: true }],
    });
    state.changes[2].settled = true;
    await open(page, birta, doc, state);
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "bar").length).toBe(3);
    const bars = (await marks(page)).filter((m) => m.mark === "bar");
    const bar = (t: string) => bars.find((m) => m.sp === sp(t))!;
    expect(Math.round(bar("- retries on 503").width)).toBe(3);
    expect([bar("- retries on 503").check, bar("# Upload retries").check, bar("- backoff").check]).toEqual(["new", "checked", "checked"]);
    // Kept is checked: the bar thins, as a settled one is
    expect(bar("# Upload retries").width).toBeLessThan(3);
    expect(Math.round(bar("- backoff").width)).toBe(1);
    const s = await shot(page, await frameBox(page));
    const mid = (m: Box) => s.at((m.left + m.right) / 2, (m.top + m.bottom) / 2);
    expect(near(mid(bar("- retries on 503")), NEW_BAR, 12), `${mid(bar("- retries on 503"))}`).toBe(true);
    expect(near(mid(bar("# Upload retries")), NEW_BAR, 40), `${mid(bar("# Upload retries"))}`).toBe(false);
  });

  test("in the dark theme a fresh bar is the dark accent, and a 1 px bar still reads on the canvas", async ({ page, birta }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    const doc = fixtureDoc("fresh");
    const sp = (t: string) => spAt(doc, t);
    const state = laneState(doc, { bars: [{ sp: sp("- retries on 503") }, { sp: sp("- backoff"), seen: true }] });
    state.changes[1].settled = true;
    await open(page, birta, doc, state);
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "bar").length).toBe(2);
    const bars = (await marks(page)).filter((m) => m.mark === "bar");
    const fresh = bars.find((m) => m.sp === sp("- retries on 503"))!;
    const thin = bars.find((m) => m.sp === sp("- backoff"))!;
    const s = await shot(page, await frameBox(page));
    const at = (m: Box, dx = 0) => s.at((m.left + m.right) / 2 + dx, (m.top + m.bottom) / 2);
    expect(near(at(fresh), hex("#d29922"), 14), `${at(fresh)}`).toBe(true);
    expect(contrast(at(thin), at(thin, -6)), `${at(thin)} on ${at(thin, -6)}`).toBeGreaterThanOrEqual(3);
  });

  test("an earlier round's handled note folds to a dot even unseen", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("replied"), twoRounds());
    await expect.poll(async () => (await dots(page)).map((d) => d.sp)).toEqual(["5:1-6:68"]);
    expect(await hostWith(page, "say why")).toBeNull();
  });

  test("an earlier round's note with an unseen reply stays open", async ({ page, birta }) => {
    const state = twoRounds((s) => {
      s.items.find((i: State) => i.n === 4).replies.push({
        n: 14, id: "#14", by: { type: "agent", name: "claude", via: "cli" }, kind: "note", body: "Said it.", ts: s.server_now - 1_000, seen: false,
      });
    });
    await open(page, birta, fixtureDoc("replied"), state);
    await expect.poll(() => hostWith(page, "say why")).not.toBeNull();
    expect(await dots(page)).toEqual([]);
  });
});

// --- The lost row (#10) -----------------------------------------------------------------------

test.describe("lost row", () => {
  // 4 names its section; 5 is from a log with no section to name
  const lostState = () =>
    bent("flow", (s) => {
      s.items.find((i: State) => i.n === 4).at.section = "Why";
      delete s.items.find((i: State) => i.n === 5).at.section;
    });

  test("each lost note says where it was", async ({ page, birta }) => {
    await open(page, birta, FLOW, lostState());
    await expect.poll(() => hostWith(page, "2 notes lost their place")).not.toBeNull();
    await page.locator("birta-flow", { hasText: "2 notes lost their place" }).getByText("▸").click();
    await expect.poll(async () => (await hostWith(page, "lost their place"))?.text).toContain('was on: Why · "Nothing"');
    expect((await hostWith(page, "lost their place"))?.text).toContain('was on: "yet"');
    expect((await hostWith(page, "lost their place"))?.text).toContain("you · cut this");
  });

  test("the lost row is a walk stop that ⏎ opens and folds", async ({ page, birta }) => {
    await open(page, birta, FLOW, lostState());
    await expect.poll(() => hostWith(page, "2 notes lost their place")).not.toBeNull();
    expect(await page.evaluate(() => !!(window as unknown as W).BR.flow.focusStop({ kind: "lost", id: "lost", sp: null }))).toBe(true);
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await hostWith(page, "lost their place"))?.text).toContain("cut this");
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await hostWith(page, "lost their place"))?.text).not.toContain("cut this");
  });
});

// --- Replies and their places (#4 (b), N8, N9) ------------------------------------------------

test.describe("replies", () => {
  test("a doc note's done reply counts its changes and says n walks them", async ({ page, birta }) => {
    const state = bent("flow", (s) => {
      s.items.find((i: State) => i.n === 10).outcome = { done: true, no_change: false, changes: ["13.0", "19.0", "26.0"] };
    });
    await open(page, birta, FLOW, state);
    await expect.poll(() => hostWith(page, "3 changes")).toMatchObject({ text: "↳ Claude · done · 3 changes · n walks them", kbd: [], sp: null });
    // The changes keep their own rows, at their blocks
    await focusOn(page, "5:1-6:68");
    await expect.poll(async () => (await hostWith(page, "because you said"))?.sp).toBe("5:1-6:68");
  });

  test("a doc note's done reply with one change is singular", async ({ page, birta }) => {
    const state = bent("flow", (s) => {
      s.items.find((i: State) => i.n === 10).outcome = { done: true, no_change: false, changes: ["13.0"] };
    });
    await open(page, birta, FLOW, state);
    await expect.poll(async () => (await hostWith(page, "1 change"))?.text).toBe("↳ Claude · done · 1 change · n walks them");
  });

  test("the turn note reads as a reply under the whole-doc note", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await expect.poll(() => hostWith(page, "Capped the backoff.")).not.toBeNull();
    const slot = (await hosts(page)).filter((h) => h.sp === null).map((h) => h.text);
    const turn = slot.findIndex((t) => t.includes("Capped the backoff."));
    expect(slot[turn]).toBe("↳ Claude · Capped the backoff.");
    expect(slot[turn - 2]).toBe("you · is the test ladder still needed?");
    expect(slot[turn - 1]).toContain("Yes: CI runs it on every push.");
  });

  test("with no whole-doc note the turn note comes first in the doc slot", async ({ page, birta }) => {
    const state = bent("flow", (s) => (s.items = s.items.filter((i: State) => i.at.kind !== "doc")));
    await open(page, birta, FLOW, state);
    await expect.poll(() => hostWith(page, "Capped the backoff.")).not.toBeNull();
    const slot = (await hosts(page)).filter((h) => h.sp === null).map((h) => h.text);
    expect(slot[0]).toBe("↳ Claude · Capped the backoff.");
  });

  test("a bare verdict the agent acted on by a removal shows the removal at the change's tick", async ({ page, birta }) => {
    const state = bent("unseen", (s) => {
      const c = s.changes.find((c: State) => c.id === "11.0");
      c.asked = { kind: "location", notes: [30] };
      s.items.push({
        ...JSON.parse(JSON.stringify(s.items[0])),
        n: 30,
        cid: "c-30",
        body: undefined,
        verdict: "no",
        status: "handled",
        seen: false,
        replies: [],
        at: { kind: "anchor", status: "orphaned", scope: "block", quote: "Nothing yet.", loc: null, change: "11.0" },
        outcome: { done: true, no_change: false, changes: ["11.0"] },
      });
    });
    await open(page, birta, UNSEEN, state);
    // Its one row is the removal itself, which opens at the removal's stop
    await walkTo(page, "11.0");
    await expect
      .poll(async () => (await changeRows(page)).map((r) => [r.lead, r.substance, r.sp]))
      .toEqual([['Claude · because you said no to "Nothing yet."', "removed: Nothing yet.", "14:1-14:13"]]);
  });

  test("a handled note whose words the agent rewrote sits at its change", async ({ page, birta }) => {
    const state = bent("flow", (s) => {
      const i = s.items.find((i: State) => i.n === 6);
      i.at = { ...i.at, status: "orphaned", loc: null, change: "15.0" };
    });
    await open(page, birta, FLOW, state);
    await expect.poll(async () => (await hostWith(page, "cap it"))?.sp).toBe("11:1-11:44");
  });
});

// --- Focus (#17, C9) --------------------------------------------------------------------------

/** The focus outline and the scope wash, as drawn, with their styles. */
function focusMarks(page: Page) {
  return page.evaluate(() => {
    const out: Record<string, { box: { top: number; bottom: number; left: number; right: number }; style: Record<string, string> }> = {};
    for (const tag of ["birta-under", "birta-over"]) {
      const root = document.querySelector(`#lane-frame > ${tag}`)!.shadowRoot!;
      for (const el of root.querySelectorAll('[data-mark="focus"], [data-mark="scope"]')) {
        const r = el.getBoundingClientRect();
        const cs = getComputedStyle(el);
        out[el.getAttribute("data-mark")!] = {
          box: { top: r.top, bottom: r.bottom, left: r.left, right: r.right },
          style: {
            borderTopWidth: cs.borderTopWidth,
            borderTopStyle: cs.borderTopStyle,
            borderTopColor: cs.borderTopColor,
            borderRadius: cs.borderTopLeftRadius,
            background: cs.backgroundColor,
          },
        };
      }
    }
    return out;
  });
}

/** WCAG contrast of two colours. */
function contrast(a: Rgb, b: Rgb): number {
  const lum = (c: Rgb) => {
    const [r, g, bl] = c.map((v) => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

test.describe("focus", () => {
  for (const [what, text] of [
    ["a paragraph", "When the upload hits"],
    ["a list item", "with jitter"],
    ["a heading", "What changes"],
  ] as const) {
    test(`${what} takes a 1 px accent outline you can see`, async ({ page, birta }) => {
      await open(page, birta, STOPS_DOC);
      const sp = await stopSp(page, text);
      await focusOn(page, sp);
      const f = (await focusMarks(page)).focus;
      expect(f.style).toMatchObject({ borderTopWidth: "1px", borderTopStyle: "solid", borderTopColor: ACCENT, borderRadius: "6px" });
      const b = (await blockBox(page, sp))!;
      // The block's own lines, never its whole section
      expect(f.box.top).toBeLessThanOrEqual(b.top);
      expect(f.box.bottom).toBeGreaterThanOrEqual(b.bottom);
      expect(f.box.bottom - b.bottom).toBeLessThan(12);
      const s = await shot(page, { left: f.box.left - 2, top: f.box.top - 2, width: 6, height: f.box.bottom - f.box.top + 4 });
      const edge = s.at(f.box.left + 0.5, (f.box.top + f.box.bottom) / 2);
      expect(contrast(edge, WHITE), `${edge}`).toBeGreaterThanOrEqual(3);
    });
  }

  test("a heading's section is washed as its scope, under the outline on the heading", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const heading = await stopSp(page, "What changes");
    await focusOn(page, heading);
    const m = await focusMarks(page);
    const last = (await blockBox(page, await stopSp(page, "Detail words.")))!;
    expect(m.scope.box.bottom).toBeGreaterThanOrEqual(last.bottom);
    expect(m.scope.style.background).not.toBe("rgba(0, 0, 0, 0)");
    const own = (await blockBox(page, heading))!;
    expect(m.focus.box.bottom - own.bottom).toBeLessThan(12);
  });

  test("a paragraph's focus has no wash", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    await focusOn(page, await stopSp(page, "When the upload hits"));
    const m = await focusMarks(page);
    expect(m.scope).toBeUndefined();
    expect(m.focus.style.background).toBe("rgba(0, 0, 0, 0)");
  });

  test("the header chip takes no outline and no wash, even with no H1 over the doc", async ({ page, birta }) => {
    // With no H1 spanning the doc, the file chip once stood for the doc
    await open(page, birta, "Intro words.\n\n" + STOPS_DOC);
    const look = () =>
      page.locator(".file-header-name").evaluate((el) => {
        const cs = getComputedStyle(el);
        return { style: cs.outlineStyle, bg: cs.backgroundColor, shadow: cs.boxShadow };
      });
    const before = await look();
    expect(before.style).toBe("none");
    await page.keyboard.press("j");
    await page.keyboard.press("k");
    await expect.poll(() => focused(page)).toBe("doc");
    await expect(page.locator(".file-header-name[data-review-focus]")).toHaveCount(0);
    expect(await look()).toEqual(before);
  });

  test("an H1 at the top of the page shows its outline below the header", async ({ page, birta }) => {
    const doc = "# Title\n\nFirst paragraph.\n\n# Second\n\nMore.\n";
    await open(page, birta, doc);
    await focusOn(page, spAt(doc, "# Title"));
    expect(await page.evaluate(() => scrollY)).toBe(0);
    const f = (await focusMarks(page)).focus;
    const header = await page.locator(".file-header").evaluate((el) => el.getBoundingClientRect().bottom);
    expect(f.box.top).toBeGreaterThanOrEqual(header);
    const s = await shot(page, { left: f.box.left, top: f.box.top - 1, width: f.box.right - f.box.left, height: 4 });
    const top = s.at((f.box.left + f.box.right) / 2, f.box.top + 0.5);
    expect(contrast(top, WHITE), `${top}`).toBeGreaterThanOrEqual(3);
  });

  test("in the dark theme the outline still stands out from the canvas", async ({ page, birta }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    await open(page, birta, STOPS_DOC);
    await expect.poll(() => page.evaluate(() => document.documentElement.getAttribute("data-theme"))).toBe("dark");
    await focusOn(page, await stopSp(page, "When the upload hits"));
    const f = (await focusMarks(page)).focus;
    const s = await shot(page, { left: f.box.left - 4, top: f.box.top, width: 12, height: f.box.bottom - f.box.top });
    const y = (f.box.top + f.box.bottom) / 2;
    const edge = s.at(f.box.left + 0.5, y);
    const canvas = s.at(f.box.left - 3, y);
    expect(contrast(edge, canvas), `${edge} on ${canvas}`).toBeGreaterThanOrEqual(3);
  });
});

// --- The composer (N13, N4) -------------------------------------------------------------------

const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"]');

async function labelOf(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const h = document.querySelector('birta-flow[data-kind="composer"]');
    const label = h?.shadowRoot?.querySelector(".label");
    return label ? label.textContent : null;
  });
}

test.describe("composer", () => {
  test("its label names kind and scope: the whole doc, a section, Claude's change, a list item and a selection", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await page.keyboard.press("c");
    await expect.poll(() => labelOf(page)).toBe("note on whole doc");
    await page.keyboard.press("Escape");
    await focusOn(page, "3:1-3:6");
    await page.keyboard.press("c");
    await expect.poll(() => labelOf(page)).toBe("note on ## Why (section)");
    await page.keyboard.press("Escape");
    // A block whose change row is open: c replies on the change
    await focusOn(page, "5:1-6:68");
    await page.keyboard.press("c");
    await expect.poll(() => labelOf(page)).toBe("reply to Claude's change on ## Why › paragraph");
    await page.keyboard.press("Escape");
    // A list item is named by its markdown, whole when it is short
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("c");
    await expect.poll(() => labelOf(page)).toBe("note on - retries on 503 (list item)");
    await page.keyboard.press("Escape");
    await page.evaluate(() => {
      const el = document.querySelector('#content [data-sourcepos="5:1-6:68"]')!;
      const t = el.firstChild as Text;
      const at = t.data.indexOf("we just fail");
      const r = document.createRange();
      r.setStart(t, at);
      r.setEnd(t, at + "we just fail".length);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    });
    await page.keyboard.press("c");
    await expect.poll(() => labelOf(page)).toBe('note on "we just fail"');
  });

  test("a selection keeps a pale tint while its composer is open", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await page.evaluate(() => {
      const t = document.querySelector('#content [data-sourcepos="5:1-6:68"]')!.firstChild as Text;
      const at = t.data.indexOf("we just fail");
      const r = document.createRange();
      r.setStart(t, at);
      r.setEnd(t, at + "we just fail".length);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    });
    await page.keyboard.press("c");
    await expect(composer(page).locator("textarea")).toBeFocused();
    const tinted = () =>
      page.evaluate(() => {
        const hl = CSS.highlights.get("birta-pending");
        return hl ? Array.from(hl as unknown as Iterable<Range>).map((r) => r.toString()) : [];
      });
    await expect.poll(tinted).toEqual(["we just fail"]);
    await page.keyboard.press("Escape");
    await expect.poll(tinted).toEqual([]);
    // And on a save
    await page.evaluate(() => {
      const t = document.querySelector('#content [data-sourcepos="5:1-6:68"]')!.firstChild as Text;
      const r = document.createRange();
      r.setStart(t, 0);
      r.setEnd(t, 4);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    });
    await page.keyboard.press("c");
    await expect.poll(tinted).toEqual(["When"]);
    await page.keyboard.type("why");
    await page.keyboard.press("Enter");
    await expect.poll(tinted).toEqual([]);
  });

  test("its label names Claude's change by where it is, a paragraph by its first words, your own note, and a label the caller gives", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    const openWith = (kind: string, at: object, own: object | null, label?: string) =>
      page.evaluate(({ kind, at, own, label }) => (window as unknown as W).BR.flow.open(kind, at, own, label), { kind, at, own, label });
    await openWith("note", { k: "change", id: "13.0" }, null);
    await expect.poll(() => labelOf(page)).toBe("reply to Claude's change on ## Why › paragraph");
    // How many words a long paragraph keeps is open; its first words lead, unquoted, then … and its kind
    await openWith("note", { k: "block", sp: "5:1-6:68", scope: "block" }, null);
    await expect.poll(() => labelOf(page)).toMatch(/^note on When the upload( [^\s"]+)*… \(paragraph\)$/);
    // A change on a heading is named by that heading, its own section
    await openWith("note", { k: "change", id: "19.0" }, null);
    await expect.poll(() => labelOf(page)).toBe("reply to Claude's change on ## Stacked on nothing");
    await openWith("note", { k: "block", sp: "10:1-10:16", scope: "block" }, { n: 7, body: "drop this bullet" });
    await expect.poll(() => labelOf(page)).toBe("editing your note");
    await openWith("note", { k: "doc" }, null, "new round · whole doc");
    await expect.poll(() => labelOf(page)).toBe("new round · whole doc");
  });

  test("both composers say how to save, break a line, hand back and close, with their placeholders", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    const mac = await page.evaluate(() => /Mac/.test(navigator.platform));
    const hint = `⏎ save · ⇧⏎ new line · ${mac ? "⌘⏎" : "Ctrl⏎"} save & hand back · esc save & close`;
    await page.keyboard.press("c");
    await expect(composer(page).getByText(hint, { exact: true })).toBeVisible();
    await expect(composer(page).locator("textarea")).toHaveAttribute("placeholder", 'note… · start with "words" to point at them');
    await page.keyboard.press("Escape");
    await focusOn(page, "10:1-10:16");
    await page.keyboard.press("q");
    await expect(composer(page).getByText(hint, { exact: true })).toBeVisible();
    await expect(composer(page).locator("textarea")).toHaveAttribute("placeholder", "question for Claude…");
    await expect.poll(() => labelOf(page)).toBe("question on - retries on 503 (list item)");
  });
});

// --- Pill copy (N6) and the phrase (N7, N14) --------------------------------------------------

test.describe("copy", () => {
  test("the first note on a doc pills note saved", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("c");
    await page.keyboard.type("first");
    await page.keyboard.press("Enter");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBeGreaterThan(0);
    await expect.poll(() => stripMessage(page)).toBe("note saved · z to undo");
  });

  test("the phrase holds one event and one count, fits the header at 1280 px, and leaves nothing to the tooltip", async ({
    page,
    birta,
  }) => {
    const state = bent("flow", (s) => Object.assign(s.counts, { unseen: 12, drafts: 10, decided: 10, items: 17, to_check: 12, attention: 12 }));
    await open(page, birta, FLOW, state);
    const p = page.locator(".file-header birta-phrase");
    await expect.poll(() => p.evaluate((h) => h.shadowRoot!.querySelector(".phrase")!.textContent)).toMatch(/^Claude replied \d\d:\d\d · 12 to check$/);
    const fits = await p.evaluate((h) => {
      const span = h.shadowRoot!.querySelector(".phrase") as HTMLElement;
      return { text: span.textContent, fits: span.scrollWidth <= span.clientWidth, title: h.getAttribute("title") };
    });
    expect(fits.fits).toBe(true);
    // The reply's tallies, your notes and decided blocks are gone; the rows and the button carry them
    expect(fits.title).toBe(fits.text);
    expect(fits.title).not.toMatch(/unseen|decided|done|answered|for you|without a note|not handled|notes?\b/);
  });
});

// --- Cards and verdict colours (C9) -----------------------------------------------------------

test.describe("cards", () => {
  test("every annotation is a card with a 13 px muted author lead", async ({ page, birta }) => {
    await open(page, birta, FLOW, fixtureState("flow"));
    await expect.poll(() => hostWith(page, "drop this bullet")).not.toBeNull();
    // Claude's done row under `cap it` opens with its block
    await focusOn(page, "11:1-11:44");
    await expect.poll(() => rowsLed(page, "↳ Claude · done")).toHaveLength(1);
    for (const [has, lead] of [
      ["drop this bullet", "you · "],
      ["is 503 the only code?", "Claude · "],
      ["Yes: CI runs it on every push.", "↳ Claude · "],
      ["yes, keep it", "↳ you · "],
      ["↳ Claude · done", "↳ Claude · done"],
    ]) {
      const got = await page.evaluate(
        ({ has }) => {
          for (const h of document.querySelectorAll("birta-flow")) {
            const root = h.shadowRoot!;
            if (!(root.textContent ?? "").includes(has)) continue;
            const box = root.querySelector(".flow")!;
            const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
              acceptNode: (n) =>
                n.parentElement?.closest("style") || !(n.textContent ?? "").trim() ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT,
            });
            const first = walker.nextNode()!;
            const lead = getComputedStyle(first.parentElement!);
            const card = getComputedStyle(box);
            return {
              text: first.textContent,
              size: lead.fontSize,
              color: lead.color,
              fill: card.backgroundColor,
              border: card.borderTopStyle,
            };
          }
          return null;
        },
        { has },
      );
      expect({ ...got, text: got?.text?.startsWith(lead) ? lead : got?.text }, has).toEqual({ text: lead, size: "13px", color: MUTED, fill: CARD, border: "none" });
    }
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`an open row's and a tombstone's lead is a 13 px lead that reads on its card, ${scheme}`, async ({ page, birta }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await open(page, birta, UNSEEN, fixtureState("unseen"));
      await walkTo(page, "11.0");
      for (const has of ["removed:"]) {
        await expect.poll(() => hostWith(page, has)).not.toBeNull();
      }
      await focusOn(page, "11:1-11:44");
      await expect.poll(() => hostWith(page, "without a note")).not.toBeNull();
      const got = await page.evaluate(() => {
        const out: { text: string; size: string; color: string; fill: string }[] = [];
        for (const h of document.querySelectorAll("birta-flow")) {
          const root = h.shadowRoot!;
          if (!/without a note|removed:/.test(root.textContent ?? "")) continue;
          const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
            acceptNode: (n) =>
              n.parentElement?.closest("style") || !(n.textContent ?? "").trim() ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT,
          });
          const first = walker.nextNode()!;
          const cs = getComputedStyle(first.parentElement!);
          out.push({ text: first.textContent!, size: cs.fontSize, color: cs.color, fill: getComputedStyle(root.querySelector(".flow")!).backgroundColor });
        }
        return out;
      });
      // The lead's words follow its author, on the same 13 px lead
      expect(got.map((g) => [g.text.startsWith("Claude · ") ? "Claude · " : g.text, g.size])).toEqual([["Claude · ", "13px"]]);
      const rgb = (c: string) => c.match(/\d+/g)!.slice(0, 3).map(Number) as Rgb;
      for (const g of got) expect(contrast(rgb(g.color), rgb(g.fill)), `${g.color} on ${g.fill}`).toBeGreaterThanOrEqual(4.5);
    });
  }

  test("a selection copied across notes, rows and a tombstone holds only the doc's words", async ({ page, birta }) => {
    const state = bent("flow", (s) => {
      s.deletions.push({
        change: "19.0",
        kind: "block",
        at_line: 15,
        at_byte: 290,
        removed: "Gone.\n",
        after_sp: "14:1-14:21",
        first_line: "Gone.",
        label: "paragraph",
        head: "Gone.",
        more: 0,
      });
      Object.assign(s.changes.find((c: State) => c.id === "19.0"), { range: { status: "ok", lines: [15, 14], bytes: [290, 290] } });
      // News names it as the removal it now is, in its gap
      Object.assign(s.news.find((n: State) => n.id === "19.0"), { kind: "removal", sp: null, after_sp: "14:1-14:21" });
    });
    await open(page, birta, FLOW, state);
    await walkTo(page, "19.0");
    await expect.poll(() => hostWith(page, "removed:")).not.toBeNull();
    const copied = await page.evaluate(() => {
      const content = document.getElementById("content")!;
      const r = document.createRange();
      r.setStartBefore(content.firstElementChild!);
      r.setEndAfter(content.lastElementChild!);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
      const data = new DataTransfer();
      document.dispatchEvent(new ClipboardEvent("copy", { clipboardData: data, bubbles: true, cancelable: true }));
      return data.getData("text/plain");
    });
    expect(copied).toContain("Upload retries");
    expect(copied).toContain("metrics dashboard");
    for (const word of ["Claude ·", "you ·", "y keep", "removed:", "reverted", "was on:", "cap it", "↳"]) expect(copied, word).not.toContain(word);
  });

  test("verdict glyphs take their verdict's colour; a draft is a ring and a sent one is filled", async ({ page, birta }) => {
    const verdicts = ["yes", "no", "ask", "later", "your_call"];
    const doc = `# Icons\n\n${verdicts.map((v) => `Paragraph for ${v}.`).join("\n\n")}\n\nDraft no.\n`;
    const state = laneState(doc, {
      glyphs: [
        ...verdicts.map((v) => ({ sp: spAt(doc, `for ${v}.`), verdict: v, status: "sent" })),
        { sp: spAt(doc, "Draft no."), verdict: "no", status: "draft" },
      ],
    });
    await open(page, birta, doc, state);
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(6);
    const looks = await page.evaluate(() => {
      const root = document.querySelector("#lane-frame > birta-over")!.shadowRoot!;
      return Array.from(root.querySelectorAll('[data-mark="glyph"]')).map((g) => {
        const cs = getComputedStyle(g);
        return { verdict: g.getAttribute("data-verdict"), role: g.getAttribute("data-role"), fill: cs.backgroundColor, ring: cs.borderTopColor, ringWidth: cs.borderTopWidth };
      });
    });
    const sent = (v: string) => looks.find((l) => l.verdict === v && l.role === "agent")!;
    expect(sent("no").fill).toBe(DANGER);
    expect(sent("yes").fill).toBe(SUCCESS);
    expect(sent("later").fill).toBe(MUTED);
    expect(sent("your_call").fill).toBe(MUTED);
    expect(sent("ask").fill).toBe(ACCENT);
    const labels = await page.evaluate(() =>
      Array.from(document.querySelector("#lane-frame > birta-over")!.shadowRoot!.querySelectorAll('[data-mark="glyph"][data-role="agent"]'))
        .map((g) => [g.getAttribute("data-verdict"), g.getAttribute("aria-label")]),
    );
    expect(Object.fromEntries(labels)).toEqual({ yes: "yes ✓", no: "no ×", ask: "ask ?", later: "later ◷", your_call: "your call ·" });
    const draft = looks.find((l) => l.role === "you")!;
    expect(draft.ring).toBe(DANGER);
    expect(draft.ringWidth).not.toBe("0px");
    expect(draft.fill).not.toBe(DANGER);
  });

  test("a no verdict dims its block's text and never strikes it", async ({ page, birta }) => {
    const doc = "# Dim\n\nThis paragraph gets a no.\n\nThis paragraph gets nothing.\n";
    const no = spAt(doc, "gets a no.");
    const plain = spAt(doc, "gets nothing.");
    await open(page, birta, doc, laneState(doc, { glyphs: [{ sp: no, verdict: "no", status: "sent" }] }));
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(1);
    await page.waitForTimeout(SETTLE_MS);
    const a = (await blockBox(page, no))!;
    const b = (await blockBox(page, plain))!;
    const s = await shot(page, await frameBox(page));
    const inkA = ink(pixelsOf(s, a), WHITE)!;
    const inkB = ink(pixelsOf(s, b), WHITE)!;
    // 70 % of the text's darkness, give or take antialiasing
    const ratio = distance(inkA, WHITE) / distance(inkB, WHITE);
    expect(ratio, `${inkA} vs ${inkB}`).toBeGreaterThan(0.55);
    expect(ratio, `${inkA} vs ${inkB}`).toBeLessThan(0.85);
    const deco = await page.locator(`#content [data-sourcepos="${no}"]`).evaluate((el) => getComputedStyle(el).textDecorationLine);
    expect(deco).toBe("none");
  });

  test("a no on a heading dims its whole section", async ({ page, birta }) => {
    const doc = "# Dim\n\n## Cut\n\nUnder the cut heading, [to keep](#keep).\n\n## Keep\n\nUnder the kept heading.\n";
    const state = laneState(doc, { glyphs: [{ sp: spAt(doc, "## Cut"), verdict: "no", status: "sent" }] });
    state.items[0].at.scope = "section";
    delete state.items[0].body;
    await open(page, birta, doc, state);
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(1);
    await page.waitForTimeout(SETTLE_MS);
    const a = (await blockBox(page, spAt(doc, "Under the cut")))!;
    const b = (await blockBox(page, spAt(doc, "Under the kept")))!;
    const s = await shot(page, await frameBox(page));
    const ratio = distance(ink(pixelsOf(s, a), WHITE)!, WHITE) / distance(ink(pixelsOf(s, b), WHITE)!, WHITE);
    expect(ratio).toBeGreaterThan(0.55);
    expect(ratio).toBeLessThan(0.85);
    // The next section stays bright
    const heading = (await blockBox(page, spAt(doc, "## Keep")))!;
    const title = (await blockBox(page, spAt(doc, "# Dim")))!;
    const bright = distance(ink(pixelsOf(s, heading), WHITE)!, WHITE) / distance(ink(pixelsOf(s, title), WHITE)!, WHITE);
    expect(bright).toBeGreaterThan(0.9);
    // Dimmed words still take a click
    await page.locator("#content a", { hasText: "to keep" }).click();
    await expect.poll(() => page.evaluate(() => location.hash)).toBe("#keep");
  });

  test("every glyph sits 4 px clear of the rule, stacked over a note or alone, at both widths", async ({ page, birta }) => {
    const doc = "# Rule\n\nOne line.\n\nNext.\n";
    const sp = spAt(doc, "One line.");
    const state = laneState(doc, {
      glyphs: [
        { sp, verdict: "no", status: "sent" },
        { sp, verdict: "later", status: "sent" },
        { sp, verdict: "ask", status: "sent" },
        { sp: spAt(doc, "Next."), verdict: "yes", status: "draft" },
      ],
    });
    state.items.forEach((i: State, k: number) => (k ? delete i.body : (i.body = "a note under it")));
    // The turn note's rule is in the doc slot, nowhere near
    state.phrase.replied.note = null;
    await open(page, birta, doc, state);
    const note = page.locator("birta-flow", { hasText: "a note under it" });
    for (const viewport of [WIDE, NARROW]) {
      await page.setViewportSize(viewport);
      await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(4);
      await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "rule").length).toBeGreaterThan(0);
      await page.waitForTimeout(SETTLE_MS);
      const ms = await marks(page);
      const rules = ms.filter((m) => m.mark === "rule");
      const glyphs = ms.filter((m) => m.mark === "glyph");
      const host = (await note.boundingBox())!;
      expect(glyphs.some((g) => g.bottom > host.y && g.top < host.y + host.height), `${viewport.width} setup: a glyph reaches the note`).toBe(true);
      // The rule still runs beside the note wherever no glyph is near
      expect(rules.some((r) => r.bottom >= host.y + host.height - 1), `${viewport.width} rule reaches the note's end`).toBe(true);
      for (const g of glyphs) {
        const near = { left: g.left - 4, right: g.right + 4, top: g.top - 4, bottom: g.bottom + 4 };
        for (const r of rules) {
          const hit = r.left < near.right && r.right > near.left && r.top < near.bottom && r.bottom > near.top;
          expect(hit, `${viewport.width} ${g.verdict}: rule ${r.top}-${r.bottom} within 4 px of ${g.top}-${g.bottom}`).toBe(false);
        }
      }
    }
  });
});

