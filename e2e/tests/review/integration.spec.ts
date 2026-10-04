import { Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  GATE_ON,
  Received,
  State,
  agent,
  booted,
  focused,
  highlighted,
  hhmm,
  linesNamed,
  marks,
  misaligned,
  phraseOf,
  record,
  revOf,
  reviewLog,
  spAt,
  stripMessage,
  turnOf,
} from "./harness";

// The real server end to end: no socket mock here; one test passes the socket through to slip in a frame.

// The phrase prints local times; UTC lets the spec name them from the epoch alone
test.use({ timezoneId: "UTC" });

const WIDE = { width: 1280, height: 900 };
const SETTLE_MS = 400;

type Start = (
  files: Record<string, string>,
  env?: Record<string, string>,
  args?: string[],
) => Promise<{ url: string; root: string; write(rel: string, text: string): void }>;

/** Bold text, so a quote's source and rendered text differ, and a code block. */
const DOC = [
  "# Upload retries",
  "",
  "## Why",
  "",
  "When the upload **hits** a 503 we just fail.",
  "",
  "```js",
  "retry(upload, 3);",
  "```",
  "",
  "## Stacked on",
  "",
  "Nothing yet.",
  "",
].join("\n");
const PARA = "5:1-5:44";
const CODE = "7:1-9:3";

async function openReal(page: Page, birta: Start, env: Record<string, string> = {}, args?: string[]) {
  const served = await birta({ "pr.md": DOC }, { ...GATE_ON, ...env }, args);
  const frames = record(page);
  await page.setViewportSize(WIDE);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  return { served, frames };
}

/** Selects the rendered `text` (its last match with `last`) in block `sp`; returns the range's top. */
async function selectText(page: Page, sp: string, text: string, last = false): Promise<number> {
  return page.evaluate(
    ({ sp, text, last }) => {
      const all = document.querySelectorAll(`#content [data-sourcepos="${sp}"]`);
      const el = all[all.length - 1]!;
      const nodes: Text[] = [];
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) nodes.push(n as Text);
      const whole = nodes.map((n) => n.data).join("");
      const at = last ? whole.lastIndexOf(text) : whole.indexOf(text);
      if (at < 0) throw new Error(`${sp} has no ${JSON.stringify(text)}`);
      const point = (i: number, end: boolean): [Text, number] => {
        let off = 0;
        for (const n of nodes) {
          if (i < off + n.data.length || (end && i === off + n.data.length)) return [n, i - off];
          off += n.data.length;
        }
        const last = nodes[nodes.length - 1];
        return [last, last.data.length];
      };
      const r = document.createRange();
      r.setStart(...point(at, false));
      r.setEnd(...point(at + text.length, true));
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
      return r.getBoundingClientRect().top;
    },
    { sp, text, last },
  );
}

const composer = (page: Page) => page.locator('birta-flow[data-kind="composer"] textarea');

async function keep(page: Page, body: string) {
  await page.keyboard.press("c");
  await expect(composer(page)).toBeFocused();
  await page.keyboard.type(body);
  await page.keyboard.press("Enter");
  await expect(composer(page)).toHaveCount(0);
}

// --- hello and the state ------------------------------------------------------------------

test.describe("hello", () => {
  test("a fresh socket says hello and gets content, then review_state, then dir_state, for its page", async ({
    page,
    birta,
  }) => {
    const { frames } = await openReal(page, birta);
    await expect.poll(() => frames.got.filter((f) => f.socket === 0 && f.msg.type === "dir_state").length).toBeGreaterThan(0);
    const hello = frames.sent.find((f) => f.socket === 0 && f.msg.type === "hello");
    expect(hello?.msg).toMatchObject({ type: "hello", path: "pr.md" });
    expect(typeof hello?.msg.tab).toBe("string");
    const mine = frames.got.filter((f) => f.socket === 0 && (f.msg.path === "pr.md" || f.msg.type === "dir_state"));
    const at = (type: string) => mine.findIndex((f) => f.msg.type === type);
    expect(at("content")).toBeGreaterThanOrEqual(0);
    expect(at("content")).toBeLessThan(at("review_state"));
    expect(at("review_state")).toBeLessThan(at("dir_state"));
    const content = mine[at("content")].msg;
    expect(content.rev).toBe(revOf(DOC));
    expect(mine[at("review_state")].msg.rev).toBe(content.rev);
  });

  test("a kept note comes back in the server's state, and again after a reload", async ({ page, birta }) => {
    const { served, frames } = await openReal(page, birta);
    await keep(page, "way too verbose");
    await expect.poll(() => linesNamed(served.root, "comment").map((l) => l.body)).toEqual(["way too verbose"]);
    const n = linesNamed(served.root, "comment")[0].n;
    const has = (f: Received) =>
      f.msg.type === "review_state" && (f.msg.state?.items ?? []).some((i: State) => i.body === "way too verbose");
    await expect.poll(() => frames.got.filter((f) => f.socket === 0 && has(f)).length).toBeGreaterThan(0);
    const state = frames.got.find((f) => f.socket === 0 && has(f))!.msg.state;
    expect(state.rev).toBe(revOf(DOC));
    expect(state.items.find((i: State) => i.body === "way too verbose")).toMatchObject({ n, status: "draft" });
    // The phrase never counts your notes; the button does
    await expect.poll(() => phraseOf(page)).toBe("your turn");
    await expect.poll(() => turnOf(page)).toMatch(/^Hand back · 1( |$)/);

    await page.reload();
    await booted(page);
    await expect.poll(() => frames.got.filter((f) => f.socket === 1 && has(f)).length).toBeGreaterThan(0);
    await expect.poll(() => phraseOf(page)).toBe("your turn");
    await expect.poll(() => turnOf(page)).toMatch(/^Hand back · 1( |$)/);
    await expect(page.locator("birta-flow").filter({ hasText: "way too verbose" })).toBeVisible();
  });
});

