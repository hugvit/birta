import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  STOPS_DOC,
  State,
  booted,
  fixtureDoc,
  fixtureState,
  focusTo,
  focused,
  longDoc,
  mockReview,
  stopSp,
  stops,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
/** How long a key that should do nothing is given to do something anyway. */
const SETTLE_MS = 400;
const SOURCE_SAYS = "source view · esc back";
const READING_SAYS = "reading mode · esc back";
const CONTROL_SAYS = "on page controls · esc back to the doc";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

interface OpenOpts {
  state?: State;
  args?: string[];
  env?: Record<string, string>;
  files?: Record<string, string>;
  path?: string;
}

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, opts: OpenOpts = {}) {
  const served = await birta({ "pr.md": doc, ...(opts.files ?? {}) }, opts.env ?? GATE_ON, opts.args);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state: opts.state });
  await page.goto(`${served.url}/view/${opts.path ?? "pr.md"}`);
  await booted(page);
  return { served, mock };
}

/** WebKit on macOS moves focus to links and buttons only with Option held, as Safari does. */
function tabKey(browserName: string, shift = false): string {
  const tab = browserName === "webkit" ? "Alt+Tab" : "Tab";
  return shift ? `Shift+${tab}` : tab;
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

/** The phrase menu's rows while it is open, and which one is current. */
function menuRows(page: Page): Promise<{ rows: string[]; current: number } | null> {
  return page.evaluate(() => {
    for (const host of document.querySelectorAll("birta-pop")) {
      const root = host.shadowRoot ?? host;
      for (const el of root.querySelectorAll('[role="menu"]')) {
        let open = false;
        try {
          open = el.matches(":popover-open") && el.getClientRects().length > 0;
        } catch {
          open = false;
        }
        if (!open) continue;
        const items = Array.from(el.querySelectorAll('[role="menuitem"]'));
        return {
          rows: items.map((r) => (r.querySelector(".label")?.textContent ?? "").trim()),
          current: items.findIndex((r) => r.classList.contains("current")),
        };
      }
    }
    return null;
  });
}

const sourceShown = (page: Page) => page.locator("#source-view").isVisible();
const docShown = (page: Page) => page.locator("#lane-frame").isVisible();
const readingMode = (page: Page) => page.evaluate(() => document.body.classList.contains("reading-mode"));
const marksOff = (page: Page) => page.evaluate(() => document.documentElement.getAttribute("data-br-marks") === "off");

/** The page control holding DOM focus, by id or class, or null when focus is on the doc or nothing. */
function control(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const a = document.activeElement;
    if (!a || a === document.body || document.getElementById("content")!.contains(a)) return null;
    if (!a.matches("button, select, input, [role='button'], birta-phrase, birta-turn")) return null;
    return a.id || a.localName + (a.className ? "." + String(a.className).split(" ")[0] : "");
  });
}

/** What holds DOM focus outside the doc, for a Tab sweep: an id, a class or a tag. */
function activeName(page: Page): Promise<string> {
  return page.evaluate(() => {
    const a = document.activeElement;
    if (!a || a === document.body) return "body";
    const where = document.getElementById("content")!.contains(a) ? "doc " : "";
    if (a.matches("a.anchor")) return where + "a.anchor";
    return where + (a.id ? "#" + a.id : a.localName + (a.className ? "." + String(a.className).split(" ")[0] : ""));
  });
}

/** Whether the block at `sp` is fully between the header and the window's bottom. */
function inView(page: Page, sp: string): Promise<boolean> {
  return page.evaluate((sp) => {
    const el = document.querySelector(`#content [data-sourcepos="${sp}"]`);
    const header = document.querySelector(".file-header");
    if (!el || !header) return false;
    const r = el.getBoundingClientRect();
    return r.top >= header.getBoundingClientRect().bottom - 1 && r.bottom <= innerHeight + 1;
  }, sp);
}

/** Ops that write the review file, without their frame fields. */
function writes(ops: State[]): State[] {
  return ops
    .filter((o) => o.op !== "seen")
    .map((o) => {
      const out = { ...o };
      for (const k of ["type", "cid", "path", "rev"]) delete out[k];
      return out;
    });
}

// --- ⌥u: the source view's key in review tabs --------------------------------------------------

