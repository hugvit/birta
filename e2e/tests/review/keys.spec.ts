import { Page } from "@playwright/test";
import {
  test,
  expect,
  GATE_ON,
  Rgb,
  STOPS_DOC,
  State,
  booted,
  dispatchKey,
  draftFiles,
  fixtureDoc,
  fixtureState,
  focused,
  frameBox,
  ink,
  mockReview,
  pixelsOf,
  reviewLog,
  shot,
  stops,
  watchSent,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const WHITE: Rgb = [255, 255, 255];
/** Every review key at its default binding, as Playwright names them. */
const REVIEW_KEYS = ["j", "k", "c", "q", "y", "x", "l", ".", "n", "p", "z", "m", "Enter", "Shift+J", "Shift+K"];
/** How long a key that should do nothing is given to do something anyway. */
const SETTLE_MS = 400;
/** A paragraph the unseen fixture draws a bar and a glyph beside. */
const UNSEEN_BAR = "5:1-6:68";

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** Serves `doc` as pr.md with review on and the socket mocked, and waits for review to boot. */
async function open(page: Page, birta: Start, doc: string, opts: { state?: State; args?: string[] } = {}) {
  const served = await birta({ "pr.md": doc }, GATE_ON, opts.args);
  await page.setViewportSize(WIDE);
  const mock = await mockReview(page, { state: opts.state });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, mock };
}

/** Whether the unseen fixture's lane is painted: its bar column carries ink. */
async function laneShown(page: Page): Promise<boolean> {
  const frame = await frameBox(page);
  const block = await page.locator(`#content [data-sourcepos="${UNSEEN_BAR}"]`).first().boundingBox();
  const box = { left: frame.left + 12, top: block!.y + 2, right: frame.left + 28, bottom: block!.y + block!.height - 2 };
  const s = await shot(page, { left: box.left, top: box.top, width: box.right - box.left, height: box.bottom - box.top });
  return ink(pixelsOf(s, { ...box, width: 16, height: box.bottom - box.top }), WHITE) !== null;
}

async function readingMode(page: Page): Promise<boolean> {
  return page.evaluate(() => document.body.classList.contains("reading-mode"));
}

async function selectionText(page: Page): Promise<string> {
  return page.evaluate(() => getSelection()?.toString() ?? "");
}

/** Adds a text input and a contenteditable outside the doc, where a reader might type. */
async function addFields(page: Page) {
  await page.evaluate(() => {
    const input = document.createElement("input");
    input.id = "probe-input";
    const area = document.createElement("textarea");
    area.id = "probe-area";
    const edit = document.createElement("div");
    edit.id = "probe-edit";
    edit.contentEditable = "true";
    edit.style.cssText = "min-height:20px;border:1px solid";
    document.querySelector(".container")!.prepend(input, area, edit);
  });
}