// --- content before review_state ----------------------------------------------------------

/** Keeps a note so every save pushes a state too, then saves the doc six times, 40 ms apart. */
async function saveAgainAndAgain(page: Page, birta: Start) {
  const { served, frames } = await openReal(page, birta);
  await keep(page, "first");
  await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
  let text = DOC;
  for (let i = 1; i <= 6; i++) {
    text = text + `\nSaved paragraph ${i}.\n`;
    served.write("pr.md", text);
    await page.waitForTimeout(40);
  }
  const last = revOf(text);
  await expect
    .poll(() => frames.got.some((f) => f.socket === 0 && f.msg.type === "review_state" && f.msg.rev === last), {
      timeout: 10_000,
    })
    .toBe(true);
  return { served, frames, last };
}

/** A frame that puts a rendered doc on the page: `content`, or a theme update carrying one. */
const carriesDoc = (msg: State) => msg.type === "content" || (msg.type === "theme_update" && msg.html != null);

test.describe("content before review_state", () => {
  test("while the doc is saved again and again, every state reaches the socket after the content of its rev", async ({
    page,
    birta,
  }) => {
    const { frames } = await saveAgainAndAgain(page, birta);
    const mine = frames.got.filter((f) => f.socket === 0 && f.msg.path === "pr.md");
    const states = mine.filter((f) => f.msg.type === "review_state");
    expect(states.length).toBeGreaterThan(1);
    for (const s of states) {
      const i = mine.indexOf(s);
      const content = mine.slice(0, i).some((f) => carriesDoc(f.msg) && f.msg.rev === s.msg.rev);
      expect(content, `state for ${s.msg.rev} came before its content`).toBe(true);
    }
  });

  test("while the doc is saved again and again, the page ends on the last save with its state drawn in place", async ({
    page,
    birta,
  }) => {
    // Every frame, a drawn note must come from the state of the rev the page shows
    await page.addInitScript(() => {
      const w = window as unknown as {
        __mismatch: string[];
        BR?: { currentState(): { rev: string } | null };
        birtaBus?: { rev(): string };
      };
      w.__mismatch = [];
      const check = () => {
        const drawn = document.querySelectorAll("#content birta-flow").length > 0;
        const s = w.BR?.currentState?.();
        const rev = w.birtaBus?.rev();
        if (drawn && (!s || s.rev !== rev)) w.__mismatch.push(`${s?.rev} on ${rev}`);
      };
      // On every change to the doc, in the task that made it, and every frame besides
      new MutationObserver(check).observe(document, { childList: true, subtree: true });
      const tick = () => {
        check();
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    await saveAgainAndAgain(page, birta);
    await expect.poll(() => page.evaluate(() => document.getElementById("content")!.textContent)).toContain("Saved paragraph 6.");
    // Its own saves are outside writes; the phrase counts neither them nor your note, the button counts the note
    await expect.poll(() => phraseOf(page)).toBe("your turn");
    await expect.poll(() => turnOf(page)).toMatch(/^Hand back · 1( |$)/);
    await expect(page.locator("birta-flow").filter({ hasText: "first" })).toBeVisible();
    await expect.poll(() => misaligned(page)).toEqual([]);
    expect(await page.evaluate(() => (window as unknown as { __mismatch: string[] }).__mismatch)).toEqual([]);
  });
});

// --- the hold -----------------------------------------------------------------------------

test.describe("hold", () => {
  test("with a 300 ms hold the submit says so, and the phrase flips on its own once the hold ends", async ({
    page,
    birta,
  }) => {
    const { served, frames } = await openReal(page, birta, { BIRTA_REVIEW_HOLD_MS: "300" });
    await keep(page, "say why");
    await expect.poll(() => turnOf(page)).toMatch(/^Hand back · 1( |$)/);
    expect(await phraseOf(page)).toBe("your turn");
    // Times every change of the phrase from inside the page
    await page.evaluate(() => {
      const w = window as unknown as { __phrases: [number, string][] };
      w.__phrases = [];
      const read = () => {
        const h = document.querySelector(".file-header birta-phrase");
        if (!h) return "";
        return Array.from((h.shadowRoot ?? h).childNodes)
          .filter((c) => c.nodeName !== "STYLE")
          .map((c) => c.textContent ?? "")
          .join("")
          .replace(/\s+/g, " ")
          .trim();
      };
      let last = read();
      const tick = () => {
        const now = read();
        if (now !== last) w.__phrases.push([Date.now(), now]);
        last = now;
      };
      setInterval(tick, 5);
    });
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => linesNamed(served.root, "submit").length).toBe(1);
    const submit = linesNamed(served.root, "submit")[0];
    expect(submit.hold_ms).toBe(300);
    expect(submit.not_before - submit.ts).toBe(300);
    const sent = `sent ${hhmm(submit.not_before)} · not picked up yet`;
    await expect.poll(() => phraseOf(page), { timeout: 3_000 }).toBe(sent);
    const changes = await page.evaluate(() => (window as unknown as { __phrases: [number, string][] }).__phrases);
    const flip = changes.find(([, p]) => p.startsWith("sent "));
    expect(flip).toBeDefined();
    expect(flip![0]).toBeGreaterThanOrEqual(submit.not_before - 20);
    expect(flip![0]).toBeLessThan(submit.not_before + 500);
    expect(changes.filter(([t]) => t < submit.not_before - 20).every(([, p]) => !p.startsWith("sent "))).toBe(true);
    // Nothing the page sent after the hand-back made it flip
    const after = frames.sent.slice(frames.sent.findIndex((f) => f.msg.op === "hand_back") + 1);
    expect(after.filter((f) => f.msg.type !== "compose").map((f) => f.msg.type)).toEqual([]);
  });

  // K3
  for (const shift of [3_600_000, -3_600_000]) {
    test(`with the page's clock an hour ${shift > 0 ? "ahead" : "behind"}, the phrase flips at the server's not_before`, async ({
      page,
      birta,
    }) => {
      await page.addInitScript((shift) => {
        const Real = Date;
        const now = () => Real.now() + shift;
        class Shifted extends Real {
          constructor(...a: unknown[]) {
            super(...((a.length ? a : [now()]) as [number]));
          }
          static now() {
            return now();
          }
        }
        (window as unknown as { Date: DateConstructor }).Date = Shifted as unknown as DateConstructor;
      }, shift);
      const { served, frames } = await openReal(page, birta, { BIRTA_REVIEW_HOLD_MS: "300" });
      await keep(page, "say why");
      await expect.poll(() => turnOf(page)).toMatch(/^Hand back · 1( |$)/);
      expect(await phraseOf(page)).toBe("your turn");
      // Flip time on the real clock, which the shift doesn't touch
      await page.evaluate(() => {
        const w = window as unknown as { __flip: number | null };
        w.__flip = null;
        const t = setInterval(() => {
          const h = document.querySelector(".file-header birta-phrase");
          const text = h
            ? Array.from((h.shadowRoot ?? h).childNodes)
                .filter((c) => c.nodeName !== "STYLE")
                .map((c) => c.textContent ?? "")
                .join("")
            : "";
          if (/^\s*sent /.test(text)) {
            w.__flip = performance.timeOrigin + performance.now();
            clearInterval(t);
          }
        }, 5);
      });
      await page.keyboard.press("ControlOrMeta+Enter");
      await expect.poll(() => linesNamed(served.root, "submit").length).toBe(1);
      const submit = linesNamed(served.root, "submit")[0];
      await expect
        .poll(() => phraseOf(page), { timeout: 3_000 })
        .toBe(`sent ${hhmm(submit.not_before)} · not picked up yet`);
      const flip = await page.evaluate(() => (window as unknown as { __flip: number }).__flip);
      expect(flip).toBeGreaterThanOrEqual(submit.not_before - 20);
      expect(flip).toBeLessThan(submit.not_before + 500);
      const after = frames.sent.slice(frames.sent.findIndex((f) => f.msg.op === "hand_back") + 1);
      expect(after.filter((f) => f.msg.type !== "compose").map((f) => f.msg.type)).toEqual([]);
    });
  }
});

// --- the beacon ---------------------------------------------------------------------------

test.describe("beacon", () => {
  /** A flush body carrying one doc note. */
  const flushBody = (token: unknown, body: string) => ({
    token,
    tab: "beacon-tab",
    path: "pr.md",
    seen: [],
    ops: [{ type: "op", cid: `beacon-${body}`, path: "pr.md", rev: revOf(DOC), op: "comment", at: { k: "doc" }, body }],
  });

  test("a beacon with the page's token lands its note in the review file", async ({ page, birta }) => {
    const { served } = await openReal(page, birta);
    const queued = await page.evaluate((body) => {
      const b = { ...body, token: (window as unknown as { BOOT: { review: { token: string } } }).BOOT.review.token };
      return navigator.sendBeacon("/review/flush", new Blob([JSON.stringify(b)], { type: "application/json" }));
    }, flushBody(null, "via the beacon"));
    expect(queued).toBe(true);
    await expect.poll(() => linesNamed(served.root, "comment").map((l) => l.body)).toEqual(["via the beacon"]);
  });

  test("a wrong or missing token is refused and writes nothing", async ({ page, birta }) => {
    const { served } = await openReal(page, birta);
    const status = await page.evaluate(
      async (bodies) => {
        const out: number[] = [];
        for (const b of bodies)
          out.push((await fetch("/review/flush", { method: "POST", body: JSON.stringify(b) })).status);
        return out;
      },
      [flushBody("not-the-token", "wrong"), flushBody(undefined, "missing"), flushBody("", "empty")],
    );
    expect(status).toEqual([403, 403, 403]);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)).toBeNull();
  });

  test("a beacon from another origin is refused, even with the token", async ({ page, birta }) => {
    const { served } = await openReal(page, birta);
    const token = await page.evaluate(() => (window as unknown as { BOOT: { review: { token: string } } }).BOOT.review.token);
    const res = await page.request.post(`${served.url}/review/flush`, {
      headers: { Origin: "http://evil.example", "Content-Type": "application/json" },
      data: JSON.stringify(flushBody(token, "from elsewhere")),
    });
    expect(res.status()).toBeGreaterThanOrEqual(400);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)).toBeNull();
  });

  // K4, server side: a beacon never hands back or approves, whatever it carries
  test("a beacon carrying a hand-back or an approve writes neither", async ({ page, birta }) => {
    const { served } = await openReal(page, birta);
    await keep(page, "kept first");
    await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
    const before = reviewLog(served.root)!.length;
    const status = await page.evaluate(
      async ({ rev }) => {
        const token = (window as unknown as { BOOT: { review: { token: string } } }).BOOT.review.token;
        const body = (op: Record<string, unknown>) => ({
          token,
          tab: "beacon-tab",
          path: "pr.md",
          seen: [],
          ops: [{ type: "op", cid: `b-${op.op}`, path: "pr.md", rev, ...op }],
        });
        const out: number[] = [];
        for (const op of [{ op: "hand_back", scope: "doc" }, { op: "approve" }])
          out.push((await fetch("/review/flush", { method: "POST", body: JSON.stringify(body(op)) })).status);
        return out;
      },
      { rev: revOf(DOC) },
    );
    expect(status.every((s) => s < 500)).toBe(true);
    await page.waitForTimeout(SETTLE_MS);
    expect(reviewLog(served.root)!.length).toBe(before);
  });
});