test.describe("⌥u", () => {
  test("a review tab serves the source view on ⌥u, off the revert's u; a plain tab keeps u", async ({ page, birta }) => {
    const keys = () =>
      page.evaluate(() => {
        const kb = (window as unknown as { KEYBINDINGS: Record<string, string> }).KEYBINDINGS;
        return { toggle_raw: kb.toggle_raw, revert_change: kb.revert_change };
      });
    await open(page, birta, STOPS_DOC);
    expect(await keys()).toEqual({ toggle_raw: "Alt+u", revert_change: "u" });

    const plain = await birta({ "pr.md": STOPS_DOC }, {});
    await page.goto(`${plain.url}/view/pr.md`);
    await expect(page.locator("html[data-review]")).toHaveCount(0);
    expect((await keys()).toggle_raw).toBe("u");

    // A folder under review gets the same
    await open(page, birta, STOPS_DOC, { args: ["."] });
    expect(await keys()).toEqual({ toggle_raw: "Alt+u", revert_change: "u" });

    // A source-view key no review action uses stays as bound
    await open(page, birta, STOPS_DOC, { args: ["pr.md", "--bind", "toggle_raw=s"] });
    expect((await keys()).toggle_raw).toBe("s");
  });

  for (const where of ["a file", "a folder"]) {
    test(`in ${where}, ⌥u shows the source and says how back, and ⌥u again returns to the focused block`, async ({
      page,
      birta,
    }) => {
      await open(page, birta, longDoc(), where === "a folder" ? { args: ["."] } : {});
      const target = await stopSp(page, "Filler paragraph 20.");
      await focusTo(page, target);
      await page.keyboard.press("Alt+KeyU");
      await expect(page.locator("#source-view")).toBeVisible();
      await expect(page.locator("#lane-frame")).toBeHidden();
      await expect.poll(() => pillOf(page)).toBe(SOURCE_SAYS);
      await page.keyboard.press("Alt+KeyU");
      await expect(page.locator("#lane-frame")).toBeVisible();
      await expect.poll(() => focused(page)).toBe(target);
      await expect.poll(() => inView(page, target)).toBe(true);
      // No word for the way back in: the doc is the answer
      await expect.poll(() => pillOf(page)).not.toBe(SOURCE_SAYS);
      expect(await pillOf(page)).not.toBe("rendered view");
    });
  }

  test("u never shows the source in a review tab: on a plain block it says nothing here to revert", async ({
    page,
    birta,
  }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await focusTo(page, await stopSp(page, "Detail words."));
    await page.keyboard.press("u");
    await expect.poll(() => pillOf(page)).toBe("nothing here to revert");
    await page.waitForTimeout(SETTLE_MS);
    expect(await sourceShown(page)).toBe(false);
    expect(writes(mock.ops())).toEqual([]);
  });

  test("the Source button keeps the focus the way ⌥u does, and says the same", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const target = await stopSp(page, "Filler paragraph 20.");
    await focusTo(page, target);
    await page.click('#view-toggle .view-btn[data-view="raw"]');
    await expect(page.locator("#source-view")).toBeVisible();
    await expect.poll(() => pillOf(page)).toBe(SOURCE_SAYS);
    await page.click('#view-toggle .view-btn[data-view="preview"]');
    await expect(page.locator("#lane-frame")).toBeVisible();
    await expect.poll(() => focused(page)).toBe(target);
    await expect.poll(() => inView(page, target)).toBe(true);
  });

  test("the viewer tells the page of each view change, and lets review change the view", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    await page.evaluate(() => {
      const w = window as unknown as { __views: unknown[] };
      w.__views = [];
      document.addEventListener("birta-view", (e) => {
        const d = (e as CustomEvent).detail ?? {};
        w.__views.push({ mode: d.mode, reading: d.reading });
      });
    });
    const views = () => page.evaluate(() => (window as unknown as { __views: unknown[] }).__views);
    await page.keyboard.press("Alt+KeyU");
    await page.keyboard.press("Alt+KeyU");
    await page.keyboard.press("r");
    await page.keyboard.press("r");
    await expect
      .poll(views)
      .toEqual([
        { mode: "raw", reading: false },
        { mode: "preview", reading: false },
        { mode: "preview", reading: true },
        { mode: "preview", reading: false },
      ]);
    const api = await page.evaluate(() => {
      const v = (window as unknown as { birtaView?: Record<string, unknown> }).birtaView;
      return { setViewMode: typeof v?.setViewMode, toggleReadingMode: typeof v?.toggleReadingMode };
    });
    expect(api).toEqual({ setViewMode: "function", toggleReadingMode: "function" });
  });
});

