import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  Box,
  Mark,
  Rgb,
  booted,
  blockBox,
  contentBox,
  distance,
  fixtureDoc,
  fixtureState,
  focused,
  frameBox,
  hex,
  ink,
  laneState,
  markKeys,
  marks,
  misaligned,
  mockReview,
  near,
  over,
  pixelsOf,
  revOf,
  shot,
  spAt,
  withRev,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const AGENT = hex("#59636e");
const AGENT_DARK = hex("#9198a1");
const YOU = hex("#0969da");
const YOU_DARK = hex("#4493f8");
const NEW = hex("#fff8c5");
const WHITE: Rgb = [255, 255, 255];
/** A bar for a change of Claude's news, until it is checked. */
const FRESH = hex("#bf8700");
const FRESH_DARK = hex("#d29922");
const DANGER = hex("#d1242f");
const DANGER_DARK = hex("#f85149");
/** The wash under a block a no veils: its words, and the wash with them, at 70 %. */
const NEW_VEILED = NEW.map((v) => Math.round(v * 0.7 + 255 * 0.3)) as Rgb;

const UNSEEN_KEYS = [
  "bar 5:1-6:68 solid",
  "bar 11:1-11:44 dotted",
  "glyph 5:1-6:68 no",
  "glyph 10:1-10:16 no",
  "wash 5:1-6:68",
  "wash 11:1-11:44",
  "tick after 14:1-14:13",
].sort();

const TALL_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="400"><rect width="40" height="400"/></svg>';

type Start = (files: Record<string, string>, env?: Record<string, string>, args?: string[]) => Promise<{ url: string; write(rel: string, text: string): void }>;

/** Serves `doc` as pr.md with the gate on and the socket mocked, then waits for review to boot. */
async function open(
  page: Page,
  birta: Start,
  doc: string,
  opts: { state?: object; states?: object[]; args?: string[]; viewport?: { width: number; height: number } } = {},
) {
  const served = await birta({ "pr.md": doc }, GATE_ON, opts.args);
  await page.setViewportSize(opts.viewport ?? WIDE);
  const mock = await mockReview(page, { state: opts.state, states: opts.states });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

function only(ms: Mark[], mark: string, sp: string): Mark {
  const found = ms.filter((m) => m.mark === mark && m.sp === sp);
  expect(found, `${mark} on ${sp}`).toHaveLength(1);
  return found[0];
}

/** The lane's centre column, sampled from `top` to `bottom`. */
async function column(page: Page, x: number, top: number, bottom: number): Promise<Rgb[]> {
  const s = await shot(page, { left: x - 1, top, width: 3, height: bottom - top });
  const out: Rgb[] = [];
  for (let y = Math.ceil(top); y < Math.floor(bottom); y++) out.push(s.at(x - 0.5, y));
  return out;
}

async function laneX(page: Page): Promise<number> {
  return (await frameBox(page)).left + ((await page.evaluate(() => window.innerWidth)) < 768 ? 8 : 20);
}

/** The ink of each pixel of `box`, or null when nothing but background is there. */
async function inkIn(page: Page, box: Box, bg: Rgb): Promise<Rgb | null> {
  const s = await shot(page, box);
  return ink(pixelsOf(s, box), bg);
}

test.describe("harness", () => {
  test("the pixel probe reads what is painted", async ({ page, birta }) => {
    const served = await birta({ "pr.md": fixtureDoc("fresh") });
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await page.evaluate(() => {
      const d = document.createElement("div");
      d.style.cssText = "position:fixed;left:10px;top:10px;width:20px;height:20px;background:rgb(12,34,56);z-index:999";
      document.body.appendChild(d);
    });
    const s = await shot(page, { left: 0, top: 0, width: 40, height: 40 });
    expect(s.at(20, 20)).toEqual([12, 34, 56]);
    expect(near(s.at(35, 35), [12, 34, 56])).toBe(false);
  });
});

test.describe("containment", () => {
  test("review mode clips the doc to its own box and lays the lane over the frame", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    const got = await page.evaluate(() => {
      const el = document.getElementById("content")!;
      const cs = getComputedStyle(el);
      const hosts = ["birta-under", "birta-over"].map((t) => {
        const h = document.querySelector(t)!;
        const hs = getComputedStyle(h);
        return { parent: h.parentElement!.id, position: hs.position, pointer: hs.pointerEvents };
      });
      // The lane must not eat clicks meant for the doc
      const hits = Array.from(el.querySelectorAll("p, li, h2")).map((b) => {
        const r = b.getBoundingClientRect();
        const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return !!hit && el.contains(hit);
      });
      return {
        contain: cs.contain,
        padding: [cs.paddingTop, cs.paddingRight, cs.paddingBottom, cs.paddingLeft],
        firstChildMargin: getComputedStyle(el.firstElementChild!).marginTop,
        hosts,
        hits,
      };
    });
    expect(got.contain).toBe("paint");
    expect(got.padding).toEqual(["0px", "0px", "0px", "0px"]);
    expect(got.firstChildMargin).toBe("0px");
    for (const h of got.hosts) expect(h).toEqual({ parent: "lane-frame", position: "absolute", pointer: "none" });
    expect(got.hits.length).toBeGreaterThan(4);
    expect(got.hits.every(Boolean)).toBe(true);
  });

  test("printing drops the clip", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("fresh"), { state: fixtureState("fresh") });
    await page.emulateMedia({ media: "print" });
    await expect.poll(() => page.evaluate(() => getComputedStyle(document.getElementById("content")!).contain)).toBe("none");
  });

  for (const [variant, want] of [
    ["light", { you: "#0969da", agent: "#59636e", fresh: "#fff8c5", wash: "#818b981f" }],
    ["dark", { you: "#4493f8", agent: "#9198a1", fresh: "#bb800926", wash: "#656c7633" }],
  ] as const) {
    test(`the colour roles are defined in ${variant}`, async ({ page, birta }) => {
      await open(page, birta, fixtureDoc("fresh"), { args: ["pr.md", `--${variant}`] });
      const roles = await page.evaluate(() => {
        const cs = getComputedStyle(document.documentElement);
        const v = (n: string) => cs.getPropertyValue(n).trim().toLowerCase();
        return {
          you: v("--review-you"),
          agent: v("--review-agent"),
          fresh: v("--review-new"),
          wash: v("--review-wash"),
          quiet: v("--review-quiet"),
        };
      });
      expect(roles).toMatchObject(want);
      expect(roles.quiet).toContain("color-mix");
      expect(roles.quiet).toContain("40%");
    });
  }

  test("a heading's link icon is drawn by the overlay on hover", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("fresh"), { state: fixtureState("fresh") });
    const h = (await blockBox(page, "3:1-3:6"))!;
    const frame = await frameBox(page);
    const content = frame.left + 40;
    const zone: Box = { left: content - 20, top: h.line.top, right: content - 4, bottom: h.line.bottom, width: 16, height: h.line.bottom - h.line.top };
    const bg = WHITE;
    await page.hover("#content h2 >> nth=0");
    const icon = only(await marks(page), "anchor", "3:1-3:6");
    expect(Math.abs(icon.left - zone.left)).toBeLessThanOrEqual(1);
    expect(Math.abs(icon.width - 16)).toBeLessThanOrEqual(1);
    const cy = (icon.top + icon.bottom) / 2;
    expect(cy).toBeGreaterThanOrEqual(h.line.top);
    expect(cy).toBeLessThanOrEqual(h.line.bottom);
    expect(await inkIn(page, zone, bg)).not.toBeNull();
    await page.mouse.move(frame.right - 5, frame.bottom - 5);
    await expect.poll(() => inkIn(page, zone, bg)).toBeNull();
  });

  // Regression guard: the doc's own anchor sits in the clipped padding, so the overlay must stand in for it
  test("a heading's link icon takes a click to its fragment", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("fresh"), { state: fixtureState("fresh") });
    const h = (await blockBox(page, "3:1-3:6"))!;
    const content = (await frameBox(page)).left + 40;
    await page.hover("#content h2 >> nth=0");
    await page.mouse.move(content - 12, (h.line.top + h.line.bottom) / 2, { steps: 4 });
    await page.mouse.down();
    await page.mouse.up();
    await expect.poll(() => page.evaluate(() => location.hash)).toBe("#why");
  });

  test("a fragment link still scrolls to its heading", async ({ page, birta }) => {
    const filler = Array.from({ length: 80 }, (_, i) => `Filler paragraph ${i}.`).join("\n\n");
    const doc = `${fixtureDoc("fresh")}\n${filler}\n\n## Far away\n\n${filler}\n`;
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.setViewportSize(WIDE);
    await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md#far-away`);
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(500);
    const top = await page.evaluate(() => document.getElementById("far-away")!.closest("h2")!.getBoundingClientRect().top);
    expect(top).toBeGreaterThan(-5);
    expect(top).toBeLessThan(200);
    await booted(page);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.evaluate(() => (location.hash = "#why"));
    await page.evaluate(() => (location.hash = "#far-away"));
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(500);
  });

  test("a footnote's target ring is not clipped", async ({ page, birta }) => {
    const doc = "# Notes\n\nA claim that needs a source.[^1]\n\n[^1]: The source, with enough words to fill a line.\n";
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.setViewportSize(WIDE);
    await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md#fn-1`);
    await booted(page);
    const li = await page.evaluate(() => {
      const r = document.getElementById("fn-1")!.getBoundingClientRect();
      return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
    });
    const region: Box = { left: li.left - 30, top: li.top - 14, right: li.right + 30, bottom: li.bottom + 14, width: li.width + 60, height: li.height + 28 };
    const s = await shot(page, region);
    const RING = hex("#0969da");
    let box: Box | null = null;
    for (let y = Math.ceil(region.top); y < Math.floor(region.bottom); y++)
      for (let x = Math.ceil(region.left); x < Math.floor(region.right); x++) {
        if (!near(s.at(x, y), RING, 12)) continue;
        box = box
          ? { ...box, left: Math.min(box.left, x), top: Math.min(box.top, y), right: Math.max(box.right, x), bottom: Math.max(box.bottom, y) }
          : { left: x, top: y, right: x, bottom: y, width: 0, height: 0 };
      }
    expect(box, "the ring is drawn").not.toBeNull();
    const b = box!;
    const midX = Math.round((b.left + b.right) / 2);
    const midY = Math.round((b.top + b.bottom) / 2);
    // A clipped side leaves the box edge without its border
    const sides = {
      left: [0, 1].some((d) => near(s.at(b.left + d, midY), RING, 12)),
      right: [0, 1].some((d) => near(s.at(b.right - d, midY), RING, 12)),
      top: [0, 1].some((d) => near(s.at(midX, b.top + d), RING, 12)),
      bottom: [0, 1].some((d) => near(s.at(midX, b.bottom - d), RING, 12)),
    };
    expect(sides).toEqual({ left: true, right: true, top: true, bottom: true });
  });

  test("review boots with no errors and no CSP violations", async ({ page, birta }) => {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(`console: ${m.text()}`);
    });
    await page.addInitScript(() => {
      document.addEventListener("securitypolicyviolation", (e) =>
        console.error(`csp: ${e.violatedDirective} ${e.blockedURI}`),
      );
    });
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    await page.waitForTimeout(300);
    expect(errors).toEqual([]);
  });
});