test.describe("guard", () => {
  test("no review key acts from a text input or a contenteditable", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => laneShown(page)).toBe(true);
    await addFields(page);
    for (const field of ["#probe-input", "#probe-area", "#probe-edit"]) {
      await page.click(field);
      for (const key of REVIEW_KEYS) await page.keyboard.press(key);
      await page.waitForTimeout(SETTLE_MS);
      // The keys reached the field, so nothing swallowed them
      const typed = await page.evaluate(
        (sel) => {
          const el = document.querySelector(sel) as HTMLInputElement | HTMLElement;
          return "value" in el ? (el as HTMLInputElement).value : el.textContent;
        },
        field,
      );
      expect(typed, field).toContain("jkcqyxl.npzm");
    }
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBeNull();
    expect(await laneShown(page)).toBe(true);
    await expect(page.locator("textarea")).toHaveCount(1);
    // Out of the field, the same key acts
    await page.evaluate(() => (document.activeElement as HTMLElement).blur());
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).not.toBeNull();
  });

  test("no review key acts during IME composition", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => laneShown(page)).toBe(true);
    for (const key of ["j", "x", "y", "m", "c", "z"]) {
      await dispatchKey(page, { key, code: `Key${key.toUpperCase()}`, isComposing: true }, "body");
      await dispatchKey(page, { key, keyCode: 229 }, "body");
    }
    await dispatchKey(page, { key: "Process", keyCode: 229 }, "body");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBeNull();
    expect(await laneShown(page)).toBe(true);
    await expect(page.locator("textarea")).toHaveCount(0);
    // The same dispatch outside composition does act, so the events do reach review
    await dispatchKey(page, { key: "j", code: "KeyJ" }, "body");
    await expect.poll(() => focused(page)).not.toBeNull();
  });

  test("no review key acts with Option, Ctrl or Cmd held, and the browser keeps its shortcuts", async ({
    page,
    birta,
  }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => laneShown(page)).toBe(true);
    await page.evaluate(() => {
      (window as any).__prevented = [];
      window.addEventListener("keydown", (e) => {
        if (["Alt", "Control", "Meta", "Shift"].includes(e.key)) return;
        if (e.altKey || e.ctrlKey || e.metaKey) (window as any).__prevented.push(`${e.key}:${e.defaultPrevented}`);
      });
    });
    for (const mod of ["Alt", "Control", "Meta"]) {
      for (const key of ["j", "k", "c", "q", "y", "x", "l", "z", "m"]) await page.keyboard.press(`${mod}+${key}`);
    }
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBeNull();
    expect(await laneShown(page)).toBe(true);
    await expect(page.locator("textarea")).toHaveCount(0);
    const prevented: string[] = await page.evaluate(() => (window as any).__prevented);
    expect(prevented).toHaveLength(27);
    expect(prevented.filter((p) => p.endsWith(":true"))).toEqual([]);
    // Without the modifier, the same key acts
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).not.toBeNull();
  });

  test("a held verdict key sends one verdict, but a held j keeps moving", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const all = await stops(page);
    await dispatchKey(page, { key: "x", code: "KeyX" }, "body");
    for (let i = 0; i < 3; i++) await dispatchKey(page, { key: "x", code: "KeyX", repeat: true }, "body");
    await expect.poll(() => mock.ops().length).toBe(1);
    await dispatchKey(page, { key: "j", code: "KeyJ" }, "body");
    for (let i = 0; i < 2; i++) await dispatchKey(page, { key: "j", code: "KeyJ", repeat: true }, "body");
    await expect.poll(() => focused(page)).toBe(all[2].sp);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });

  test("Caps Lock J acts as j, and Shift+J does not", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    const all = await stops(page);
    await dispatchKey(page, { key: "J", code: "KeyJ" }, "body");
    await expect.poll(() => focused(page)).toBe(all[0].sp);
    await page.keyboard.press("Shift+J");
    await page.waitForTimeout(SETTLE_MS);
    expect(await focused(page)).toBe(all[0].sp);
    // The typed letter counts, not the key's place: a Dvorak j sits where QWERTY has c
    await dispatchKey(page, { key: "j", code: "KeyC" }, "body");
    await expect.poll(() => focused(page)).toBe(all[1].sp);
    // A Caps Lock verdict key is a verdict too
    await dispatchKey(page, { key: "X", code: "KeyX" }, "body");
    await expect.poll(() => mock.ops().map((o) => o.verdict)).toEqual(["no"]);
  });

  test("a key from a review host is ignored, and a doc-made birta-* element is not a host", async ({
    page,
    birta,
  }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await dispatchKey(page, { key: "x", code: "KeyX" }, "#lane-frame > birta-over");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.evaluate(() => {
      const fake = document.createElement("birta-flow");
      fake.id = "fake-host";
      document.getElementById("content")!.appendChild(fake);
    });
    await dispatchKey(page, { key: "x", code: "KeyX" }, "#fake-host");
    await expect.poll(() => mock.ops().length).toBe(1);
  });
});