// --- Review keys in source view and reading mode: leave the mode, then act ---------------------

test.describe("review keys in a mode", () => {
  test("j in source view returns to the doc and moves the focus", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const from = await stopSp(page, "Filler paragraph 20.");
    const to = await stopSp(page, "Filler paragraph 21.");
    await focusTo(page, from);
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("j");
    await expect(page.locator("#lane-frame")).toBeVisible();
    expect(await sourceShown(page)).toBe(false);
    await expect.poll(() => focused(page)).toBe(to);
    await expect.poll(() => inView(page, to)).toBe(true);
  });

  for (const mode of ["source view", "reading mode"]) {
    const enter = mode === "source view" ? "Alt+KeyU" : "r";
    const inMode = (page: Page) => (mode === "source view" ? sourceShown(page) : readingMode(page));

    test(`in ${mode}, x leaves it and says no to the focused block`, async ({ page, birta }) => {
      const { mock } = await open(page, birta, STOPS_DOC);
      const sp = await stopSp(page, "Detail words.");
      await focusTo(page, sp);
      await page.keyboard.press(enter);
      await expect.poll(() => inMode(page)).toBe(true);
      await page.keyboard.press("x");
      await expect.poll(() => inMode(page)).toBe(false);
      await expect
        .poll(() => writes(mock.ops()))
        .toEqual([{ op: "comment", at: { k: "block", sp, scope: "block" }, verdict: "no" }]);
      expect(await focused(page)).toBe(sp);
    });

    test(`in ${mode}, c leaves it and opens the composer on the focused block, in view`, async ({ page, birta }) => {
      await open(page, birta, longDoc());
      const sp = await stopSp(page, "Filler paragraph 20.");
      await focusTo(page, sp);
      // Mid-view, so the composer's room below the block is the mode's to give back, not the composer's own placement
      await page.evaluate((sp) => {
        const r = document.querySelector(`#content [data-sourcepos="${sp}"]`)!.getBoundingClientRect();
        window.scrollBy(0, r.top - 300);
      }, sp);
      await page.waitForTimeout(SETTLE_MS);
      await page.keyboard.press(enter);
      await expect.poll(() => inMode(page)).toBe(true);
      await page.keyboard.press("c");
      const area = page.locator('birta-flow[data-kind="composer"] textarea');
      await expect(area).toBeFocused();
      await expect(area).toBeInViewport();
      expect(await inMode(page)).toBe(false);
      expect(await focused(page)).toBe(sp);
    });

    test(`in ${mode}, m leaves it and hides the marks`, async ({ page, birta }) => {
      await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
      await page.keyboard.press(enter);
      await expect.poll(() => inMode(page)).toBe(true);
      await page.keyboard.press("m");
      await expect.poll(() => inMode(page)).toBe(false);
      expect(await marksOff(page)).toBe(true);
      await page.keyboard.press("m");
      await expect.poll(() => marksOff(page)).toBe(false);
    });
  }

  test("in reading mode j leaves it and moves, so the walk never runs unseen", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const from = await stopSp(page, "Filler paragraph 20.");
    const to = await stopSp(page, "Filler paragraph 21.");
    await focusTo(page, from);
    await page.keyboard.press("r");
    await expect.poll(() => pillOf(page)).toBe(READING_SAYS);
    await page.keyboard.press("j");
    await expect.poll(() => readingMode(page)).toBe(false);
    await expect.poll(() => focused(page)).toBe(to);
    await expect.poll(() => inView(page, to)).toBe(true);
  });

  test("a viewer key is no review key: d in source view leaves it shown", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#source-view")).toBeVisible();
    const theme = () => page.evaluate(() => document.documentElement.getAttribute("data-theme"));
    const before = await theme();
    await page.keyboard.press("d");
    await expect.poll(theme).not.toBe(before);
    expect(await sourceShown(page)).toBe(true);
  });
});

// --- Esc: one layer per press, and every mode's exit ---------------------------------------------