test.describe("gate off", () => {
  test("the page loads no review code and draws nothing", async ({ page, birta }) => {
    const served = await birta({ "pr.md": fixtureDoc("unseen") });
    const review: string[] = [];
    page.on("request", (r) => {
      if (r.url().includes("/_birta/review")) review.push(r.url());
    });
    await page.setViewportSize(WIDE);
    await mockReview(page, { state: fixtureState("unseen") });
    await page.goto(`${served.url}/view/pr.md`);
    await page.waitForTimeout(500);
    const got = await page.evaluate(() => ({
      review: document.documentElement.hasAttribute("data-review"),
      hosts: document.querySelectorAll("birta-under, birta-over, birta-flow").length,
      br: typeof (window as { BR?: unknown }).BR,
      contain: getComputedStyle(document.getElementById("content")!).contain,
    }));
    expect(review).toEqual([]);
    expect(got).toEqual({ review: false, hosts: 0, br: "undefined", contain: "none" });
  });
});

test.describe("lane overlay", () => {
  test("the unseen fixture's marks sit in the lane on their blocks", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    expect(await misaligned(page)).toEqual([]);
    for (const m of await marks(page)) {
      // Both changes are this round's
      if (m.mark === "bar") expect(Math.abs(m.width - 3), `${m.sp} bar width`).toBeLessThanOrEqual(0.5);
      if (m.mark === "glyph") {
        expect(Math.abs(m.width - 16), `${m.sp} glyph width`).toBeLessThanOrEqual(1);
        expect(Math.abs(m.height - 16), `${m.sp} glyph height`).toBeLessThanOrEqual(1);
      }
      if (m.mark === "tick") {
        expect(Math.abs(m.width - 12), "tick width").toBeLessThanOrEqual(0.5);
        expect(Math.abs(m.height - 2), "tick height").toBeLessThanOrEqual(0.5);
      }
    }
  });

  test("an asked change is a solid bar and an unasked one is dotted, fresh until checked", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const ms = await marks(page);
    const x = await laneX(page);
    const solid = only(ms, "bar", "5:1-6:68");
    const glyph = only(ms, "glyph", "5:1-6:68");
    const dotted = only(ms, "bar", "11:1-11:44");
    expect(solid.role).toBe("agent");
    expect(dotted.role).toBe("agent");
    // Neither is checked yet: both are new
    expect([solid.check, dotted.check]).toEqual(["new", "new"]);
    // Below the glyph, where only the bar paints
    const solidPx = await column(page, x, glyph.bottom + 2, solid.bottom - 1);
    expect(solidPx.length).toBeGreaterThan(8);
    expect(solidPx.filter((p) => !near(p, FRESH, 12)), "every pixel is the fresh colour").toEqual([]);
    const dottedPx = await column(page, x, dotted.top + 1, dotted.bottom - 1);
    expect(dottedPx.some((p) => near(p, FRESH, 12)), "dots").toBe(true);
    expect(dottedPx.some((p) => distance(p, FRESH) > 60), "gaps").toBe(true);
  });

  test("the new wash shows through the doc and covers a list marker", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const p = (await blockBox(page, "5:1-6:68"))!;
    const li = (await blockBox(page, "11:1-11:44"))!;
    const s = await shot(page, await frameBox(page));
    // Inside the paragraph, past the end of its short last line, under its no's veil
    expect(near(s.at(p.right - 4, p.bottom - 4), NEW_VEILED)).toBe(true);
    // In the 8px the wash reaches past the block
    expect(near(s.at(p.right + 4, p.top + 4), NEW)).toBe(true);
    // Left of the bullet, which only a wash reaching to the list edge covers
    expect(near(s.at(li.list!.left + 3, (li.top + li.bottom) / 2), NEW)).toBe(true);
    expect(near(s.at(li.right - 4, (li.top + li.bottom) / 2), NEW)).toBe(true);
    // A block with no unseen change stays plain
    const other = (await blockBox(page, "12:1-13:0"))!;
    expect(near(s.at(other.right - 4, (other.top + other.bottom) / 2), WHITE)).toBe(true);
  });

  test("a draft verdict is a ring and a sent one a disc, both in the verdict's colour", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("fresh"), { state: fixtureState("drafts") });
    await expect.poll(() => markKeys(page)).toEqual(["glyph 10:1-10:16 no", "glyph 5:1-6:54 no"]);
    expect(await misaligned(page)).toEqual([]);
    const glyph = () => marks(page).then((ms) => only(ms, "glyph", "10:1-10:16"));
    expect((await glyph()).role).toBe("you");
    const draft = await glyph();
    expect(near((await inkIn(page, draft, WHITE))!, DANGER, 10)).toBe(true);
    // A ring: the disc's middle-left is the page, not the colour
    const ds = await shot(page, draft);
    expect(near(ds.at(draft.left + 3.5, draft.top + draft.height / 2), WHITE, 40)).toBe(true);
    mock.pushState(fixtureState("sent"));
    await expect.poll(async () => (await glyph()).role).toBe("agent");
    const sent = await glyph();
    expect(near((await inkIn(page, sent, WHITE))!, DANGER, 10)).toBe(true);
    const ss = await shot(page, sent);
    expect(near(ss.at(sent.left + 3.5, sent.top + sent.height / 2), DANGER, 30)).toBe(true);
  });

  test("two changes on one block draw one bar and one wash", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("lost"), { state: fixtureState("lost") });
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(1);
    const ms = await marks(page);
    const bars = ms.filter((m) => m.mark === "bar" && m.sp === "5:1-6:68");
    expect(bars).toHaveLength(1);
    // The newest unseen change wins: the outside write, after the agent's asked one
    expect(bars[0].style).toBe("dotted");
    expect(ms.filter((m) => m.mark === "wash" && m.sp === "5:1-6:68")).toHaveLength(1);
    expect(await misaligned(page)).toEqual([]);
  });

  test("a big turn keeps a bar on every changed block, and washes its unseen ones like any turn", async ({ page, birta }) => {
    // No rewrite row stands in for its changes any more, so even a state naming one washes them
    const state = fixtureState("rewrite");
    state.rewrite = { turn: 8, pct: 0.8, blocks_changed: 8, blocks_total: 10 };
    await open(page, birta, fixtureDoc("rewrite"), { state });
    await expect
      .poll(() => markKeys(page))
      .toEqual([
        "bar 10:1-10:29 solid",
        "bar 11:1-11:18 dotted",
        "bar 12:1-13:0 dotted",
        "bar 14:1-14:20 dotted",
        "bar 16:1-16:19 dotted",
        "bar 5:1-6:68 solid",
        "bar 8:1-8:20 dotted",
        "glyph 10:1-10:29 no",
        "glyph 5:1-6:68 no",
        "wash 10:1-10:29",
        "wash 11:1-11:18",
        "wash 12:1-13:0",
        "wash 14:1-14:20",
        "wash 16:1-16:19",
        "wash 5:1-6:68",
        "wash 8:1-8:20",
      ]);
  });

  test("your own change is the accent and a change to your edit is a double rule", async ({ page, birta }) => {
    const state = fixtureState("unseen");
    state.changes[0].attr = "human";
    state.changes[0].by = { type: "human", via: "browser" };
    state.changes[1].changes_your_edit = "3.0";
    await open(page, birta, fixtureDoc("unseen"), { state });
    await expect.poll(() => markKeys(page)).toContain("bar 11:1-11:44 double");
    const ms = await marks(page);
    const yours = only(ms, "bar", "5:1-6:68");
    expect(yours.style).toBe("solid");
    expect(yours.role).toBe("you");
    const glyph = only(ms, "glyph", "5:1-6:68");
    const x = await laneX(page);
    const px = await column(page, x, glyph.bottom + 2, yours.bottom - 1);
    expect(px.filter((p) => !near(p, YOU, 12)), "every pixel is the accent").toEqual([]);
    const double = only(ms, "bar", "11:1-11:44");
    expect(Math.abs(double.width - 6)).toBeLessThanOrEqual(0.5);
    expect(Math.abs((double.left + double.right) / 2 - x)).toBeLessThanOrEqual(1);
    const y = (double.top + double.bottom) / 2;
    const s = await shot(page, { left: double.left - 2, top: y - 1, width: 10, height: 2 });
    const row: Rgb[] = [];
    for (let i = Math.floor(double.left) - 1; i <= Math.ceil(double.right); i++) row.push(s.at(i, y));
    const a = row.findIndex((p) => near(p, YOU, 12));
    const g = row.findIndex((p) => near(p, AGENT, 12));
    expect(a, `accent in ${JSON.stringify(row)}`).toBeGreaterThanOrEqual(0);
    expect(g, `grey in ${JSON.stringify(row)}`).toBeGreaterThanOrEqual(0);
    const gap = row.slice(Math.min(a, g) + 1, Math.max(a, g));
    expect(gap.some((p) => near(p, WHITE, 12)), `gap in ${JSON.stringify(row)}`).toBe(true);
  });

  test("a removed block leaves a tick in the gap it left", async ({ page, birta }) => {
    const doc = "# Ticks\n\nAlpha paragraph.\n\nBeta paragraph.\n\nGamma paragraph.\n";
    const alpha = spAt(doc, "Alpha");
    await open(page, birta, doc, { state: laneState(doc, { ticks: [{ after: alpha }, { after: null }] }) });
    await expect.poll(() => markKeys(page)).toEqual([`tick after ${alpha}`, "tick after top"].sort());
    expect(await misaligned(page)).toEqual([]);
    const s = await shot(page, await frameBox(page));
    for (const t of (await marks(page)).filter((m) => m.mark === "tick")) {
      expect(ink(pixelsOf(s, t), WHITE), `tick after ${t.after} is painted`).not.toBeNull();
    }
  });

  test("m hides every mark and m again brings them back", async ({ page, birta }) => {
    // Tall enough that the last tick sits above the strip, which would cover its ink
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen"), viewport: { width: 1280, height: 1100 } });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const ms = await marks(page);
    const drawn = ms.filter((m) => ["bar", "glyph", "wash", "tick"].includes(m.mark));
    // A wash lies under text, so only the strip it reaches past its block is text-free
    const probe = (m: Mark): Box =>
      m.mark === "wash" ? { ...m, left: m.right - 6, right: m.right - 1, width: 5 } : m;
    const painted = async () => {
      const s = await shot(page, await frameBox(page));
      return drawn.filter((m) => ink(pixelsOf(s, probe(m)), WHITE) !== null).map((m) => `${m.mark} ${m.sp ?? m.after}`);
    };
    expect(await painted()).toHaveLength(drawn.length);
    await page.keyboard.press("m");
    await expect.poll(painted).toEqual([]);
    await page.keyboard.press("m");
    await expect.poll(async () => (await painted()).length).toBe(drawn.length);
  });

  test("with reduced motion nothing in the lane animates", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const durations = () =>
      page.evaluate(() => {
        const out: number[] = [];
        for (const tag of ["birta-under", "birta-over"]) {
          const root = document.querySelector(tag)!.shadowRoot!;
          root.querySelectorAll("*").forEach((el) => {
            for (const d of getComputedStyle(el).transitionDuration.split(",")) out.push(parseFloat(d));
          });
        }
        return out;
      });
    expect(Math.max(...(await durations()))).toBeGreaterThan(0);
    await page.emulateMedia({ reducedMotion: "reduce" });
    await expect.poll(async () => Math.max(...(await durations()))).toBe(0);
  });

  test("the dark theme paints with the dark roles", async ({ page, birta }) => {
    const doc = fixtureDoc("unseen");
    const { mock } = await open(page, birta, doc, { state: fixtureState("unseen"), args: ["pr.md", "--dark"] });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const ms = await marks(page);
    const frame = await frameBox(page);
    const s = await shot(page, frame);
    const bg = s.at(frame.left + 2, frame.top + 2);
    expect(near(bg, hex("#0d1117"))).toBe(true);
    const bar = only(ms, "bar", "5:1-6:68");
    const glyph = only(ms, "glyph", "5:1-6:68");
    const px = await column(page, await laneX(page), glyph.bottom + 2, bar.bottom - 1);
    expect(px.filter((p) => !near(p, FRESH_DARK, 12))).toEqual([]);
    const p = (await blockBox(page, "5:1-6:68"))!;
    expect(near(s.at(p.right + 4, p.top + 4), over("#bb800926", bg))).toBe(true);
    // The x has a crossing wide enough to hold the full colour in both engines
    mock.pushState(laneState(doc, { glyphs: [{ sp: "10:1-10:16", verdict: "no" }] }));
    await expect.poll(() => markKeys(page)).toEqual(["glyph 10:1-10:16 no"]);
    const g = only(await marks(page), "glyph", "10:1-10:16");
    expect(near((await inkIn(page, g, bg))!, DANGER_DARK, 10)).toBe(true);
  });

  test("under 768px the lane sits at x=12 with 12px glyphs, clear of the text", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen"), viewport: { width: 375, height: 1400 } });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    expect(await misaligned(page)).toEqual([]);
    const content = await contentBox(page);
    for (const m of await marks(page)) {
      if (m.mark === "glyph") expect(Math.abs(m.width - 12), `${m.sp} glyph width`).toBeLessThanOrEqual(1);
      if (["bar", "glyph", "tick"].includes(m.mark))
        expect(m.right, `${m.mark} ${m.sp ?? m.after} keeps 4px from the text`).toBeLessThanOrEqual(content.left - 4);
    }
  });
});

