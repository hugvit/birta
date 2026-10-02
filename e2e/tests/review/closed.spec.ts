import {
  testClosed as test,
  expect,
  GATE_ON,
  booted,
  blockBox,
  fixtureDoc,
  fixtureState,
  frameBox,
  hex,
  mockReview,
  near,
  shot,
} from "./harness";

// No init script here: the roots stay as the page made them
test("the lane's shadow roots are closed and still paint", async ({ page, birta }) => {
  const served = await birta({ "pr.md": fixtureDoc("unseen") }, GATE_ON);
  await page.setViewportSize({ width: 1280, height: 900 });
  await mockReview(page, { state: fixtureState("unseen") });
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  const got = await page.evaluate(() =>
    ["birta-under", "birta-over"].map((t) => {
      const h = document.querySelector(`#lane-frame > ${t}`)!;
      return { root: h.shadowRoot === null, lightChildren: h.childNodes.length };
    }),
  );
  expect(got).toEqual([
    { root: true, lightChildren: 0 },
    { root: true, lightChildren: 0 },
  ]);
  // The unasked item is a dotted bar and the paragraph a solid one, both fresh, in the lane at x=20
  const frame = await frameBox(page);
  const p = (await blockBox(page, "5:1-6:68"))!;
  const x = frame.left + 20;
  await expect
    .poll(async () => {
      const s = await shot(page, { left: x - 1, top: p.top, width: 3, height: p.height });
      return near(s.at(x - 0.5, p.bottom - 3), hex("#bf8700"), 12);
    })
    .toBe(true);
  const li = (await blockBox(page, "11:1-11:44"))!;
  const s = await shot(page, frame);
  expect(near(s.at(li.list!.left + 3, (li.top + li.bottom) / 2), hex("#fff8c5"))).toBe(true);
});

test("a note is typed and kept in a closed root, and its keys are never verdicts", async ({ page, birta }) => {
  const served = await birta({ "pr.md": fixtureDoc("fresh") }, GATE_ON);
  await page.setViewportSize({ width: 1280, height: 900 });
  const mock = await mockReview(page);
  await page.goto(`${served.url}/view/pr.md`);
  await booted(page);
  await page.keyboard.press("c");
  // Focus sits in a closed root: the page sees only its host
  await expect
    .poll(() => page.evaluate(() => document.activeElement?.localName ?? null))
    .toBe("birta-flow");
  await page.keyboard.type("x marks y spot");
  await page.keyboard.press("Enter");
  await expect.poll(() => mock.ops().length).toBe(1);
  expect(mock.ops()[0]).toMatchObject({ op: "comment", at: { k: "doc" }, body: "x marks y spot" });
  expect(mock.ops()[0].verdict).toBeUndefined();
  const roots = await page.evaluate(() =>
    Array.from(document.querySelectorAll("birta-flow")).map((h) => h.shadowRoot === null),
  );
  expect(roots.length).toBeGreaterThan(0);
  expect(roots.every(Boolean)).toBe(true);
});