test.describe("Esc in a mode", () => {
  test("Esc leaves source view in one press, focus kept, and a second Esc does nothing", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const sp = await stopSp(page, "Filler paragraph 20.");
    await focusTo(page, sp);
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator("#lane-frame")).toBeVisible();
    await expect.poll(() => focused(page)).toBe(sp);
    await expect.poll(() => inView(page, sp)).toBe(true);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(await sourceShown(page)).toBe(false);
    expect(await focused(page)).toBe(sp);
  });

  test("reading mode says how back, and Esc leaves it in one press with the focus kept", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await page.keyboard.press("r");
    await expect.poll(() => readingMode(page)).toBe(true);
    await expect.poll(() => pillOf(page)).toBe(READING_SAYS);
    await page.keyboard.press("Escape");
    await expect.poll(() => readingMode(page)).toBe(false);
    expect(await focused(page)).toBe(sp);
  });

  test("Esc drops one layer per press: the phrase menu, then source view", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("sent"), { state: fixtureState("sent") });
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.locator(".file-header birta-phrase").click();
    await expect.poll(() => menuRows(page)).not.toBeNull();
    await page.keyboard.press("Escape");
    await expect.poll(() => menuRows(page)).toBeNull();
    expect(await sourceShown(page)).toBe(true);
    await page.keyboard.press("Escape");
    await expect(page.locator("#lane-frame")).toBeVisible();
  });

  test("Esc drops one layer per press: the composer, then reading mode", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await page.keyboard.press("c");
    const area = page.locator('birta-flow[data-kind="composer"] textarea');
    await expect(area).toBeFocused();
    // Reading mode by its button, so the composer stays open over it
    await page.evaluate(() => document.getElementById("reading-toggle")!.click());
    await expect.poll(() => readingMode(page)).toBe(true);
    await page.keyboard.press("Escape");
    await expect(area).toHaveCount(0);
    expect(await readingMode(page)).toBe(true);
    await page.keyboard.press("Escape");
    await expect.poll(() => readingMode(page)).toBe(false);
    expect(await focused(page)).toBe(sp);
  });
});

// --- Page controls ---------------------------------------------------------------------------------

const CONTROLS = [
  "#theme-dropdown-btn",
  "#theme-toggle",
  "#reading-toggle",
  "#copy-md-btn",
  '#view-toggle .view-btn[data-view="preview"]',
  '#view-toggle .view-btn[data-view="raw"]',
];

test.describe("page controls", () => {
  test("Tab out of a doc with no links reaches the page controls, never a heading's link icon or the hidden reading-exit button", async ({
    page,
    birta,
    browserName,
  }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await page.keyboard.press(tabKey(browserName));
    await expect.poll(() => control(page)).not.toBeNull();
    const seen = new Set<string>();
    for (let i = 0; i < 30; i++) {
      seen.add(await activeName(page));
      await page.keyboard.press(tabKey(browserName));
    }
    const names = Array.from(seen);
    expect(names.filter((n) => n.includes("a.anchor") || n.includes("reading-exit-btn"))).toEqual([]);
    expect(names).toContain("#theme-dropdown-btn");
    // The doc's own focus stays where it was while Tab walks the controls
    expect(await focused(page)).toBe(sp);
  });

  test("Tab onto a page control says how back, and Esc returns to the doc's stop", async ({ page, birta, browserName }) => {
    await open(page, birta, longDoc());
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await page.keyboard.press(tabKey(browserName));
    await expect.poll(() => control(page)).not.toBeNull();
    await expect.poll(() => pillOf(page)).toBe(CONTROL_SAYS);
    await page.keyboard.press("Escape");
    await expect.poll(() => control(page)).toBeNull();
    expect(await focused(page)).toBe(sp);
    // Back on the doc, the keys act there
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(await stopSp(page, "Stacked on"));
  });

  for (const sel of CONTROLS) {
    test(`a review key on ${sel} returns focus to the doc's stop, then acts`, async ({ page, birta }) => {
      await open(page, birta, longDoc());
      const from = await stopSp(page, "Filler paragraph 20.");
      const to = await stopSp(page, "Filler paragraph 21.");
      await focusTo(page, from);
      await page.focus(sel);
      expect(await control(page)).not.toBeNull();
      await page.keyboard.press("j");
      await expect.poll(() => focused(page)).toBe(to);
      expect(await control(page)).toBeNull();
      await expect.poll(() => inView(page, to)).toBe(true);
    });
  }

  test("x, c and ? on the theme button act on the doc's stop", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await page.focus("#theme-dropdown-btn");
    await page.keyboard.press("x");
    await expect
      .poll(() => writes(mock.ops()))
      .toEqual([{ op: "comment", at: { k: "block", sp, scope: "block" }, verdict: "no" }]);
    expect(await control(page)).toBeNull();
    await page.focus("#theme-dropdown-btn");
    await page.keyboard.press("c");
    await expect(page.locator('birta-flow[data-kind="composer"] textarea')).toBeFocused();
    await page.keyboard.press("Escape");
    await page.focus("#theme-dropdown-btn");
    await page.keyboard.press("?");
    await expect
      .poll(() =>
        page.evaluate(() =>
          Array.from(document.querySelectorAll("birta-pop")).some((h) =>
            Array.from((h.shadowRoot ?? h).querySelectorAll("[popover]")).some((el) => {
              try {
                return el.matches(":popover-open") && (el.textContent ?? "").includes("source view");
              } catch {
                return false;
              }
            }),
          ),
        ),
      )
      .toBe(true);
    expect(await control(page)).toBeNull();
  });

  test("⏎ and Space still press the control, and the doc's focus stays", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await page.focus("#theme-dropdown-btn");
    await page.keyboard.press("Enter");
    await expect(page.locator(".theme-dropdown")).toHaveClass(/\bopen\b/);
    await page.focus("#reading-toggle");
    await page.keyboard.press(" ");
    await expect.poll(() => readingMode(page)).toBe(true);
    expect(await focused(page)).toBe(sp);
    expect(writes(mock.ops())).toEqual([]);
  });
});