test.describe("the lane follows its blocks", () => {
  test("after an image above loads", async ({ page, birta }) => {
    const doc = "# Late image\n\n![late](late.svg)\n\nAlpha paragraph under the image.\n\nBeta paragraph.\n";
    const alpha = spAt(doc, "Alpha");
    const beta = spAt(doc, "Beta");
    const state = laneState(doc, {
      bars: [{ sp: alpha }, { sp: beta, asked: "unasked" }],
      glyphs: [{ sp: beta, verdict: "yes" }],
    });
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/local/late.svg", async (route) => {
      await held;
      await route.fulfill({ contentType: "image/svg+xml", body: TALL_SVG });
    });
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.setViewportSize(WIDE);
    await mockReview(page, { state });
    await page.goto(`${served.url}/view/pr.md`, { waitUntil: "domcontentloaded" });
    await booted(page);
    await expect.poll(() => markKeys(page)).toContain(`bar ${alpha} solid`);
    expect(await misaligned(page)).toEqual([]);
    const keys = await markKeys(page);
    const before = (await blockBox(page, alpha))!.top;
    release();
    await expect.poll(async () => (await blockBox(page, alpha))!.top - before).toBeGreaterThan(300);
    // A re-measure that lost the marks would be aligned too
    await expect
      .poll(async () => ({ keys: await markKeys(page), off: await misaligned(page) }), { timeout: 2_000 })
      .toEqual({ keys, off: [] });
  });

  test("after a web font above loads", async ({ page, birta }) => {
    const font = "https://cdn.jsdelivr.net/npm/katex@0.16.21/dist/fonts/KaTeX_Main-Regular.woff2";
    const doc = [
      "# Late font",
      "",
      `<style>@font-face{font-family:BirtaLate;src:url("${font}") format("woff2");size-adjust:300%;font-display:swap}.late{font-family:BirtaLate,serif}</style>`,
      "",
      '<p class="late">Text set in a late font, long enough to wrap once it grows.</p>',
      "",
      "Alpha paragraph.",
      "",
      "Beta paragraph.",
      "",
    ].join("\n");
    const alpha = spAt(doc, "Alpha");
    const state = laneState(doc, { bars: [{ sp: alpha }], glyphs: [{ sp: spAt(doc, "Beta"), verdict: "no" }] });
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/KaTeX_Main-Regular.woff2", async (route) => {
      await held;
      await route.fallback();
    });
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.setViewportSize(WIDE);
    await mockReview(page, { state });
    await page.goto(`${served.url}/view/pr.md`, { waitUntil: "domcontentloaded" });
    await booted(page);
    await expect.poll(() => markKeys(page)).toContain(`bar ${alpha} solid`);
    expect(await misaligned(page)).toEqual([]);
    const keys = await markKeys(page);
    const before = (await blockBox(page, alpha))!.top;
    release();
    await page.waitForFunction(() => document.fonts.check("16px BirtaLate"));
    await expect.poll(async () => (await blockBox(page, alpha))!.top - before).toBeGreaterThan(20);
    // A re-measure that lost the marks would be aligned too
    await expect
      .poll(async () => ({ keys: await markKeys(page), off: await misaligned(page) }), { timeout: 2_000 })
      .toEqual({ keys, off: [] });
  });

  test("after a diagram above renders", async ({ page, birta }) => {
    const doc1 = "# Diagram\n\nAlpha paragraph.\n\nBeta paragraph.\n";
    const doc2 = "# Diagram\n\n```mermaid\ngraph TD\n  A-->B\n  B-->C\n  C-->D\n```\n\nAlpha paragraph.\n\nBeta paragraph.\n";
    const alpha2 = spAt(doc2, "Alpha");
    const states = [
      laneState(doc1, { bars: [{ sp: spAt(doc1, "Alpha") }] }),
      laneState(doc2, { bars: [{ sp: alpha2 }], glyphs: [{ sp: spAt(doc2, "Beta"), verdict: "later" }] }),
    ];
    const { served } = await open(page, birta, doc1, { states });
    await expect.poll(() => markKeys(page)).toContain(`bar ${spAt(doc1, "Alpha")} solid`);
    await page.waitForFunction(() => typeof (window as { mermaid?: unknown }).mermaid !== "undefined");
    // Holds the diagram back, so it lands after the lane has measured the patched blocks
    await page.evaluate(() => {
      const w = window as unknown as { mermaid: { run: (o: unknown) => Promise<void> }; openMermaid: () => void };
      const gate = new Promise<void>((r) => (w.openMermaid = r));
      const run = w.mermaid.run.bind(w.mermaid);
      w.mermaid.run = (o) => gate.then(() => run(o));
    });
    served.write("pr.md", doc2);
    await expect.poll(() => markKeys(page)).toContain(`bar ${alpha2} solid`);
    expect(await misaligned(page)).toEqual([]);
    expect(await page.locator("pre.mermaid svg").count()).toBe(0);
    const keys = await markKeys(page);
    const before = (await blockBox(page, alpha2))!.top;
    await page.evaluate(() => (window as unknown as { openMermaid: () => void }).openMermaid());
    await expect(page.locator("pre.mermaid svg")).toBeAttached();
    await expect.poll(async () => Math.abs((await blockBox(page, alpha2))!.top - before)).toBeGreaterThan(20);
    // A re-measure that lost the marks would be aligned too
    await expect
      .poll(async () => ({ keys: await markKeys(page), off: await misaligned(page) }), { timeout: 2_000 })
      .toEqual({ keys, off: [] });
  });

  test("after a patch inserts blocks above", async ({ page, birta }) => {
    const doc1 = "# Patch\n\nAlpha paragraph.\n\nBeta paragraph.\n";
    const extra = Array.from({ length: 4 }, (_, i) => `Inserted ${i + 1}, a paragraph above the marks.`).join("\n\n");
    const doc2 = `# Patch\n\n${extra}\n\nAlpha paragraph.\n\nBeta paragraph.\n`;
    const lane = (doc: string) =>
      laneState(doc, {
        bars: [{ sp: spAt(doc, "Alpha") }, { sp: spAt(doc, "Beta"), asked: "unasked" }],
        glyphs: [{ sp: spAt(doc, "Beta"), verdict: "no" }],
      });
    const { served } = await open(page, birta, doc1, { states: [lane(doc1), lane(doc2)] });
    await expect.poll(() => markKeys(page)).toContain(`bar ${spAt(doc1, "Alpha")} solid`);
    const before = (await blockBox(page, spAt(doc1, "Alpha")))!.top;
    served.write("pr.md", doc2);
    await expect.poll(() => markKeys(page)).toContain(`bar ${spAt(doc2, "Alpha")} solid`);
    expect((await blockBox(page, spAt(doc2, "Alpha")))!.top - before).toBeGreaterThan(80);
    await expect.poll(() => misaligned(page), { timeout: 2_000 }).toEqual([]);
  });

  test("after the window is resized", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const keys = await markKeys(page);
    const before = (await blockBox(page, "5:1-6:68"))!.height;
    await page.setViewportSize({ width: 500, height: 1400 });
    await expect.poll(async () => (await blockBox(page, "5:1-6:68"))!.height - before).toBeGreaterThan(10);
    // A re-measure that lost the marks would be aligned too
    await expect
      .poll(async () => ({ keys: await markKeys(page), off: await misaligned(page) }), { timeout: 2_000 })
      .toEqual({ keys, off: [] });
  });
});