// --- seen ---------------------------------------------------------------------------------

test.describe("seen", () => {
  test("after a real agent round, y on its change marks it seen in the log, the server's state and a reload", async ({
    page,
    birta,
  }) => {
    const { served, frames } = await openReal(page, birta, { BIRTA_REVIEW_HOLD_MS: "300" });
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await keep(page, "say why");
    await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
    const note = linesNamed(served.root, "comment")[0];
    expect(note.at).toMatchObject({ scope: "block" });
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => phraseOf(page), { timeout: 3_000 }).toMatch(/^sent /);

    const picked = agent(served.root, ["review", "pr.md", "--as", "claude"]);
    expect(picked.status, picked.stderr).toBe(0);
    served.write("pr.md", DOC.replace("we just fail.", "we just fail, because S3 answers 503 under load."));
    // respond records nothing until every change of the turn cites a note
    const responded = agent(
      served.root,
      ["respond", "pr.md", "--as", "claude"],
      `#${note.n} done\n#${note.n} covers "because S3 answers 503 under load"\n`,
    );
    expect(responded.status, responded.stdout + responded.stderr).toBe(0);

    await expect.poll(() => linesNamed(served.root, "yield").length).toBe(1);
    const replied = `Claude replied ${hhmm(linesNamed(served.root, "yield")[0].ts)}`;
    await expect.poll(() => phraseOf(page), { timeout: 5_000 }).toMatch(new RegExp(`^${replied} · [1-9]\\d* to check$`));
    // The button never counts
    await expect.poll(() => turnOf(page)).toMatch(/^Approve(?! ·)/);
    const agentChange = () => {
      const s = [...frames.got].reverse().find((f) => f.msg.type === "review_state" && f.msg.path === "pr.md")?.msg.state;
      return (s?.changes ?? []).find((c: State) => c.attr === "agent") as State | undefined;
    };
    await expect.poll(() => agentChange()?.range?.sp).toBeDefined();
    const id = agentChange()!.id as string;
    const sp = agentChange()!.range.sp as string;
    // Walking to it checks nothing: only y does
    for (let i = 0; i < 5 && (await focused(page)) !== sp; i++) {
      await page.keyboard.press("n");
      await page.waitForTimeout(150);
    }
    await expect.poll(() => focused(page)).toBe(sp);
    await page.waitForTimeout(300);
    expect(linesNamed(served.root, "seen")).toEqual([]);
    await page.keyboard.press("y");
    await expect.poll(() => linesNamed(served.root, "seen").some((l) => l.ids.includes(id))).toBe(true);
    // One y, one seen line
    expect(linesNamed(served.root, "seen")).toHaveLength(1);
    const seenInState = (socket: number) =>
      frames.got.some(
        (f) =>
          f.socket === socket &&
          f.msg.type === "review_state" &&
          (f.msg.state?.changes ?? []).some((c: State) => c.id === id && c.seen === true),
      );
    await expect.poll(() => seenInState(0)).toBe(true);
    await expect.poll(() => phraseOf(page)).toBe(`${replied} · all checked`);

    await page.reload();
    await booted(page);
    await expect.poll(() => seenInState(1)).toBe(true);
    await expect.poll(() => phraseOf(page)).toBe(`${replied} · all checked`);
    await expect.poll(() => turnOf(page)).toMatch(/^Approve(?! ·)/);
  });
});

