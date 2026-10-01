import { Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  test,
  expect,
  GATE_ON,
  Mock,
  State,
  booted,
  cacheCdn,
  fixtureDoc,
  fixtureState,
  markKeys,
  misaligned,
  mockReview,
  revOf,
  reviewLog,
  watchSent,
  withRev,
} from "./harness";

const WIDE = { width: 1280, height: 900 };
const FRESH = fixtureDoc("fresh");
const UNSEEN = fixtureDoc("unseen");
const UNSEEN_KEYS = [
  "bar 5:1-6:68 solid",
  "bar 11:1-11:44 dotted",
  "glyph 5:1-6:68 no",
  "glyph 10:1-10:16 no",
  "wash 5:1-6:68",
  "wash 11:1-11:44",
  "tick after 14:1-14:13",
].sort();

/** Lets a push reach the page and any timer it starts run, then reports what is drawn. */
async function keysAfter(page: Page, ms: number): Promise<string[]> {
  await page.waitForTimeout(ms);
  return markKeys(page);
}

/** Sends a review op through the page's own client and returns the cid it chose. */
function sendOp(page: Page, op: object): Promise<string> {
  return page.evaluate((op) => (window as unknown as { BR: { op: (o: object) => string } }).BR.op(op), op);
}

/** What the outboxes for `path` hold, every tab's run together, or null when none does. */
function outbox(page: Page, path = "pr.md"): Promise<string | null> {
  return page.evaluate((p) => {
    const keys = Object.keys(localStorage).filter((k) => k.startsWith("birta-unacked:") && k.endsWith(":" + p));
    return keys.length ? keys.map((k) => localStorage.getItem(k)).join("") : null;
  }, path);
}

/** Frames of `type` from the socket that carried the op `cid`, and where the op sat among them. */
function onSocketOf(mock: Mock, cid: string) {
  const frames = mock.frames.filter((f) => f.msg.type === "op" && f.msg.cid === cid);
  return frames.map((f) => {
    const all = mock.frames.filter((g) => g.socket === f.socket).map((g) => g.msg);
    return { socket: f.socket, hello: all.findIndex((m) => m.type === "hello"), op: all.indexOf(f.msg), frame: f.msg };
  });
}