test.describe("the doc cannot reach the lane", () => {
  test("doc CSS cannot hide the lane", async ({ page, birta }) => {
    const doc = `${fixtureDoc("unseen")}\n<style>birta-under, birta-over { display: none !important; visibility: hidden !important; opacity: 0 !important; }</style>\n`;
    await open(page, birta, doc, { state: withRev(fixtureState("unseen"), revOf(doc)) });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const ms = await marks(page);
    const bar = only(ms, "bar", "5:1-6:68");
    const glyph = only(ms, "glyph", "5:1-6:68");
    const px = await column(page, await laneX(page), glyph.bottom + 2, bar.bottom - 1);
    expect(px.filter((p) => !near(p, FRESH, 12))).toEqual([]);
    const p = (await blockBox(page, "5:1-6:68"))!;
    const s = await shot(page, await frameBox(page));
    expect(near(s.at(p.right + 4, p.top + 4), NEW)).toBe(true);
  });

  test("raw HTML cannot paint outside the doc's box", async ({ page, birta }) => {
    const red = "background:rgb(255,0,0)";
    const doc = [
      fixtureDoc("unseen"),
      `<div style="position:fixed;inset:0;${red}"></div>`,
      "",
      `<div style="position:absolute;left:-40px;top:0;width:40px;height:3000px;${red}"></div>`,
      "",
      `<div style="margin:0 -40px;height:30px;${red}"></div>`,
      "",
    ].join("\n");
    await open(page, birta, doc, { state: withRev(fixtureState("unseen"), revOf(doc)) });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const frame = await frameBox(page);
    const box = await contentBox(page);
    const s = await shot(page, frame);
    const RED = hex("#ff0000");
    // The raw HTML did render, inside the box
    expect(near(s.at(box.left + box.width / 2, box.top + 20), RED)).toBe(true);
    const leaks: string[] = [];
    // The doc's rows can run past the viewport, and the shot stops there
    const bottom = Math.min(frame.bottom, page.viewportSize()!.height);
    for (let y = Math.ceil(frame.top) + 1; y < bottom - 1; y += 7) {
      for (const x of [frame.left + 2, frame.left + 10, frame.left + 30, frame.right - 30, frame.right - 3]) {
        if (near(s.at(x, y), RED, 40)) leaks.push(`${x},${y}`);
      }
    }
    expect(leaks).toEqual([]);
  });
});