// --- The phrase menu -------------------------------------------------------------------------------

test.describe("phrase menu", () => {
  test("opens on its first, safest row, with withdraw last, and ⏎ there withdraws nothing", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("sent"), { state: fixtureState("sent") });
    await page.locator(".file-header birta-phrase").click();
    await expect.poll(() => menuRows(page)).not.toBeNull();
    const menu = (await menuRows(page))!;
    expect(menu.rows[menu.rows.length - 1]).toBe("withdraw");
    expect(menu.current).toBe(0);
    expect(menu.rows[0]).not.toBe("withdraw");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops().filter((o) => o.op === "withdraw")).toEqual([]);
  });

  test("⏎ on the phrase reached by keyboard opens the menu on its first row", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("sent"), { state: fixtureState("sent") });
    await page.locator(".file-header birta-phrase .phrase").focus();
    await page.keyboard.press("Enter");
    await expect.poll(() => menuRows(page)).not.toBeNull();
    const menu = (await menuRows(page))!;
    expect(menu.current).toBe(0);
    expect(menu.rows[0]).not.toBe("withdraw");
  });
});

// --- Adversary: inputs the tests above let a broken build through ----------------------------------

/** Presses ⌥u as a Mac does: Option turns `u` into `¨` (or a dead key), and only the physical key is KeyU. */
function macAltU(page: Page, key = "¨"): Promise<void> {
  return page.evaluate((key) => {
    const t = (document.activeElement as HTMLElement | null) ?? document.body;
    t.dispatchEvent(
      new KeyboardEvent("keydown", { key, code: "KeyU", altKey: true, bubbles: true, cancelable: true, composed: true }),
    );
  }, key);
}

/** Clicks a viewer button without moving DOM focus or the selection, as a script click does. */
function clickQuiet(page: Page, sel: string): Promise<void> {
  return page.evaluate((sel) => (document.querySelector(sel) as HTMLElement).click(), sel);
}

const RAW_BTN = '#view-toggle .view-btn[data-view="raw"]';