test.describe("rev gate", () => {
  test("a state ahead of the page waits for its content", async ({ page, birta }) => {
    const served = await birta({ "pr.md": FRESH, "next.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    const renders: string[] = [];
    page.on("request", (r) => {
      if (r.url().includes("/render/")) renders.push(r.url());
    });
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const html = await page.evaluate(() => fetch("/render/next.md").then((r) => r.text()));
    renders.length = 0;
    mock.pushState(fixtureState("unseen"));
    expect(await keysAfter(page, 250)).toEqual([]);
    mock.push({ type: "content", path: "pr.md", rev: revOf(UNSEEN), html });
    await expect.poll(() => markKeys(page), { timeout: 1_000 }).toEqual(UNSEEN_KEYS);
    expect(await misaligned(page)).toEqual([]);
    expect(renders).toEqual([]);
  });

  test("a state whose content never comes is drawn after the page refetches", async ({ page, birta }) => {
    const served = await birta({ "pr.md": FRESH }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    mock.holdContent = true;
    served.write("pr.md", UNSEEN);
    await page.waitForTimeout(400);
    const pushed = Date.now();
    const refetch = page.waitForRequest((r) => r.url().endsWith("/render/pr.md"), { timeout: 3_000 });
    mock.pushState(fixtureState("unseen"));
    expect(await keysAfter(page, 250)).toEqual([]);
    await refetch;
    expect(Date.now() - pushed, "the page waits about 500 ms for the content first").toBeGreaterThanOrEqual(400);
    expect(Date.now() - pushed, "and no longer").toBeLessThan(1_500);
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    expect(await misaligned(page)).toEqual([]);
    // The page now shows the refetched text, so ops name its rev
    mock.frames.length = 0;
    await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].rev).toBe(revOf(UNSEEN));
  });

  test("a refetch that finds another rev drops the state and shows that rev", async ({ page, birta }) => {
    const served = await birta({ "pr.md": FRESH }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    mock.holdContent = true;
    const moved = `${UNSEEN}\nOne more paragraph.\n`;
    served.write("pr.md", moved);
    await page.waitForTimeout(400);
    const refetch = page.waitForRequest((r) => r.url().endsWith("/render/pr.md"), { timeout: 3_000 });
    mock.pushState(fixtureState("unseen"));
    await refetch;
    await expect(page.locator("#content p", { hasText: "One more paragraph." })).toBeAttached();
    expect(await keysAfter(page, 600)).toEqual([]);
    // The page's rev comes from the render it fetched, never from the state
    mock.frames.length = 0;
    await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].rev).toBe(revOf(moved));
  });

  test("a state for a rev the page has left is dropped", async ({ page, birta }) => {
    const served = await birta({ "pr.md": FRESH }, GATE_ON);
    await page.setViewportSize(WIDE);
    const renders: string[] = [];
    page.on("request", (r) => {
      if (r.url().includes("/render/")) renders.push(r.url());
    });
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    served.write("pr.md", UNSEEN);
    await expect(page.locator("#content li", { hasText: "capped at 8" })).toBeAttached();
    renders.length = 0;
    // drafts.json names the rev the page showed first
    mock.pushState(fixtureState("drafts"));
    expect(await keysAfter(page, 900)).toEqual([]);
    expect(renders).toEqual([]);
  });

  test("a new rev clears the old marks until its own state comes", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, { state: fixtureState("unseen") });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    const next = `${UNSEEN}\nOne more paragraph.\n`;
    served.write("pr.md", next);
    await expect(page.locator("#content p", { hasText: "One more paragraph." })).toBeAttached();
    await expect.poll(() => markKeys(page), { timeout: 1_000 }).toEqual([]);
    mock.pushState(withRev(fixtureState("unseen"), revOf(next)));
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
  });

  test("a state for another doc is ignored", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    mock.pushState(fixtureState("unseen"), "other.md");
    expect(await keysAfter(page, 700)).toEqual([]);
  });

  test("a later snapshot replaces the earlier one", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, { state: fixtureState("unseen") });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    mock.pushState(withRev(fixtureState("fresh"), revOf(UNSEEN)));
    await expect.poll(() => markKeys(page)).toEqual([]);
  });

  test("a state sent before the review script ran is still drawn", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    // review.js is deferred behind the CDN scripts, so holding one lets the state arrive first
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/mermaid.min.js", async (route) => {
      await held;
      await route.fallback();
    });
    const mock = await mockReview(page, { state: fixtureState("unseen") });
    mock.onStatePushed = () => setTimeout(release, 100);
    // Waiting for load would deadlock: the held script only goes once the state is sent
    await page.goto(`${served.url}/view/pr.md`, { waitUntil: "commit" });
    await page.waitForLoadState("domcontentloaded");
    await booted(page);
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
  });
});