test.describe("the doc cannot reach the lane, however hard it tries", () => {
  test("doc CSS cannot paint over the wash", async ({ page, birta }) => {
    const doc = `${fixtureDoc("unseen")}\n<style>#content, .markdown-body { background: #fff !important; }</style>\n`;
    await open(page, birta, doc, { state: withRev(fixtureState("unseen"), revOf(doc)) });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const p = (await blockBox(page, "5:1-6:68"))!;
    const s = await shot(page, await frameBox(page));
    expect(near(s.at(p.right - 4, p.bottom - 4), NEW_VEILED)).toBe(true);
  });

  test("doc CSS cannot lift the doc over the lane", async ({ page, birta }) => {
    const css =
      "#content { position: relative !important; z-index: 9 !important; margin-left: -40px !important; padding-left: 40px !important; background: #fff !important; }";
    const doc = `${fixtureDoc("unseen")}\n<style>${css}</style>\n`;
    await open(page, birta, doc, { state: withRev(fixtureState("unseen"), revOf(doc)) });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const ms = await marks(page);
    const bar = only(ms, "bar", "5:1-6:68");
    const glyph = only(ms, "glyph", "5:1-6:68");
    const px = await column(page, await laneX(page), glyph.bottom + 2, bar.bottom - 1);
    expect(px.filter((p) => !near(p, FRESH, 12))).toEqual([]);
  });

  test("doc CSS cannot shrink, clip or filter the lane hosts", async ({ page, birta }) => {
    const css =
      "birta-under, birta-over { width: 0 !important; height: 0 !important; overflow: hidden !important; clip-path: inset(50%) !important; filter: opacity(0) !important; mask-image: linear-gradient(transparent, transparent) !important; content-visibility: hidden !important; }";
    const doc = `${fixtureDoc("unseen")}\n<style>${css}</style>\n`;
    await open(page, birta, doc, { state: withRev(fixtureState("unseen"), revOf(doc)) });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const ms = await marks(page);
    const bar = only(ms, "bar", "5:1-6:68");
    const glyph = only(ms, "glyph", "5:1-6:68");
    const px = await column(page, await laneX(page), glyph.bottom + 2, bar.bottom - 1);
    expect(px.filter((p) => !near(p, FRESH, 12))).toEqual([]);
    const p = (await blockBox(page, "5:1-6:68"))!;
    const s = await shot(page, await frameBox(page));
    expect(near(s.at(p.right + 4, p.top + 4), NEW)).toBe(true);
  });
});