test.describe("adversary", () => {
  test("⌥u on a Mac (e.key ¨ or Dead, code KeyU) shows the source, and back keeps the focus in view", async ({
    page,
    birta,
  }) => {
    await open(page, birta, longDoc());
    const target = await stopSp(page, "Filler paragraph 20.");
    await focusTo(page, target);
    for (const key of ["¨", "Dead"]) {
      await macAltU(page, key);
      await expect(page.locator("#source-view")).toBeVisible();
      await expect.poll(() => pillOf(page)).toBe(SOURCE_SAYS);
      await macAltU(page, key);
      await expect(page.locator("#lane-frame")).toBeVisible();
      await expect.poll(() => focused(page)).toBe(target);
      await expect.poll(() => inView(page, target)).toBe(true);
    }
  });

  test("a review key in reading mode entered from source view brings the doc back, not the source", async ({
    page,
    birta,
  }) => {
    await open(page, birta, longDoc());
    const from = await stopSp(page, "Filler paragraph 20.");
    const to = await stopSp(page, "Filler paragraph 21.");
    await focusTo(page, from);
    await clickQuiet(page, RAW_BTN);
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("r");
    await expect.poll(() => readingMode(page)).toBe(true);
    await page.keyboard.press("j");
    await expect.poll(() => readingMode(page)).toBe(false);
    await expect(page.locator("#lane-frame")).toBeVisible();
    expect(await sourceShown(page)).toBe(false);
    await expect.poll(() => focused(page)).toBe(to);
    await expect.poll(() => inView(page, to)).toBe(true);
  });

  test("Esc in reading mode entered from source view removes one layer, then the next", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await clickQuiet(page, RAW_BTN);
    await page.keyboard.press("r");
    await expect.poll(() => readingMode(page)).toBe(true);
    await page.keyboard.press("Escape");
    await expect.poll(() => readingMode(page)).toBe(false);
    // The viewer puts back the view reading mode was entered from
    await page.waitForTimeout(SETTLE_MS);
    expect(await sourceShown(page)).toBe(true);
    await page.keyboard.press("Escape");
    await expect(page.locator("#lane-frame")).toBeVisible();
    expect(await sourceShown(page)).toBe(false);
    expect(await focused(page)).toBe(sp);
  });

  test("Esc on a page control in source view returns to the doc's stop first, then leaves the view", async ({
    page,
    birta,
    browserName,
  }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    const onRaw = () => page.evaluate((sel) => document.activeElement === document.querySelector(sel), RAW_BTN);
    for (let i = 0; i < 40 && !(await onRaw()); i++) await page.keyboard.press(tabKey(browserName));
    expect(await onRaw()).toBe(true);
    // ⏎ presses the Source button
    await page.keyboard.press("Enter");
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect.poll(() => control(page)).toBeNull();
    await page.waitForTimeout(SETTLE_MS);
    expect(await sourceShown(page)).toBe(true);
    await page.keyboard.press("Escape");
    await expect(page.locator("#lane-frame")).toBeVisible();
    expect(await focused(page)).toBe(sp);
  });

  test("a selection hidden by source view is no Esc layer: one Esc leaves the view", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await page.evaluate((sp) => {
      const el = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      const r = document.createRange();
      r.selectNodeContents(el);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    }, sp);
    await expect.poll(() => focused(page)).toBe(sp);
    await clickQuiet(page, RAW_BTN);
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator("#lane-frame")).toBeVisible();
    expect(await sourceShown(page)).toBe(false);
  });

  test("a picked link hidden by source view is no Esc layer: one Esc leaves the view", async ({ page, birta }) => {
    const doc = "# Links\n\nSee [alpha](https://a.example) and [beta](https://b.example).\n\nPlain words.\n";
    await open(page, birta, doc);
    const sp = await stopSp(page, "See alpha");
    await focusTo(page, sp);
    // Review's own Tab picks the block's first link, in every engine
    await page.keyboard.press("Tab");
    await expect.poll(() => page.evaluate(() => !!document.querySelector("#content a[data-br-pick]"))).toBe(true);
    await clickQuiet(page, RAW_BTN);
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator("#lane-frame")).toBeVisible();
    expect(await sourceShown(page)).toBe(false);
  });

  for (const where of ["a file", "a folder"]) {
    test(`in ${where}, --bind toggle_raw=c keeps c the note's and gives the source view ⌥c`, async ({ page, birta }) => {
      const args = where === "a folder" ? [".", "--bind", "toggle_raw=c"] : ["pr.md", "--bind", "toggle_raw=c"];
      await open(page, birta, STOPS_DOC, { args });
      const kb = await page.evaluate(() => {
        const k = (window as unknown as { KEYBINDINGS: Record<string, string> }).KEYBINDINGS;
        return { toggle_raw: k.toggle_raw, note: k.note };
      });
      expect(kb).toEqual({ toggle_raw: "Alt+c", note: "c" });
      await focusTo(page, await stopSp(page, "Detail words."));
      await page.keyboard.press("c");
      await expect(page.locator('birta-flow[data-kind="composer"] textarea')).toBeFocused();
      expect(await sourceShown(page)).toBe(false);
      await page.keyboard.press("Escape");
      await page.keyboard.press("Alt+KeyC");
      await expect(page.locator("#source-view")).toBeVisible();
    });
  }

  test("a raw-view key that is a review key in another case still moves to ⌥", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC, { args: ["pr.md", "--bind", "toggle_raw=U"] });
    const raw = await page.evaluate(
      () => (window as unknown as { KEYBINDINGS: Record<string, string> }).KEYBINDINGS.toggle_raw,
    );
    expect(raw).toMatch(/^Alt\+u$/i);
    // The viewer reads a letter under Alt by its physical key only when the binding spells it lower case
    await page.keyboard.press("Alt+KeyU");
    await expect(page.locator("#source-view")).toBeVisible();
  });

  test("the reading-exit button leaves the Tab order whenever it is hidden", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const state = () =>
      page.evaluate(() => {
        const bar = document.getElementById("reading-exit-bar")!;
        const btn = document.getElementById("reading-exit-btn") as HTMLButtonElement;
        const hidden = getComputedStyle(bar).opacity === "0" || bar.getClientRects().length === 0;
        return { hidden, tabIndex: btn.tabIndex };
      });
    await page.mouse.move(640, 600);
    expect(await state()).toEqual({ hidden: true, tabIndex: -1 });
    await clickQuiet(page, "#reading-toggle");
    await expect.poll(() => readingMode(page)).toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    expect(await state()).toEqual({ hidden: true, tabIndex: -1 });
    await clickQuiet(page, "#reading-toggle");
    await expect.poll(() => readingMode(page)).toBe(false);
    expect(await state()).toEqual({ hidden: true, tabIndex: -1 });
  });

  // Disputable: 6.12 makes the phrase a page control only by where it sits
  test("j on the header's phrase returns focus to the doc's stop, then moves", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("sent"), { state: fixtureState("sent") });
    const all = await stops(page);
    await focusTo(page, all[1].sp);
    await page.locator(".file-header birta-phrase .phrase").focus();
    expect(await control(page)).toBe("birta-phrase");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(all[2].sp);
    expect(await control(page)).toBeNull();
    expect(await menuRows(page)).toBeNull();
  });

  test("the phrase menu opens on its first row every time, not on the row last left", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("sent"), { state: fixtureState("sent") });
    const phrase = page.locator(".file-header birta-phrase");
    await phrase.click();
    await expect.poll(() => menuRows(page)).not.toBeNull();
    const n = (await menuRows(page))!.rows.length;
    for (let i = 0; i < n; i++) await page.keyboard.press("j");
    expect((await menuRows(page))!.current).toBeGreaterThan(0);
    await page.keyboard.press("Escape");
    await expect.poll(() => menuRows(page)).toBeNull();
    await phrase.click();
    await expect.poll(() => menuRows(page)).not.toBeNull();
    const menu = (await menuRows(page))!;
    expect(menu.current).toBe(0);
    expect(menu.rows[0]).not.toBe("withdraw");
  });
});