test.describe("guard, edges", () => {
  test("Shift with a verdict, note or undo key does nothing", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    for (const combo of ["Shift+X", "Shift+Y", "Shift+L", "Shift+C", "Shift+Q", "Shift+Z"]) await page.keyboard.press(combo);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await expect(page.locator("textarea")).toHaveCount(0);
  });

  test("Enter on a focused summary or link keeps its native action", async ({ page, birta }) => {
    const doc = `${STOPS_DOC}\n<details><summary>More</summary>\n\nInside details.\n\n</details>\n\n[jump](#stacked-on)\n`;
    await open(page, birta, doc);
    await page.focus("#content summary");
    await page.keyboard.press("Enter");
    await expect(page.locator("#content details")).toHaveAttribute("open", "");
    await page.focus('#content a[href="#stacked-on"]');
    await page.keyboard.press("Enter");
    await expect.poll(() => page.evaluate(() => location.hash)).toBe("#stacked-on");
  });

  test("with review off, no review key sends, writes or throws, and r still works", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, {});
    const sent = watchSent(page);
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await expect(page.locator("html[data-review]")).toHaveCount(0);
    for (const key of REVIEW_KEYS) await page.keyboard.press(key);
    await page.waitForTimeout(SETTLE_MS);
    expect(sent.filter((f) => ["op", "compose", "resolve"].includes(f.type))).toEqual([]);
    expect(reviewLog(served.root)).toBeNull();
    expect(errors).toEqual([]);
    await page.keyboard.press("r");
    expect(await readingMode(page)).toBe(true);
  });

  test("with the folder not writable, review keys do nothing", async ({ page, birta }) => {
    const state = { ...fixtureState("unseen"), log_writable: false };
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state });
    await expect.poll(() => laneShown(page)).toBe(true);
    for (const key of ["x", "y", "l", ".", "c", "q", "z", "j", "m"]) await page.keyboard.press(key);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBeNull();
    expect(await laneShown(page)).toBe(true);
    await expect(page.locator("textarea")).toHaveCount(0);
  });

  test("in the raw view review keys do nothing, and the view toggle still works", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => laneShown(page)).toBe(true);
    await page.keyboard.press("u");
    await expect(page.locator("#source-view")).toBeVisible();
    for (const key of ["j", "x", "y", "m", "z"]) await page.keyboard.press(key);
    await page.waitForTimeout(SETTLE_MS);
    await page.keyboard.press("u");
    await expect(page.locator("#lane-frame")).toBeVisible();
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBeNull();
    await expect.poll(() => laneShown(page)).toBe(true);
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).not.toBeNull();
  });

  test("review keys do nothing while the theme menu button has focus", async ({ page, birta }) => {
    const { mock } = await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => laneShown(page)).toBe(true);
    await page.focus("#theme-dropdown-btn");
    for (const key of ["j", "k", "x", "y", "l", ".", "z", "m"]) await page.keyboard.press(key);
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    expect(await focused(page)).toBeNull();
    expect(await laneShown(page)).toBe(true);
    // Once the control lets go, the same key acts
    await page.evaluate(() => (document.activeElement as HTMLElement).blur());
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).not.toBeNull();
  });

  test("a held z takes back nothing more", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC);
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.waitForTimeout(SETTLE_MS);
    await dispatchKey(page, { key: "z", code: "KeyZ", repeat: true }, "body");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
    await dispatchKey(page, { key: "z", code: "KeyZ" }, "body");
    await expect.poll(() => mock.ops().map((o) => o.op)).toEqual(["comment", "retract"]);
  });
});