test.describe("the doc cannot hide or recolour the notes", () => {
  const HIDE =
    "clip-path: inset(50%) !important; filter: opacity(0) !important; opacity: 0 !important; translate: -9999px 0 !important; rotate: 90deg !important; scale: 0 !important; zoom: 0.01 !important; offset-path: path('M 0 0 L -9999 0') !important; mix-blend-mode: screen !important; mask-image: linear-gradient(transparent, transparent) !important; -webkit-mask-image: linear-gradient(transparent, transparent) !important; content-visibility: hidden !important; visibility: hidden !important;";
  const VARS =
    "--review-agent: transparent !important; --review-you: transparent !important; --review-new: transparent !important; --review-wash: transparent !important; --birta-fg: transparent !important; --birta-fg-muted: transparent !important; --birta-bg: transparent !important;";
  const css = [
    `birta-flow, birta-under, birta-over, birta-phrase, birta-turn, birta-pop { ${HIDE} }`,
    `:root, html, body, #lane-frame, #content, li, birta-flow, birta-under, birta-over, birta-phrase, birta-turn, birta-pop { ${VARS} }`,
  ].join("\n");
  const doc = `${fixtureDoc("unseen")}\n<style>${css}</style>\n`;
  const ITEM = "11:1-11:44";

  test("doc CSS cannot hide, move or blend away a note, the lane or their colours", async ({ page, birta }) => {
    await open(page, birta, doc, { state: withRev(fixtureState("unseen"), revOf(doc)) });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    // The lane: the agent's fresh bar, and the new block's wash
    const ms = await marks(page);
    const bar = only(ms, "bar", "5:1-6:68");
    const glyph = only(ms, "glyph", "5:1-6:68");
    const px = await column(page, await laneX(page), glyph.bottom + 2, bar.bottom - 1);
    expect(px.filter((p) => !near(p, FRESH, 12))).toEqual([]);
    const p = (await blockBox(page, "5:1-6:68"))!;
    const s = await shot(page, await frameBox(page));
    expect(near(s.at(p.right + 4, p.top + 4), NEW)).toBe(true);
    // The row under the item, open while it has focus: laid out where it belongs, and painted there
    for (let i = 0; i < 20 && (await focused(page)) !== ITEM; i++) await page.keyboard.press("j");
    const host = page.locator(`#content [data-sourcepos="${ITEM}"] > birta-flow`, { hasText: "without a note" });
    await expect(host).toHaveCount(1);
    const styles = await host.evaluate((h) => {
      const cs = getComputedStyle(h);
      const line = h.shadowRoot!.querySelector(".text")!;
      return {
        clip: cs.clipPath,
        filter: cs.filter,
        opacity: cs.opacity,
        translate: cs.translate,
        rotate: cs.rotate,
        scale: cs.scale,
        offset: cs.getPropertyValue("offset-path"),
        blend: cs.mixBlendMode,
        visibility: cs.visibility,
        mask: cs.maskImage || cs.getPropertyValue("-webkit-mask-image"),
        text: getComputedStyle(line).color,
      };
    });
    expect(styles).toMatchObject({
      clip: "none",
      filter: "none",
      opacity: "1",
      translate: "none",
      rotate: "none",
      scale: "none",
      blend: "normal",
      visibility: "visible",
      mask: "none",
    });
    expect(["none", ""]).toContain(styles.offset);
    expect(styles.text).not.toMatch(/rgba\(0, 0, 0, 0\)|transparent/);
    const item = (await blockBox(page, ITEM))!;
    const hb = await host.evaluate((h) => {
      const r = h.getBoundingClientRect();
      return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
    });
    expect(hb.top).toBeGreaterThanOrEqual(item.top - 1);
    expect(hb.left).toBeGreaterThanOrEqual(item.left - 1);
    expect(hb.height).toBeGreaterThan(10);
    expect(await inkIn(page, hb, WHITE)).not.toBeNull();
    // The hosts carry the theme's own colours, whatever the doc said
    const vars = await page.evaluate(() =>
      ["birta-over", "birta-flow"].map((t) => getComputedStyle(document.querySelector(t)!).getPropertyValue("--review-agent").trim()),
    );
    expect(vars.map((v) => near(hex(v.length === 4 ? `#${v[1]}${v[1]}${v[2]}${v[2]}${v[3]}${v[3]}` : v), AGENT, 2))).toEqual([true, true]);
  });
});