test.describe("repair", () => {
  // A mouse-focused button shows no ring, so it is no Esc layer; WebKit never focuses a clicked button
  test("after a real click on the reading button, one Esc leaves reading mode", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await page.click("#reading-toggle");
    await expect.poll(() => readingMode(page)).toBe(true);
    await page.keyboard.press("Escape");
    await expect.poll(() => readingMode(page)).toBe(false);
    expect(await focused(page)).toBe(sp);
  });

  test("after a real click on the Source button, one Esc leaves the source view", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    const sp = await stopSp(page, "Detail words.");
    await focusTo(page, sp);
    await page.click(RAW_BTN);
    await expect(page.locator("#source-view")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator("#lane-frame")).toBeVisible();
    expect(await focused(page)).toBe(sp);
  });

  test("a review key on the open theme menu's button closes the menu, then acts", async ({ page, birta }) => {
    await open(page, birta, longDoc());
    const from = await stopSp(page, "Filler paragraph 20.");
    const to = await stopSp(page, "Filler paragraph 21.");
    await focusTo(page, from);
    await page.click("#theme-dropdown-btn");
    const dropdown = page.locator(".theme-dropdown");
    await expect(dropdown).toHaveClass(/\bopen\b/);
    await page.focus("#theme-dropdown-btn");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(to);
    await expect(dropdown).not.toHaveClass(/\bopen\b/);
  });
});