test.describe("existing keys", () => {
  test("r, d, t and u still work with review on", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    await page.keyboard.press("r");
    expect(await readingMode(page)).toBe(true);
    await page.keyboard.press("r");
    expect(await readingMode(page)).toBe(false);

    const theme = () => page.evaluate(() => document.documentElement.getAttribute("data-theme"));
    const before = await theme();
    await page.keyboard.press("d");
    await expect.poll(theme).not.toBe(before);

    await page.keyboard.press("t");
    await expect(page.locator(".theme-dropdown")).toHaveClass(/\bopen\b/);
    await page.keyboard.press("t");
    await expect(page.locator(".theme-dropdown")).not.toHaveClass(/\bopen\b/);

    await page.keyboard.press("u");
    await expect(page.locator("#source-view")).toBeVisible();
    await expect(page.locator("#lane-frame")).toBeHidden();
    await page.keyboard.press("u");
    await expect(page.locator("#lane-frame")).toBeVisible();
  });

  for (const gate of ["on", "off"]) {
    test(`with review ${gate}, Caps Lock R toggles reading mode, and Option+R or r in a contenteditable does not`, async ({
      page,
      birta,
    }) => {
      const served = await birta({ "pr.md": STOPS_DOC }, gate === "on" ? GATE_ON : {});
      await page.setViewportSize(WIDE);
      await page.goto(`${served.url}/view/pr.md`);
      if (gate === "on") await booted(page);
      await dispatchKey(page, { key: "R", code: "KeyR" }, "body");
      expect(await readingMode(page)).toBe(true);
      await dispatchKey(page, { key: "R", code: "KeyR" }, "body");
      expect(await readingMode(page)).toBe(false);
      await page.keyboard.press("Alt+r");
      expect(await readingMode(page)).toBe(false);
      await addFields(page);
      await page.click("#probe-edit");
      await page.keyboard.press("r");
      expect(await readingMode(page)).toBe(false);
      await expect(page.locator("#probe-edit")).toHaveText("r");
    });
  }

  test("reading mode hides the lane, and leaving it brings the lane back", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => laneShown(page)).toBe(true);
    await page.keyboard.press("r");
    await expect.poll(() => laneShown(page)).toBe(false);
    await page.keyboard.press("r");
    await expect.poll(() => laneShown(page)).toBe(true);
  });

  test("m with a modifier or Shift leaves the marks, and Caps Lock M hides them", async ({ page, birta }) => {
    await open(page, birta, fixtureDoc("unseen"), { state: fixtureState("unseen") });
    await expect.poll(() => laneShown(page)).toBe(true);
    for (const combo of ["Meta+m", "Control+m", "Alt+m", "Shift+M"]) await page.keyboard.press(combo);
    await page.waitForTimeout(SETTLE_MS);
    expect(await laneShown(page)).toBe(true);
    await dispatchKey(page, { key: "M", code: "KeyM" }, "body");
    await expect.poll(() => laneShown(page)).toBe(false);
    await page.keyboard.press("m");
    await expect.poll(() => laneShown(page)).toBe(true);
  });
});

test.describe("Esc ladder", () => {
  test("Esc drops one layer per press: theme menu, selection, focus, then reading mode", async ({
    page,
    birta,
  }) => {
    await open(page, birta, STOPS_DOC);
    const all = await stops(page);
    await page.keyboard.press("r");
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await expect.poll(() => focused(page)).toBe(all[1].sp);
    // Select a word inside the focused heading, so no click moves focus
    await page.evaluate((sp) => {
      const h = document.querySelector(`#content [data-sourcepos="${sp}"]`)!;
      const text = Array.from(h.childNodes).find((n) => n.nodeType === 3 && n.textContent!.trim())!;
      const r = document.createRange();
      r.setStart(text, 0);
      r.setEnd(text, 3);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
    }, all[1].sp);
    await page.keyboard.press("t");
    const dropdown = page.locator(".theme-dropdown");
    await expect(dropdown).toHaveClass(/\bopen\b/);

    const layers = async () => ({
      menu: /\bopen\b/.test((await dropdown.getAttribute("class")) ?? ""),
      selection: (await selectionText(page)) !== "",
      focus: await focused(page),
      reading: await readingMode(page),
    });
    expect(await layers()).toEqual({ menu: true, selection: true, focus: all[1].sp, reading: true });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ menu: false, selection: true, focus: all[1].sp, reading: true });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ menu: false, selection: false, focus: all[1].sp, reading: true });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ menu: false, selection: false, focus: null, reading: true });
    await page.keyboard.press("Escape");
    await expect.poll(layers).toEqual({ menu: false, selection: false, focus: null, reading: false });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(SETTLE_MS);
    expect(await layers()).toEqual({ menu: false, selection: false, focus: null, reading: false });
  });

  test("with only reading mode on, Esc leaves it", async ({ page, birta }) => {
    await open(page, birta, STOPS_DOC);
    await page.keyboard.press("r");
    expect(await readingMode(page)).toBe(true);
    await page.keyboard.press("Escape");
    await expect.poll(() => readingMode(page)).toBe(false);
  });
});