// --- one writer at a time ----------------------------------------------------------------

const HELD = "Claude has the doc · this works again after its reply";
/** Claude's edit of the Why paragraph; only its words are new. */
const WRITTEN = DOC.replace("we just fail.", "we just fail, because S3 answers 503 under load.");
const ONLY_WRITTEN = "because S3 answers 503 under load";

const docText = (page: Page) => page.evaluate(() => document.getElementById("content")!.textContent ?? "");
const bars = async (page: Page) => (await marks(page)).filter((m) => m.mark === "bar" && m.width > 0 && m.height > 0);

/** The page hands back a note on the Why paragraph and Claude picks it up. */
async function handedBack(page: Page, birta: Start) {
  const opened = await openReal(page, birta, { BIRTA_REVIEW_HOLD_MS: "300" });
  for (let i = 0; i < 3; i++) await page.keyboard.press("j");
  await keep(page, "say why");
  await expect.poll(() => linesNamed(opened.served.root, "comment").length).toBe(1);
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect.poll(() => phraseOf(page), { timeout: 3_000 }).toMatch(/^sent /);
  const picked = agent(opened.served.root, ["review", "pr.md", "--as", "claude"]);
  expect(picked.status, picked.stderr).toBe(0);
  const claim = linesNamed(opened.served.root, "claim")[0];
  await expect.poll(() => phraseOf(page)).toBe(`Claude picked up ${hhmm(claim.ts)}`);
  return { ...opened, note: linesNamed(opened.served.root, "comment")[0].n as number };
}