test.describe("outbox", () => {
  test("an op carries a cid, the page's path and the shown rev until it is acked", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const cid = await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toEqual({ type: "op", cid, path: "pr.md", rev: revOf(UNSEEN), op: "seen", ids: ["9.0"] });
    expect(await outbox(page)).toContain(cid);
    mock.ack(cid);
    await expect.poll(async () => (await outbox(page)) ?? "").not.toContain(cid);
  });

  test("an unacked op is replayed once after the next hello, as it was sent", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const cid = await sendOp(page, { op: "comment", at: { k: "doc" }, body: "keep me" });
    await expect.poll(() => mock.ops().length).toBe(1);
    const first = mock.ops()[0];
    await page.reload();
    await booted(page);
    await expect.poll(() => mock.ops().length).toBe(2);
    const [, replay] = onSocketOf(mock, cid);
    expect(replay.frame).toEqual(first);
    expect(replay.hello, "a hello opens the socket").toBeGreaterThanOrEqual(0);
    expect(replay.op, "the replay follows the hello").toBeGreaterThan(replay.hello);
    mock.ack(cid);
    await expect.poll(async () => (await outbox(page)) ?? "").not.toContain(cid);
    await page.reload();
    await booted(page);
    await page.waitForTimeout(500);
    expect(mock.ops().filter((o) => o.cid === cid)).toHaveLength(2);
  });

  test("a nacked op is dropped, not replayed", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const cid = await sendOp(page, { op: "comment", at: { k: "doc" }, body: "refused" });
    await expect.poll(() => mock.ops().length).toBe(1);
    mock.nack(cid, "changed_since");
    await expect.poll(async () => (await outbox(page)) ?? "").not.toContain(cid);
    await page.reload();
    await booted(page);
    await page.waitForTimeout(500);
    expect(mock.ops()).toHaveLength(1);
  });

  test("an op sent while the socket is down goes out after it reconnects", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const early = await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    await mock.drop();
    const late = await sendOp(page, { op: "seen", ids: ["10.0"] });
    await expect.poll(() => mock.ops().map((o) => o.cid), { timeout: 10_000 }).toEqual([early, early, late]);
    const replays = [...onSocketOf(mock, early).slice(1), ...onSocketOf(mock, late)];
    for (const r of replays) expect(r.op).toBeGreaterThan(r.hello);
    expect(new Set(replays.map((r) => r.socket)).size).toBe(1);
  });

  test("each doc keeps its own outbox", async ({ page, birta }) => {
    const served = await birta({ "a.md": "# A\n\nAlpha.\n", "b.md": "# B\n\nBeta.\n" }, GATE_ON, ["."]);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/a.md`);
    await booted(page);
    const cid = await sendOp(page, { op: "comment", at: { k: "doc" }, body: "on a" });
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].path).toBe("a.md");
    await page.goto(`${served.url}/view/b.md`);
    await booted(page);
    await page.waitForTimeout(500);
    expect(mock.ops()).toHaveLength(1);
    await page.goto(`${served.url}/view/a.md`);
    await booted(page);
    await expect.poll(() => mock.ops().filter((o) => o.cid === cid).length).toBe(2);
  });

  test("a new page load never reuses a cid", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const before = [await sendOp(page, { op: "seen", ids: ["9.0"] }), await sendOp(page, { op: "seen", ids: ["10.0"] })];
    await page.reload();
    await booted(page);
    const after = await sendOp(page, { op: "seen", ids: ["9.0"] });
    expect(new Set([...before, after]).size).toBe(3);
  });
});

test.describe("outbox, only notes and only this tab's", () => {
  const ACTS = [
    { op: "approve" },
    { op: "hand_back", scope: "doc" },
    { op: "undo_change", change: "10.0" },
    { op: "owner", drafted_by: null },
    { op: "withdraw", to: 3 },
    { op: "release", claim: 3 },
  ];

  test("an approve, a hand-back, an undo, an owner change, a withdraw or a release is never kept, so a reload sends none again", async ({
    page,
    birta,
  }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const acts: string[] = [];
    for (const op of ACTS) acts.push(await sendOp(page, op));
    const note = await sendOp(page, { op: "comment", at: { k: "doc" }, body: "keep me" });
    await expect.poll(() => mock.ops().length).toBe(ACTS.length + 1);
    const kept = (await outbox(page)) ?? "";
    expect(kept).toContain(note);
    for (const cid of acts) expect(kept).not.toContain(cid);
    await page.reload();
    await booted(page);
    await expect.poll(() => mock.ops().filter((o) => o.cid === note).length).toBe(2);
    await page.waitForTimeout(500);
    expect(mock.ops().filter((o) => acts.includes(o.cid))).toHaveLength(ACTS.length);
  });

  test("an outbox kept under the old key is dropped at boot, and none of it is sent", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const sent = watchSent(page);
    const rev = revOf(UNSEEN);
    const old = [...ACTS.slice(0, 3), { op: "comment", at: { k: "doc" }, body: "from an old page" }].map((op, i) => ({
      type: "op",
      cid: `rforeign-${i + 1}`,
      path: "pr.md",
      rev,
      ...op,
    }));
    await page.addInitScript((box) => {
      if (!sessionStorage.getItem("seeded")) localStorage.setItem("birta-outbox:pr.md", box);
      sessionStorage.setItem("seeded", "1");
    }, JSON.stringify(old));
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.waitForTimeout(1_000);
    expect(sent.filter((f) => f.type === "op")).toEqual([]);
    expect(reviewLog(served.root)).toBeNull();
    expect(await page.evaluate(() => localStorage.getItem("birta-outbox:pr.md"))).toBeNull();
  });

  test("another tab's outbox is never sent from this one", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const sent = watchSent(page);
    const frame = { type: "op", cid: "rother-1", path: "pr.md", rev: revOf(UNSEEN), op: "comment", at: { k: "doc" }, body: "theirs" };
    await page.addInitScript((box) => localStorage.setItem("birta-unacked:othertab:pr.md", box), JSON.stringify([frame]));
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.waitForTimeout(1_000);
    expect(sent.filter((f) => f.type === "op")).toEqual([]);
    expect(reviewLog(served.root)).toBeNull();
  });

  test("an undo lost with its socket leaves x free to undo again, and is not sent twice", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, { state: fixtureState("unseen"), acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const receipt = page.locator("birta-flow", { hasText: "nobody asked" }).filter({ hasText: "x undo" }).first();
    await receipt.click();
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().filter((o) => o.op === "undo_change").length).toBe(1);
    const first = mock.ops().find((o) => o.op === "undo_change")!.cid;
    await mock.drop();
    await expect.poll(() => mock.frames.filter((f) => f.msg.type === "hello").length, { timeout: 10_000 }).toBe(2);
    await receipt.click();
    await page.keyboard.press("x");
    await expect.poll(() => mock.ops().filter((o) => o.op === "undo_change").length).toBe(2);
    expect(mock.ops().filter((o) => o.cid === first)).toHaveLength(1);
  });
});

test.describe("a doc element with the id BR", () => {
  const DOC = '# T\n\n<a id="BR">x</a>\n\ntext\n';

  for (const gate of ["off", "on"] as const) {
    test(`with review ${gate}, the page keeps its socket and live reload, and nothing throws`, async ({ page, birta }) => {
      const served = await birta({ "pr.md": DOC }, gate === "on" ? GATE_ON : {});
      const errors: string[] = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.goto(`${served.url}/view/pr.md`);
      // The doc's id reaches the page as is, so the global it names is the element
      await expect(page.locator('#content [id="BR"]')).toHaveCount(1);
      if (gate === "on") {
        await booted(page);
        expect(await page.evaluate(() => (window as unknown as { BR: { op?: unknown } }).BR.op !== undefined)).toBe(true);
      }
      served.write("pr.md", DOC.replace("text", "text, edited on disk"));
      await expect(page.locator("#content")).toContainText("text, edited on disk");
      expect(errors).toEqual([]);
    });
  }
});

test.describe("one socket", () => {
  test("the page opens one socket and says hello once", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, { state: fixtureState("unseen") });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    await page.waitForTimeout(300);
    expect(new Set(mock.frames.map((f) => f.socket)).size).toBe(1);
    expect(mock.frames.filter((f) => f.msg.type === "hello")).toHaveLength(1);
  });

  test("a checkbox still writes with review on", async ({ page, birta }) => {
    const doc = "# Tasks\n\n- [ ] one\n- [ ] two\n";
    const served = await birta({ "pr.md": doc }, GATE_ON);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.locator("#content input[type=checkbox]").nth(1).check();
    await expect
      .poll(() => fs.readFileSync(path.join(served.root, "pr.md"), "utf-8"))
      .toBe("# Tasks\n\n- [ ] one\n- [x] two\n");
  });
});

test.describe("hosts", () => {
  test("isHost trusts FLOW_HOSTS members and the lane hosts, never doc elements or names", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const got = await page.evaluate(() => {
      const BR = (window as unknown as { BR: { isHost: (n: unknown) => boolean; FLOW_HOSTS: WeakSet<object> } }).BR;
      const member = document.createElement("div");
      BR.FLOW_HOSTS.add(member);
      return {
        weakSet: BR.FLOW_HOSTS instanceof WeakSet,
        member: BR.isHost(member),
        over: BR.isHost(document.querySelector("birta-over")),
        under: BR.isHost(document.querySelector("birta-under")),
        para: BR.isHost(document.querySelector("#content p")),
        div: BR.isHost(document.createElement("div")),
        text: BR.isHost(document.querySelector("#content p")!.firstChild),
        none: BR.isHost(null),
        forged: BR.isHost(document.createElement("birta-flow")),
      };
    });
    expect(got).toEqual({
      weakSet: true,
      member: true,
      over: true,
      under: true,
      para: false,
      div: false,
      text: false,
      none: false,
      forged: false,
    });
  });
});

test.describe("rev gate, timing and races", () => {
  test("content that comes in time cancels the refetch", async ({ page, birta }) => {
    const served = await birta({ "pr.md": FRESH, "next.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    const renders: string[] = [];
    page.on("request", (r) => {
      if (r.url().includes("/render/")) renders.push(r.url());
    });
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const html = await page.evaluate(() => fetch("/render/next.md").then((r) => r.text()));
    renders.length = 0;
    mock.pushState(fixtureState("unseen"));
    await page.waitForTimeout(100);
    mock.push({ type: "content", path: "pr.md", rev: revOf(UNSEEN), html });
    await expect.poll(() => markKeys(page), { timeout: 1_000 }).toEqual(UNSEEN_KEYS);
    // Well past the deadline the push started
    await page.waitForTimeout(900);
    expect(renders).toEqual([]);
    expect(await markKeys(page)).toEqual(UNSEEN_KEYS);
  });

  test("a stream of states ahead does not put the refetch off, and the newest wins", async ({ page, birta }) => {
    const served = await birta({ "pr.md": FRESH }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    mock.holdContent = true;
    served.write("pr.md", UNSEEN);
    await page.waitForTimeout(400);
    const refetch = page.waitForRequest((r) => r.url().endsWith("/render/pr.md"), { timeout: 3_000 });
    const pushed = Date.now();
    mock.pushState(fixtureState("unseen"));
    // The same rev without its verdicts: drawn only if the newest buffered state wins
    const newest = fixtureState("unseen");
    newest.items = [];
    const stream = setInterval(() => mock.pushState(newest), 150);
    try {
      await refetch;
    } finally {
      clearInterval(stream);
    }
    expect(Date.now() - pushed, "the deadline runs from the first state ahead").toBeLessThan(1_200);
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS.filter((k) => !k.startsWith("glyph")));
  });

  test("a refetch that lands after newer content does not roll the page back", async ({ page, birta }) => {
    const served = await birta({ "pr.md": FRESH }, GATE_ON);
    await page.setViewportSize(WIDE);
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const old: Record<string, { body: string; rev: string }> = {};
    await page.route("**/render/pr.md*", async (route) => {
      // Answered late with the render from before the next write
      await held;
      const got = old[route.request().url().includes("mode=source") ? "source" : "html"];
      await route.fulfill({ contentType: "text/html", headers: { "x-birta-rev": got.rev }, body: got.body });
    });
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    mock.holdContent = true;
    served.write("pr.md", UNSEEN);
    await page.waitForTimeout(400);
    for (const [key, query] of [["html", ""], ["source", "?mode=source"]]) {
      const res = await fetch(`${served.url}/render/pr.md${query}`);
      old[key] = { body: await res.text(), rev: res.headers.get("x-birta-rev")! };
    }
    expect(old.html.rev).toBe(revOf(UNSEEN));
    const refetch = page.waitForRequest((r) => r.url().endsWith("/render/pr.md"), { timeout: 3_000 });
    mock.pushState(fixtureState("unseen"));
    await refetch;
    mock.holdContent = false;
    const newer = `${UNSEEN}\nOne more paragraph.\n`;
    served.write("pr.md", newer);
    await expect(page.locator("#content p", { hasText: "One more paragraph." })).toBeAttached();
    release();
    await page.waitForTimeout(600);
    await expect(page.locator("#content p", { hasText: "One more paragraph." })).toBeAttached();
    expect(await markKeys(page)).toEqual([]);
    mock.frames.length = 0;
    await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].rev).toBe(revOf(newer));
  });

  test("a refetch finds a doc whose name needs escaping", async ({ page, birta }) => {
    const name = "my notes #2.md";
    const served = await birta({ [name]: FRESH }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/${encodeURIComponent(name)}`);
    await booted(page);
    // Control: the server itself serves the escaped name
    expect(await page.evaluate((n) => fetch("/render/" + encodeURIComponent(n)).then((r) => r.status), name)).toBe(200);
    mock.holdContent = true;
    served.write(name, UNSEEN);
    await page.waitForTimeout(400);
    const refetch = page.waitForResponse((r) => r.url().includes("/render/") && !r.url().includes("mode=source"), {
      timeout: 3_000,
    });
    mock.pushState(fixtureState("unseen"));
    expect((await refetch).status()).toBe(200);
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
  });

  test("a refetch brings the raw view along", async ({ page, birta }) => {
    const served = await birta({ "pr.md": FRESH }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    mock.holdContent = true;
    served.write("pr.md", UNSEEN);
    await page.waitForTimeout(400);
    mock.pushState(fixtureState("unseen"));
    await expect.poll(() => markKeys(page), { timeout: 3_000 }).toEqual(UNSEEN_KEYS);
    await page.locator('#view-toggle .view-btn[data-view="raw"]').click();
    await expect(page.locator("#source-view")).toContainText("capped at 8");
  });

  test("the page's own variant sync fetches nothing, and its doc still shows in that variant", async ({ page, birta }) => {
    const doc = "# Code\n\n```js\nconst answer = 42;\n```\n";
    // In a folder the theme update carries the entry doc, not this one. This theme colours
    // code inline per variant, so a doc left in the old variant shows
    const served = await birta({ "pr.md": doc, "a.md": "# Entry\n" }, GATE_ON, [".", "--theme", "catppuccin"]);
    await page.setViewportSize(WIDE);
    // The OS prefers the variant the server did not render, so the page syncs it on open
    const html = await (await page.request.get(`${served.url}/view/pr.md`)).text();
    const boot = JSON.parse(html.match(/<script type="application\/json" id="birta-boot"[^>]*>([^<]*)<\/script>/)![1]);
    const want = boot.activeVariant === "dark" ? "light" : "dark";
    await page.emulateMedia({ colorScheme: want });
    const renders: string[] = [];
    page.on("request", (r) => {
      if (r.url().includes("/render/")) renders.push(r.url());
    });
    const themes: State[] = [];
    page.on("websocket", (ws) =>
      ws.on("framereceived", (f) => {
        try {
          const msg = JSON.parse(String(f.payload));
          if (msg.type === "theme_update") themes.push(msg);
        } catch {
          // not JSON
        }
      }),
    );
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    await page.waitForTimeout(900);
    // Control: the page did sync its variant, and the update was for another doc
    expect(themes.map((t) => [t.active_variant, t.path])).toEqual([[want, "a.md"]]);
    expect(renders).toEqual([]);
    const fresh = await page.evaluate(async () => {
      const t = document.createElement("template");
      t.innerHTML = await (await fetch("/render/pr.md", { cache: "no-store" })).text();
      return t.content.querySelector("pre")!.innerHTML;
    });
    expect(await page.locator("#content pre").evaluate((e) => e.innerHTML)).toBe(fresh);
  });

  test("a state for another doc fetches nothing and leaves the page alone", async ({ page, birta }) => {
    const other = "# Other\n\nA different doc.\n";
    const served = await birta({ "pr.md": UNSEEN, "other.md": other }, GATE_ON, ["."]);
    await page.setViewportSize(WIDE);
    const renders: string[] = [];
    page.on("request", (r) => {
      if (r.url().includes("/render/")) renders.push(r.url());
    });
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    renders.length = 0;
    mock.pushState(withRev(fixtureState("unseen"), revOf(other)), "other.md");
    await page.waitForTimeout(900);
    expect(renders).toEqual([]);
    expect(await markKeys(page)).toEqual([]);
    await expect(page.locator("#content")).not.toContainText("A different doc.");
  });

  test("a frame whose rev disagrees with its state is dropped", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    mock.push({ type: "review_state", path: "pr.md", rev: revOf(FRESH), state: fixtureState("unseen") });
    expect(await keysAfter(page, 900)).toEqual([]);
  });

  test("a checkbox ack does not move the page's rev ahead of its content", async ({ page, birta }) => {
    const doc = `${UNSEEN}\n- [ ] tick me\n`;
    const served = await birta({ "pr.md": doc }, GATE_ON);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    mock.holdContent = true;
    await page.locator("#content input[type=checkbox]").last().check();
    await expect.poll(() => fs.readFileSync(path.join(served.root, "pr.md"), "utf-8")).toContain("- [x] tick me");
    await page.waitForTimeout(300);
    mock.frames.length = 0;
    await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].rev, "the text on screen is still the unticked one").toBe(revOf(doc));
  });

  test("a theme refetch for another doc takes its rev from the render", async ({ page, birta }) => {
    const served = await birta({ "a.md": FRESH, "b.md": "# B\n" }, GATE_ON, ["."]);
    await page.setViewportSize(WIDE);
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/a.md`);
    await booted(page);
    mock.holdContent = true;
    served.write("a.md", UNSEEN);
    await page.waitForTimeout(400);
    const css = await page.evaluate(() => document.getElementById("theme-vars")!.textContent);
    const refetch = page.waitForResponse((r) => r.url().endsWith("/render/a.md"), { timeout: 3_000 });
    mock.push({
      type: "theme_update",
      path: "b.md",
      html: "<h1>B</h1>",
      theme_name: "github",
      active_variant: "light",
      css_vars: css,
      variants: ["light", "dark"],
      has_toggle: true,
      theme_attr: null,
    });
    await refetch;
    await expect(page.locator("#content li", { hasText: "capped at 8" })).toBeAttached();
    mock.pushState(fixtureState("unseen"), "a.md");
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
    mock.frames.length = 0;
    await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0].rev).toBe(revOf(UNSEEN));
  });

  test("a burst of frames before the review script runs still ends on the newest state", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    await page.setViewportSize(WIDE);
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/mermaid.min.js", async (route) => {
      await held;
      await route.fallback();
    });
    const mock = await mockReview(page, {});
    await page.goto(`${served.url}/view/pr.md`, { waitUntil: "commit" });
    await expect.poll(() => mock.frames.some((f) => f.msg.type === "hello")).toBe(true);
    // More than the viewer keeps for a script that hasn't loaded; only the newest may count
    const empty = withRev(fixtureState("fresh"), revOf(UNSEEN));
    for (let i = 0; i < 300; i++) mock.pushState(empty);
    mock.pushState(fixtureState("unseen"));
    await page.waitForTimeout(300);
    release();
    await page.waitForLoadState("domcontentloaded");
    await booted(page);
    await expect.poll(() => markKeys(page)).toEqual(UNSEEN_KEYS);
  });
});

test.describe("outbox, sharing and limits", () => {
  test("an ack settles only its own op, and a checkbox ack settles none", async ({ page, birta }) => {
    const doc = `${UNSEEN}\n- [ ] tick me\n`;
    const served = await birta({ "pr.md": doc }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const kept = await sendOp(page, { op: "comment", at: { k: "doc" }, body: "keep me" });
    const done = await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(2);
    // Checkbox frames pass the mock, so the real server acks them on the same socket
    await page.locator("#content input[type=checkbox]").last().check();
    await expect.poll(() => fs.readFileSync(path.join(served.root, "pr.md"), "utf-8")).toContain("- [x] tick me");
    expect(await outbox(page)).toContain(kept);
    expect(await outbox(page)).toContain(done);
    mock.ack(done);
    await expect.poll(async () => (await outbox(page)) ?? "").not.toContain(done);
    expect(await outbox(page)).toContain(kept);
    await page.reload();
    await booted(page);
    await expect.poll(() => mock.ops().filter((o) => o.cid === kept).length).toBe(2);
    expect(mock.ops().filter((o) => o.cid === done)).toHaveLength(1);
  });

  test("an ack in one tab leaves another tab's op in the outbox", async ({ page, birta, context }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const one = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const other = await context.newPage();
    await cacheCdn(other);
    const two = await mockReview(other, { acks: "manual" });
    await other.goto(`${served.url}/view/pr.md`);
    await booted(other);
    const first = await sendOp(page, { op: "comment", at: { k: "doc" }, body: "from tab one" });
    await expect.poll(() => one.ops().length).toBe(1);
    const second = await sendOp(other, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => two.ops().length).toBe(1);
    two.ack(second);
    await expect.poll(async () => (await outbox(other)) ?? "").not.toContain(second);
    expect(await outbox(page)).toContain(first);
  });

  test("an unacked op is replayed once when the socket opened before the review script ran", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const cid = await sendOp(page, { op: "comment", at: { k: "doc" }, body: "keep me" });
    await expect.poll(() => mock.ops().length).toBe(1);
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/mermaid.min.js", async (route) => {
      await held;
      await route.fallback();
    });
    await page.reload({ waitUntil: "commit" });
    // The viewer says hello while review.js still waits behind the held script
    await expect.poll(() => mock.frames.filter((f) => f.msg.type === "hello").length).toBe(2);
    release();
    await page.waitForLoadState("domcontentloaded");
    await booted(page);
    await expect.poll(() => mock.ops().filter((o) => o.cid === cid).length).toBe(2);
    await page.waitForTimeout(500);
    expect(mock.ops().filter((o) => o.cid === cid)).toHaveLength(2);
  });

  test("boot runs once however often the socket comes back", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const cid = await sendOp(page, { op: "seen", ids: ["9.0"] });
    await expect.poll(() => mock.ops().length).toBe(1);
    for (let round = 2; round <= 3; round++) {
      await mock.drop();
      await expect.poll(() => mock.frames.filter((f) => f.msg.type === "hello").length, { timeout: 10_000 }).toBe(round);
      // One subscription replays once per reconnect
      await expect.poll(() => mock.ops().filter((o) => o.cid === cid).length).toBe(round);
    }
    await page.waitForTimeout(500);
    expect(mock.ops().filter((o) => o.cid === cid)).toHaveLength(3);
    const hosts = await page.evaluate(() => [
      document.querySelectorAll("birta-under").length,
      document.querySelectorAll("birta-over").length,
    ]);
    expect(hosts).toEqual([1, 1]);
  });

  test("an op too large for the socket is refused and never queued", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    let sockets = 0;
    page.on("websocket", () => sockets++);
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const threw = await page.evaluate(() => {
      try {
        (window as unknown as { BR: { op: (o: object) => string } }).BR.op({
          op: "comment",
          at: { k: "doc" },
          body: "x".repeat(300 * 1024),
        });
        return false;
      } catch {
        return true;
      }
    });
    expect(threw).toBe(true);
    await page.waitForTimeout(3_000);
    expect(sockets, "no reconnect loop").toBeLessThanOrEqual(1);
    expect((await outbox(page)) ?? "").not.toContain("xxxxxxxx");
  });

  test("an op cannot override the fields the client owns", async ({ page, birta }) => {
    const served = await birta({ "pr.md": UNSEEN }, GATE_ON);
    const mock = await mockReview(page, { acks: "manual" });
    await page.goto(`${served.url}/view/pr.md`);
    await booted(page);
    const cid = await sendOp(page, { op: "seen", ids: ["9.0"], type: "checkbox", cid: "mine", path: "other.md", rev: "0".repeat(40) });
    expect(cid).not.toBe("mine");
    await expect.poll(() => mock.ops().length).toBe(1);
    expect(mock.ops()[0]).toEqual({ type: "op", cid, path: "pr.md", rev: revOf(UNSEEN), op: "seen", ids: ["9.0"] });
  });
});