test.describe("files and activation", () => {
  test("moving focus and hiding marks write nothing, and the first verdict creates the file and z undoes it", async ({
    page,
    birta,
  }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    const sent = watchSent(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    for (const key of ["j", "j", "k", "k", "m", "m"]) await page.keyboard.press(key);
    await expect.poll(() => focused(page)).toBeNull();
    await page.waitForTimeout(SETTLE_MS);
    expect(sent.filter((f) => f.type === "op" || f.type === "compose")).toEqual([]);
    expect(reviewLog(served.root)).toBeNull();
    expect(draftFiles(served.root)).toEqual([]);

    // A stray x on the doc is a kept verdict, and says how to take it back
    await page.keyboard.press("x");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    const created = reviewLog(served.root)!;
    expect(created.map((l) => l.op ?? l.birta)).toEqual(["review/1", "snap", "owner", "comment"]);
    expect(created[3]).toMatchObject({ op: "comment", at: "doc", verdict: "no" });
    expect(created[3].by).toStrictEqual({ type: "human", via: "browser" });
    await expect(page.getByText("review file created · z to undo")).toBeVisible();

    await page.keyboard.press("z");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(5);
    expect(reviewLog(served.root)![4]).toMatchObject({ op: "retract", to: 4 });
    expect(draftFiles(served.root)).toEqual([]);
  });

  test("a verdict on a doc that already has a review file does not claim to create it", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("x");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    await page.reload();
    await booted(page);
    await page.keyboard.press("y");
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(5);
    await page.waitForTimeout(SETTLE_MS);
    await expect(page.getByText("review file created")).toHaveCount(0);
  });

  // Needs the composer, which step 5.4 builds
  test.fixme("an empty composer opened and closed with Esc writes nothing", async ({ page, birta }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    const sent = watchSent(page);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    for (const key of ["c", "q"]) {
      await page.keyboard.press(key);
      await expect(page.locator("textarea")).toBeFocused();
      await page.keyboard.press("Escape");
      await expect(page.locator("textarea")).toHaveCount(0);
    }
    await page.waitForTimeout(SETTLE_MS);
    expect(sent.filter((f) => f.type === "op" || (f.type === "compose" && f.text))).toEqual([]);
    expect(reviewLog(served.root)).toBeNull();
    expect(draftFiles(served.root)).toEqual([]);
  });

  test("a review key rebound with --bind acts on its new key only", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC, { args: ["pr.md", "--bind", "yes=g"] });
    await page.keyboard.press("y");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toEqual([]);
    await page.keyboard.press("g");
    await expect.poll(() => mock.ops().map((o) => [o.op, o.verdict])).toEqual([["comment", "yes"]]);
  });

  test("a --bind onto a taken key wins it, and the action it took from is unbound", async ({ page, birta }) => {
    const { mock } = await open(page, birta, STOPS_DOC, { args: ["pr.md", "--bind", "yes=j"] });
    await page.keyboard.press("j");
    await expect.poll(() => mock.ops().map((o) => [o.op, o.verdict])).toEqual([["comment", "yes"]]);
    expect(await focused(page)).toBeNull();
    await page.keyboard.press("y");
    await page.waitForTimeout(SETTLE_MS);
    expect(mock.ops()).toHaveLength(1);
  });

  test("a verdict pressed just before a reload is kept once, and z takes back nothing from an earlier page", async ({
    page,
    birta,
  }) => {
    const served = await birta({ "pr.md": STOPS_DOC }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.keyboard.press("z");
    await page.keyboard.press("x");
    await page.reload();
    await booted(page);
    await expect.poll(() => reviewLog(served.root)?.length ?? 0).toBe(4);
    await page.keyboard.press("z");
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)).toHaveLength(4);
  });
});