/** Claude writes its edit, and birta records it. */
async function claudeWrites(root: string, write: (rel: string, text: string) => void) {
  write("pr.md", WRITTEN);
  await expect.poll(() => linesNamed(root, "external").map((l) => l.to_rev)).toEqual([revOf(WRITTEN)]);
}

function respond(root: string, note: number) {
  const r = agent(root, ["respond", "pr.md", "--as", "claude"], `#${note} done\n#${note} covers "${ONLY_WRITTEN}"\n`);
  expect(r.status, r.stdout + r.stderr).toBe(0);
  expect(linesNamed(root, "yield")).toHaveLength(1);
}

/** Polls for `ms` that the page still shows the handed-back text, with no bar of Claude's. */
async function holdsHandedBack(page: Page, ms: number, when: string) {
  const end = Date.now() + ms;
  do {
    const text = await docText(page);
    expect(text, `the handed-back text ${when}`).toContain("we just fail.");
    expect(text, `Claude's text ${when}`).not.toContain(ONLY_WRITTEN);
    expect(await bars(page), `Claude's bars ${when}`).toEqual([]);
    await page.waitForTimeout(100);
  } while (Date.now() < end);
}

test.describe("one writer at a time", () => {
  test("Claude's write mid-turn doesn't change the page; its text, rows and bars arrive with the reply", async ({
    page,
    birta,
  }) => {
    const { served, frames, note } = await handedBack(page, birta);
    await claudeWrites(served.root, served.write);
    await holdsHandedBack(page, 1_500, "after Claude's write");
    expect(
      frames.got.filter((f) => f.socket === 0 && f.msg.type === "content" && f.msg.rev === revOf(WRITTEN)),
      "no content of Claude's text mid-turn",
    ).toEqual([]);
    await expect.poll(() => turnOf(page)).toBe("With Claude");
    respond(served.root, note);
    await expect.poll(() => docText(page), { timeout: 5_000 }).toContain(ONLY_WRITTEN);
    await expect.poll(async () => (await bars(page)).some((b) => b.check === "new")).toBe(true);
    await expect.poll(() => phraseOf(page)).toMatch(/^Claude replied \d\d:\d\d · [1-9]\d* to check$/);
    // One frame pair: the content of Claude's text, then at once the state that shows its rows
    const mine = frames.got.filter((f) => f.socket === 0);
    const at = mine.findIndex((f) => f.msg.type === "content" && f.msg.rev === revOf(WRITTEN));
    expect(at).toBeGreaterThanOrEqual(0);
    expect(mine[at + 1]?.msg.type).toBe("review_state");
    expect(mine[at + 1]?.msg.state?.held ?? null).toBeNull();
    expect((mine[at + 1]?.msg.state?.changes ?? []).length).toBeGreaterThan(0);
  });

  test("a reload mid-turn shows the doc as handed back, and the reply lands without another reload", async ({
    page,
    birta,
  }) => {
    const { served, note } = await handedBack(page, birta);
    await claudeWrites(served.root, served.write);
    await page.reload();
    await booted(page);
    await page.evaluate(() => ((window as unknown as { __sameLoad: boolean }).__sameLoad = true));
    await holdsHandedBack(page, 1_000, "after a reload mid-turn");
    expect(await phraseOf(page)).toBe(`Claude picked up ${hhmm(linesNamed(served.root, "claim")[0].ts)}`);
    respond(served.root, note);
    await expect.poll(() => docText(page), { timeout: 5_000 }).toContain(ONLY_WRITTEN);
    await expect.poll(async () => (await bars(page)).length).toBeGreaterThan(0);
    await expect.poll(() => phraseOf(page)).toMatch(/^Claude replied /);
    expect(
      await page.evaluate(() => (window as unknown as { __sameLoad?: boolean }).__sameLoad === true),
      "no reload brought the reply",
    ).toBe(true);
  });

  test("a reload mid-turn never paints Claude's text, not even before the page boots", async ({ page, birta }) => {
    await page.addInitScript((words) => {
      const w = window as unknown as { __leak: boolean };
      w.__leak = false;
      const check = () => {
        if ((document.getElementById("content")?.textContent ?? "").includes(words)) w.__leak = true;
      };
      new MutationObserver(check).observe(document, { childList: true, subtree: true, characterData: true });
    }, ONLY_WRITTEN);
    const { served, note } = await handedBack(page, birta);
    await claudeWrites(served.root, served.write);
    await page.reload();
    await booted(page);
    await holdsHandedBack(page, 1_000, "after a reload mid-turn");
    expect(await page.evaluate(() => (window as unknown as { __leak: boolean }).__leak), "Claude's text painted").toBe(false);
    respond(served.root, note);
    await expect.poll(() => docText(page), { timeout: 5_000 }).toContain(ONLY_WRITTEN);
  });

  test("a write that reaches the socket before its hello is answered is never painted", async ({ page, birta }) => {
    const EARLY = "Claude's words, arriving early";
    await page.addInitScript((words) => {
      const w = window as unknown as { __leak: boolean };
      w.__leak = false;
      const check = () => {
        if ((document.getElementById("content")?.textContent ?? "").includes(words)) w.__leak = true;
      };
      new MutationObserver(check).observe(document, { childList: true, subtree: true, characterData: true });
    }, EARLY);
    const { served, note } = await handedBack(page, birta);
    await claudeWrites(served.root, served.write);
    // Claude's content lands between the page's hello and the server's answer to it
    await page.routeWebSocket(/\/ws$/, (ws) => {
      const server = ws.connectToServer();
      ws.onMessage((raw) => {
        const msg = JSON.parse(raw.toString());
        if (msg.type === "hello") {
          ws.send(
            JSON.stringify({ type: "content", path: "pr.md", rev: revOf(WRITTEN), html: `<p>${EARLY}</p>`, source: "", file_stats: "", stops: [] }),
          );
        }
        server.send(raw);
      });
    });
    await page.reload();
    await booted(page);
    await holdsHandedBack(page, 1_000, "after a write slipped in before the hello's answer");
    expect(await docText(page)).not.toContain(EARLY);
    expect(await page.evaluate(() => (window as unknown as { __leak: boolean }).__leak), "the early write painted").toBe(false);
    respond(served.root, note);
    await expect.poll(() => docText(page), { timeout: 5_000 }).toContain(ONLY_WRITTEN);
  });

  test("when the reply brings several writes, focus goes to the block that took its place", async ({ page, birta }) => {
    const { served, note } = await handedBack(page, birta);
    const before = await focused(page);
    expect(before, "setup: focus on the Why paragraph").toBe(PARA);
    // Claude rewrites the focused paragraph, then adds a line at the top in a second save
    const rewritten = DOC.replace("When the upload **hits** a 503 we just fail.", "An upload that hits a 503 is retried.");
    served.write("pr.md", rewritten);
    await expect.poll(() => linesNamed(served.root, "external").length).toBe(1);
    const both = rewritten.replace("# Upload retries\n", "# Upload retries\n\nA summary line.\n");
    served.write("pr.md", both);
    await expect.poll(() => linesNamed(served.root, "external").length).toBe(2);
    const r = agent(
      served.root,
      ["respond", "pr.md", "--as", "claude"],
      `#${note} done\n#${note} covers "is retried"\nunasked "A summary line.": a summary helps\n`,
    );
    expect(r.status, r.stdout + r.stderr).toBe(0);
    await expect.poll(() => docText(page), { timeout: 5_000 }).toContain("is retried");
    const sp = spAt(both, "An upload that hits a 503 is retried.");
    await expect.poll(() => focused(page)).toBe(sp);
  });

  test("in a folder, a theme change mid-turn keeps the doc as handed back", async ({ page, birta }) => {
    const served = await birta({ "index.md": "# Index\n", "pr.md": DOC }, { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "300" }, ["."]);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await keep(page, "tighten it");
    await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => linesNamed(served.root, "submit").length).toBe(1);
    await expect.poll(() => phraseOf(page), { timeout: 3_000 }).toMatch(/^sent /);
    const picked = agent(served.root, ["review", "pr.md", "--as", "claude"]);
    expect(picked.status, picked.stderr).toBe(0);
    await claudeWrites(served.root, served.write);
    await holdsHandedBack(page, 500, "after Claude's write");
    // Another tab picks a theme; every page gets it
    await page.evaluate(
      () =>
        new Promise<void>((done) => {
          const ws = new WebSocket(`ws://${location.host}/ws`);
          ws.onopen = () => {
            ws.send(JSON.stringify({ type: "theme_change", theme: "dracula" }));
            setTimeout(() => (ws.close(), done()), 300);
          };
        }),
    );
    await expect(page.locator("html")).toHaveAttribute("data-birta-theme", "dracula");
    await holdsHandedBack(page, 1_500, "after a theme change");
  });

  test("with Claude, u says Claude has the doc and writes nothing", async ({ page, birta }) => {
    const { served } = await handedBack(page, birta);
    await claudeWrites(served.root, served.write);
    const log = reviewLog(served.root)!.length;
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe(HELD);
    await page.waitForTimeout(SETTLE_MS);
    expect(fs.readFileSync(path.join(served.root, "pr.md"), "utf-8")).toBe(WRITTEN);
    expect(reviewLog(served.root)!.length).toBe(log);
  });

  test("with Claude, a task checkbox says Claude has the doc, writes nothing and stays as it was", async ({
    page,
    birta,
  }) => {
    const doc = DOC + "\n- [ ] Ran the integration suite\n";
    const served = await birta({ "pr.md": doc }, { ...GATE_ON, BIRTA_REVIEW_HOLD_MS: "300" });
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await keep(page, "tighten it");
    await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => phraseOf(page), { timeout: 3_000 }).toMatch(/^sent /);
    const picked = agent(served.root, ["review", "pr.md", "--as", "claude"]);
    expect(picked.status, picked.stderr).toBe(0);
    const log = reviewLog(served.root)!.length;
    const box = page.locator('#content input[type="checkbox"]');
    await box.click();
    await expect.poll(() => stripMessage(page)).toBe(HELD);
    await expect(box).not.toBeChecked();
    await expect(page.locator("#status")).not.toHaveText("changed since · try again");
    await page.waitForTimeout(SETTLE_MS);
    expect(fs.readFileSync(path.join(served.root, "pr.md"), "utf-8")).toBe(doc);
    expect(reviewLog(served.root)!.length).toBe(log);
  });

  test("in the hand-back's hold, u already says Claude has the doc and writes nothing", async ({ page, birta }) => {
    const { served } = await openReal(page, birta, { BIRTA_REVIEW_HOLD_MS: "8000" });
    for (let i = 0; i < 3; i++) await page.keyboard.press("j");
    await keep(page, "say why");
    await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect.poll(() => linesNamed(served.root, "submit").length).toBe(1);
    const submit = linesNamed(served.root, "submit")[0];
    const log = reviewLog(served.root)!.length;
    await page.keyboard.press("u");
    await expect.poll(() => stripMessage(page)).toBe(HELD);
    expect(Date.now(), "still inside the hold").toBeLessThan(submit.not_before);
    await page.waitForTimeout(SETTLE_MS);
    expect(fs.readFileSync(path.join(served.root, "pr.md"), "utf-8")).toBe(DOC);
    expect(reviewLog(served.root)!.length).toBe(log);
  });
});