test.describe("lane edges", () => {
  test("a removed item leaves its tick between the items around it", async ({ page, birta }) => {
    const doc = "# Ticks\n\n- First item\n- Second item\n- Third item\n\nAfter the list.\n";
    const first = spAt(doc, "First item");
    await open(page, birta, doc, { state: laneState(doc, { ticks: [{ after: first }] }) });
    await expect.poll(() => markKeys(page)).toEqual([`tick after ${first}`]);
    expect(await misaligned(page)).toEqual([]);
    const tick = (await marks(page)).find((m) => m.mark === "tick")!;
    const [a, b] = await page.evaluate(() =>
      Array.from(document.querySelectorAll("#content li"))
        .slice(0, 2)
        .map((li) => ({ top: li.getBoundingClientRect().top, bottom: li.getBoundingClientRect().bottom })),
    );
    const cy = (tick.top + tick.bottom) / 2;
    expect(cy).toBeGreaterThanOrEqual(a.bottom - 1);
    expect(cy).toBeLessThanOrEqual(b.top + 1);
  });

  test("marks on a block that is not rendered paint nothing outside the frame", async ({ page, birta }) => {
    const doc = "# Hidden\n\n<details>\n<summary>More</summary>\n\nHidden paragraph.\n\n</details>\n\nShown paragraph.\n";
    const hidden = spAt(doc, "Hidden paragraph");
    const shown = spAt(doc, "Shown paragraph");
    const state = laneState(doc, {
      bars: [{ sp: hidden }, { sp: shown }],
      glyphs: [{ sp: hidden, verdict: "no" }],
      ticks: [{ after: hidden }],
    });
    await open(page, birta, doc, { state });
    await expect.poll(() => markKeys(page)).toContain(`bar ${shown} solid`);
    const frame = await frameBox(page);
    const outside = (await marks(page)).filter(
      (m) =>
        m.width > 0 &&
        m.height > 0 &&
        (m.top < frame.top || m.bottom > frame.bottom || m.left < frame.left || m.right > frame.right),
    );
    expect(outside.map((m) => `${m.mark} ${m.sp ?? m.after}`)).toEqual([]);
  });

  test("a wash on a wide table row stays inside the frame", async ({ page, birta }) => {
    const cols = Array.from({ length: 30 }, (_, i) => `column ${i}`);
    const doc = `# Wide\n\n| ${cols.join(" | ")} |\n|${cols.map(() => "---").join("|")}|\n| ${cols.map((c) => `${c} value`).join(" | ")} |\n`;
    const { mock } = await open(page, birta, doc);
    const sp = await page.evaluate(() => document.querySelector("#content tbody tr")!.getAttribute("data-sourcepos")!);
    expect(sp, "table rows carry a sourcepos").toBeTruthy();
    mock.pushState(laneState(doc, { bars: [{ sp }] }));
    await expect.poll(() => markKeys(page)).toContain(`wash ${sp}`);
    const frame = await frameBox(page);
    const wash = (await marks(page)).find((m) => m.mark === "wash")!;
    expect(wash.right).toBeLessThanOrEqual(frame.right);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });

  test("two glyphs on one block stack in note order", async ({ page, birta }) => {
    const doc = "# Stack\n\nAlpha paragraph.\n";
    const alpha = spAt(doc, "Alpha");
    await open(page, birta, doc, {
      state: laneState(doc, { glyphs: [{ sp: alpha, verdict: "no" }, { sp: alpha, verdict: "ask" }] }),
    });
    await expect.poll(() => markKeys(page)).toEqual([`glyph ${alpha} ask`, `glyph ${alpha} no`]);
    expect(await misaligned(page)).toEqual([]);
    const [first, second] = (await marks(page)).filter((m) => m.mark === "glyph").sort((a, b) => a.n! - b.n!);
    expect(first.verdict).toBe("no");
    expect(second.top).toBeGreaterThanOrEqual(first.bottom);
  });

  for (const [status, viewport, double] of [
    ["draft", WIDE, false],
    ["sent", WIDE, false],
    ["sent", { width: 500, height: 900 }, false],
    ["sent", WIDE, true],
    ["sent", { width: 500, height: 900 }, true],
  ] as const) {
    const bar = double ? "double rule" : "change bar";
    test(`a ${status} verdict glyph and a ${bar} on one line sit side by side at ${viewport.width}px`, async ({
      page,
      birta,
    }) => {
      const doc = fixtureDoc("fresh");
      const sp = spAt(doc, "- retries on 503");
      const state = laneState(doc, {
        bars: [{ sp, seen: true, yourEdit: double }],
        glyphs: [{ sp, verdict: "rewrite", status }],
      });
      await open(page, birta, doc, { state, viewport });
      await expect.poll(() => markKeys(page)).toEqual([`bar ${sp} ${double ? "double" : "solid"}`, `glyph ${sp} rewrite`]);
      const ms = await marks(page);
      const bar = only(ms, "bar", sp);
      const glyph = only(ms, "glyph", sp);
      // A seen change is a kept one: its bar is checked, muted
      expect(bar.check).toBe("checked");
      // Two marks with daylight between them, never one merged shape
      const gap = Math.max(bar.left - glyph.right, glyph.left - bar.right);
      expect(gap, `bar ${JSON.stringify(bar)} glyph ${JSON.stringify(glyph)}`).toBeGreaterThanOrEqual(2);
      const b = (await blockBox(page, sp))!;
      expect(Math.abs(bar.top - b.top) <= 1 && Math.abs(bar.bottom - b.bottom) <= 1, `${JSON.stringify(bar)}`).toBe(true);
      expect(await misaligned(page)).toEqual([]);
      // A verdict with no colour of its own is muted, drawn or sent
      const glyphInk = (await inkIn(page, glyph, WHITE))!;
      expect(near(glyphInk, AGENT, 10), `${glyphInk}`).toBe(true);
      const grey = async (x: number) => {
        const px = await column(page, x, bar.top + 1, bar.bottom - 1);
        expect(px.length).toBeGreaterThan(8);
        return px.filter((p) => !near(p, AGENT, 12));
      };
      if (double && viewport.width >= 768) {
        // The whole rule: your colour, a gap, then the agent's
        const yours = await column(page, bar.left + 1, bar.top + 1, bar.bottom - 1);
        expect(yours.filter((p) => !near(p, YOU, 12)), "every pixel is the accent").toEqual([]);
        expect(await grey(bar.right - 1), "every pixel is the agent grey").toEqual([]);
      } else {
        // One rule the whole block long; a double rule squeezed beside a glyph keeps the agent's half
        expect(bar.width).toBeLessThanOrEqual(double ? 2 : 3);
        expect(await grey((bar.left + bar.right) / 2), "every pixel is the agent grey").toEqual([]);
      }
    });
  }

  test("each verdict draws its own icon", async ({ page, birta }) => {
    const verdicts = ["yes", "no", "ask", "later", "your_call"];
    const doc = `# Icons\n\n${verdicts.map((v) => `Paragraph for ${v}.`).join("\n\n")}\n`;
    const state = laneState(doc, {
      glyphs: verdicts.map((v) => ({ sp: spAt(doc, `for ${v}.`), verdict: v, status: "sent" })),
    });
    await open(page, birta, doc, { state });
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "glyph").length).toBe(verdicts.length);
    const ms = await marks(page);
    const s = await shot(page, await frameBox(page));
    // The inner 8×8 of each glyph: question and clock share the octicon ring, so it can't tell them apart
    const masks = verdicts.map((v) => {
      const g = ms.find((m) => m.mark === "glyph" && m.verdict === v)!;
      const out: number[] = [];
      for (let dy = 4; dy < 12; dy++)
        for (let dx = 4; dx < 12; dx++) out.push(distance(s.at(g.left + dx + 0.5, g.top + dy + 0.5), WHITE) > 60 ? 1 : 0);
      return out;
    });
    for (let i = 0; i < masks.length; i++)
      for (let j = i + 1; j < masks.length; j++) {
        const differ = masks[i].filter((b, k) => b !== masks[j][k]).length;
        expect(differ, `${verdicts[i]} and ${verdicts[j]} look alike`).toBeGreaterThan(masks[i].length * 0.15);
      }
  });

  test("a custom theme's note colour is the review accent", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("fresh"), { args: ["pr.md", "--theme", "dracula"] });
    const got = await page.evaluate(() => {
      const cs = getComputedStyle(document.querySelector("#lane-frame > birta-over")!);
      const v = (n: string) => cs.getPropertyValue(n).trim().toLowerCase();
      return { you: v("--review-you"), note: v("--alert-note") };
    });
    expect(got.note).not.toBe("");
    expect(got.you).toBe(got.note);
  });

  test("printed pages carry no marks", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const bar = only(await marks(page), "bar", "5:1-6:68");
    await page.emulateMedia({ media: "print" });
    const frame = await frameBox(page);
    const s = await shot(page, frame);
    const bg = s.at(frame.left + 2, bar.top + 2);
    expect(ink(pixelsOf(s, bar), bg)).toBeNull();
  });

  test("wide display math scrolls inside the doc instead of being clipped", async ({ page, birta }) => {
    const doc = `# Math\n\n$$\n${Array.from({ length: 120 }, (_, i) => `x_{${i}}`).join(" + ")}\n$$\n\nAfter.\n`;
    await open(page, birta, doc);
    const el = page.locator("#content .katex-display").first();
    await expect(el).toBeAttached({ timeout: 30_000 });
    const got = await el.evaluate((d) => {
      const content = document.getElementById("content")!.getBoundingClientRect();
      const r = d.getBoundingClientRect();
      d.scrollLeft = 200;
      return {
        wide: d.scrollWidth > d.clientWidth,
        overflow: getComputedStyle(d).overflowX,
        inside: r.left >= content.left - 0.5 && r.right <= content.right + 0.5,
        scrolled: d.scrollLeft > 0,
      };
    });
    expect(got.wide).toBe(true);
    expect(["auto", "scroll"]).toContain(got.overflow);
    expect(got.inside).toBe(true);
    expect(got.scrolled).toBe(true);
  });

  test("Tab never reaches a heading's hidden link icon, and the overlay still shows it when focused", async ({
    page,
    birta,
    browserName,
  }) => {
    await open(page, birta, fixtureDoc("fresh"), { state: fixtureState("fresh") });
    // WebKit on macOS moves focus to links only with Option held, as Safari does
    const tab = browserName === "webkit" ? "Alt+Tab" : "Tab";
    const anchorSp = () =>
      page.evaluate(() => {
        const a = document.activeElement;
        return a && a.matches("#content a.anchor") ? a.closest("h1,h2,h3,h4,h5,h6")!.getAttribute("data-sourcepos") : null;
      });
    for (let i = 0; i < 40; i++) {
      await page.keyboard.press(tab);
      expect(await anchorSp(), `Tab ${i + 1} lands on no heading link`).toBeNull();
    }
    await page.locator("#content a.anchor").first().focus();
    const sp = await anchorSp();
    expect(sp, "a heading link can still take focus").not.toBeNull();
    await expect.poll(async () => inkIn(page, only(await marks(page), "anchor", sp!), WHITE)).not.toBeNull();
  });

  test("under 768px a heading's link icon stays inside the frame", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("fresh"), { state: fixtureState("fresh"), viewport: { width: 375, height: 900 } });
    await page.hover("#content h2 >> nth=0");
    const frame = await frameBox(page);
    const icon = only(await marks(page), "anchor", "3:1-3:6");
    expect(icon.left).toBeGreaterThanOrEqual(frame.left - 0.5);
    expect(icon.right).toBeLessThanOrEqual(frame.right + 0.5);
    expect(await inkIn(page, icon, WHITE)).not.toBeNull();
  });
});

test.describe("at 1.5 device pixels per CSS pixel", () => {
  test.use({ deviceScaleFactor: 1.5 });

  test("the unseen fixture's marks still sit on their blocks", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    expect(await misaligned(page)).toEqual([]);
  });
});

test.describe("lane cost", () => {
  /** Counts mutations inside both lane roots from now on. */
  async function watchLane(page: Page): Promise<void> {
    await page.evaluate(() => {
      const w = window as unknown as { laneMutations: { sp: string | null; mark: string | null }[] };
      w.laneMutations = [];
      for (const tag of ["birta-under", "birta-over"]) {
        new MutationObserver((records) => {
          for (const r of records) {
            const el = (r.target.nodeType === 1 ? r.target : r.target.parentElement) as Element | null;
            const mark = el?.closest("[data-mark]") ?? null;
            w.laneMutations.push({ sp: mark?.getAttribute("data-sp") ?? null, mark: mark?.getAttribute("data-mark") ?? null });
          }
        }).observe(document.querySelector(tag)!.shadowRoot!, { subtree: true, childList: true, attributes: true, characterData: true });
      }
    });
  }

  function laneMutations(page: Page) {
    return page.evaluate(() => (window as unknown as { laneMutations: { sp: string | null; mark: string | null }[] }).laneMutations);
  }

  test("frames and moves that change nothing leave the lane alone", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    await page.waitForTimeout(300);
    await watchLane(page);
    mock.push({ type: "ack", cid: "not-ours", n: 1 });
    mock.push({ type: "dir_state", root: ".", truncated: false, folder: null, files: [], totals: { drafts: 0, unseen_files: 0, decided: 0, items: 0 } });
    for (const sp of ["5:1-6:68", "10:1-10:16", "11:1-11:44"]) {
      const b = (await blockBox(page, sp))!;
      await page.mouse.move(b.left + 20, (b.top + b.bottom) / 2);
    }
    await page.waitForTimeout(500);
    expect(await laneMutations(page)).toEqual([]);
  });

  test("hovering a heading touches only its own link icon", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    await page.mouse.move(1, 1);
    await page.waitForTimeout(300);
    await watchLane(page);
    await page.hover("#content h2 >> nth=0");
    await page.waitForTimeout(300);
    const seen = await laneMutations(page);
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.filter((m) => !(m.mark === "anchor" && m.sp === "3:1-3:6"))).toEqual([]);
  });

  test("500 marks draw within one 200 ms task", async ({ page, birta, browserName }) => {
    test.skip(browserName !== "chromium", "long tasks are reported only by chromium");
    test.skip(!!process.env.CI, "timing on shared CI runners is noisy");
    const doc = `# Many\n\n${Array.from({ length: 500 }, (_, i) => `Paragraph number ${i}.`).join("\n\n")}\n`;
    const sps = Array.from({ length: 500 }, (_, i) => spAt(doc, `number ${i}.`));
    const { mock } = await open(page, birta, doc);
    await page.evaluate(() => {
      const w = window as unknown as { longest: number };
      w.longest = 0;
      new PerformanceObserver((list) => {
        for (const e of list.getEntries()) w.longest = Math.max(w.longest, e.duration);
      }).observe({ type: "longtask", buffered: false });
    });
    mock.pushState(laneState(doc, { bars: sps.map((sp) => ({ sp })) }));
    await expect.poll(async () => (await marks(page)).filter((m) => m.mark === "bar").length).toBe(500);
    await page.waitForTimeout(300);
    expect(await page.evaluate(() => (window as unknown as { longest: number }).longest)).toBeLessThan(200);
  });
});