// --- the resolver -------------------------------------------------------------------------

test.describe("selection through the resolver", () => {
  test("a selection over bold text keeps the source quote, shows the words, and repaints them after a reload", async ({
    page,
    birta,
  }) => {
    const { served } = await openReal(page, birta);
    await selectText(page, PARA, "upload hits a");
    await keep(page, "which upload?");
    await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
    const at = linesNamed(served.root, "comment")[0].at;
    expect(at).toMatchObject({ scope: "quote", quote: "upload **hits** a", rev: revOf(DOC) });
    expect(at.shown ?? "upload hits a").toBe("upload hits a");
    expect(at.pre.endsWith("When the ")).toBe(true);
    expect(at.suf.startsWith(" 503")).toBe(true);
    await expect.poll(() => highlighted(page)).toEqual(["upload hits a"]);

    await page.reload();
    await booted(page);
    await expect.poll(() => highlighted(page)).toEqual(["upload hits a"]);
  });

  test("a selection in a code block keeps the line's own text and repaints it after a reload", async ({ page, birta }) => {
    const { served } = await openReal(page, birta);
    await selectText(page, CODE, "upload, 3");
    await keep(page, "why three?");
    await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
    const at = linesNamed(served.root, "comment")[0].at;
    expect(at).toMatchObject({ scope: "quote", quote: "upload, 3", rev: revOf(DOC) });
    expect(at.pre.endsWith("retry(")).toBe(true);
    expect(at.suf.startsWith(");")).toBe(true);
    await expect.poll(() => highlighted(page)).toEqual(["upload, 3"]);

    await page.reload();
    await booted(page);
    await expect.poll(() => highlighted(page)).toEqual(["upload, 3"]);
  });

  test("the note's target line comes from the server: it names the quote's words", async ({ page, birta }) => {
    await openReal(page, birta);
    await selectText(page, PARA, "upload hits a");
    await keep(page, "which upload?");
    await expect(page.locator("birta-flow").filter({ hasText: "which upload?" })).toContainText('→ "upload hits a"');
    await page.reload();
    await booted(page);
    await expect(page.locator("birta-flow").filter({ hasText: "which upload?" })).toContainText('→ "upload hits a"');
  });

  // K8a
  test("in a code block the second of two equal lines keeps its own place and every space", async ({ page, birta }) => {
    const doc = ["# Twice", "", "```js", "retry(upload,   3);", "retry(upload,   3);", "```", ""].join("\n");
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.setViewportSize(WIDE);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const top = await selectText(page, "3:1-6:3", "upload,   3", true);
    await keep(page, "twice?");
    await expect.poll(() => linesNamed(served.root, "comment").length).toBe(1);
    const at = linesNamed(served.root, "comment")[0].at;
    expect(at).toMatchObject({ scope: "quote", quote: "upload,   3" });
    // Context is stored with its whitespace collapsed; the line says which copy it is
    expect(at.lines).toEqual([5, 5]);
    expect(at.pre.endsWith("3); retry(")).toBe(true);
    const tops = () =>
      page.evaluate(() => {
        const out: number[] = [];
        for (const [, hl] of (CSS as unknown as { highlights: Map<string, Set<Range>> }).highlights)
          for (const r of hl) out.push(r.getBoundingClientRect().top);
        return out;
      });
    await expect.poll(async () => (await tops()).map((t) => Math.round(t - top))).toEqual([0]);
    await page.reload();
    await booted(page);
    await expect.poll(async () => (await tops()).map((t) => Math.round(t - top))).toEqual([0]);
  });

  // K8b
  test("a quote note made by the agent's CLI shows its words, before and after a reload", async ({ page, birta }) => {
    const { served } = await openReal(page, birta);
    const made = agent(served.root, ["comment", "pr.md", "--quote", "upload hits a", "--as", "claude", "which upload?"]);
    expect(made.status, made.stderr).toBe(0);
    await expect(page.locator("birta-flow").filter({ hasText: "which upload?" })).toContainText('→ "upload hits a"');
    await page.reload();
    await booted(page);
    await expect(page.locator("birta-flow").filter({ hasText: "which upload?" })).toContainText('→ "upload hits a"');
  });
});
