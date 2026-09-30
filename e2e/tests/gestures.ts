import { expect, Page } from "@playwright/test";

// The anchor.md gestures, shared by the extractor spec that recorded them and the review
// specs that replay them, so both act on exactly the same glyphs.

// mermaid.min.js is about 3.5 MB from the CDN
export const CDN_TIMEOUT = 30_000;

export const OBJ = "\u{fffc}";
export const T_PR =
  "This PR adds retry with jittered backoff to the upload() client – because S3 returns 503 & drops events… See the docs 🎉. He said “we just fail” and don’t *panic*.";

/** What `BR.quote.fromRange` reports, reduced to the fields the server consumes. */
export interface Quote {
  sp: string | null;
  quote: string;
  pre: string;
  suf: string;
  off: number | null;
  tl: number | null;
  crossBlock: boolean;
}

export interface Gesture {
  id: string;
  act: (page: Page) => Promise<void>;
  /** Expected quote, from the fixture text. Omit it where engines may pick different text. */
  quote?: string;
  /** Expected `crossBlock`, per engine where the engines' selections end differently. */
  crossBlock: boolean | Record<string, boolean>;
  /** Expected code-point offset and text length, where they are the point of the gesture. */
  at?: { off: number; tl: number };
}

export type Rect = { left: number; right: number; top: number; bottom: number };

/**
 * The on-screen box of the `nth` occurrence of `word` in the text under `scope`,
 * so gestures land on glyphs rather than on font-dependent offsets.
 */
export async function wordRect(page: Page, scope: string, word: string, nth = 0): Promise<Rect> {
  const rect = await page.evaluate(
    ({ scope, word, nth }) => {
      const root = document.querySelector(scope);
      if (!root) return null;
      const box = root.getBoundingClientRect();
      if (box.top < 60 || box.bottom > innerHeight) root.scrollIntoView({ block: "center" });
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
        // KaTeX output is not text a reader would aim at
        acceptNode: (n) =>
          n.parentElement?.closest("[data-math-style]") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
      });
      let seen = 0;
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const data = (n as Text).data;
        for (let at = data.indexOf(word); at >= 0; at = data.indexOf(word, at + 1)) {
          if (seen++ < nth) continue;
          const r = document.createRange();
          r.setStart(n, at);
          r.setEnd(n, at + word.length);
          const b = r.getBoundingClientRect();
          return { left: b.left, right: b.right, top: b.top, bottom: b.bottom };
        }
      }
      return null;
    },
    { scope, word, nth },
  );
  if (!rect) throw new Error(`no "${word}" #${nth} under ${scope}`);
  return rect;
}

export const mid = (r: Rect) => ({ x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 });

export async function dblclickWord(page: Page, scope: string, word: string, nth = 0) {
  const { x, y } = mid(await wordRect(page, scope, word, nth));
  await page.mouse.dblclick(x, y);
}

export async function tripleClickWord(page: Page, scope: string, word: string) {
  const { x, y } = mid(await wordRect(page, scope, word));
  await page.mouse.click(x, y, { clickCount: 3 });
}

/** Drags from just inside the first glyph of `from` to just inside the last glyph of `to`. */
export async function dragWords(page: Page, from: [string, string], to: [string, string]) {
  await wordRect(page, ...from);
  const b = await wordRect(page, ...to);
  // Measured after `to` is on screen, in case reaching it scrolled
  const a = await wordRect(page, ...from);
  await page.mouse.move(a.left + 1, (a.top + a.bottom) / 2);
  await page.mouse.down();
  await page.mouse.move(b.right - 1, (b.top + b.bottom) / 2, { steps: 8 });
  await page.mouse.up();
}

export const sp = (s: string) => `[data-sourcepos="${s}"]`;
export const P_PR = sp("3:1-3:213");
export const P_RETRY = sp("5:1-7:70");

// The research gesture matrix (1-11) plus repeats, code, raw HTML and hidden text.
export const GESTURES: Gesture[] = [
  {
    id: "01-dblclick-emphasis",
    act: (p) => dblclickWord(p, P_PR, "jittered"),
    quote: "jittered",
    crossBlock: false,
  },
  {
    id: "02-triple-click-paragraph",
    act: (p) => tripleClickWord(p, P_PR, "backoff"),
    quote: T_PR,
    crossBlock: true,
  },
  {
    id: "03-triple-click-heading",
    act: (p) => tripleClickWord(p, "h1" + sp("1:1-1:26"), "design"),
    quote: "Retry “layer” — design",
    crossBlock: true,
  },
  {
    id: "04-triple-click-item-with-nested-list",
    act: (p) => tripleClickWord(p, "li" + sp("10:1-11:22"), "idempotency"),
    quote: "Checked idempotency on every POST",
    crossBlock: true,
  },
  {
    id: "05-triple-click-cell",
    act: (p) => tripleClickWord(p, "td" + sp("16:2-16:10"), "retries"),
    quote: "retries",
    crossBlock: { chromium: false, webkit: true },
  },
  {
    id: "06-triple-click-task-item",
    act: (p) => tripleClickWord(p, "li" + sp("9:1-9:31"), "integration"),
    quote: "Ran the integration suite",
    crossBlock: true,
  },
  {
    id: "07-drag-strong-to-link-end",
    act: (p) => dragWords(p, [P_PR, "adds"], [P_PR, "docs"]),
    quote:
      "adds retry with jittered backoff to the upload() client – because S3 returns 503 & drops events… See the docs",
    crossBlock: false,
  },
  {
    id: "08-drag-into-next-paragraph",
    act: (p) => dragWords(p, [P_PR, "adds"], [P_RETRY, "hard"]),
    quote: T_PR.slice(T_PR.indexOf("adds")),
    crossBlock: true,
  },
  {
    id: "09-drag-across-inline-math",
    act: (p) => dragWords(p, [P_RETRY, "three"], [P_RETRY, "end"]),
    quote: `three Ctrl and math ${OBJ} end`,
    crossBlock: false,
  },
  {
    id: "10-drag-across-footnote-ref",
    act: (p) => dragWords(p, [sp("99:1-99:39"), "Second"], [sp("99:1-99:39"), "with"]),
    quote: `Second ref${OBJ} with`,
    crossBlock: false,
  },
  {
    id: "11-dblclick-after-emoji",
    act: (p) => dblclickWord(p, P_PR, "panic"),
    quote: "panic",
    crossBlock: false,
  },
  {
    // Chromium won't double-click-select an emoji, so this one drags
    id: "12-drag-to-emoji-at-end",
    act: (p) => dragWords(p, [sp("74:5-74:35"), "café"], [sp("74:5-74:35"), "🎉"]),
    quote: "café and 🎉",
    crossBlock: false,
  },
  {
    id: "13-dblclick-repeated-word",
    act: (p) => dblclickWord(p, P_RETRY, "just", 1),
    quote: "just",
    crossBlock: false,
  },
  {
    id: "14-dblclick-in-code",
    act: (p) => dblclickWord(p, "pre" + sp("43:1-47:3"), "retry"),
    quote: "retry",
    crossBlock: false,
  },
  {
    id: "15-drag-in-raw-html",
    act: (p) => dragWords(p, ["details summary", "More"], ["details summary", "More"]),
    quote: "More",
    crossBlock: false,
  },
  {
    // The server can't see the hidden text, so this must never resolve to Exact
    id: "16-triple-click-hidden-span-paragraph",
    act: (p) => tripleClickWord(p, sp("89:1-89:46"), "Hidden"),
    quote: "Hidden inline text.",
    crossBlock: true,
  },
  {
    // The nested <ul> carries the same sp as this <li> and comes first on the page
    id: "17-dblclick-nested-item",
    act: (p) => dblclickWord(p, "li" + sp("11:3-11:22"), "nested"),
    quote: "nested",
    crossBlock: false,
  },
  {
    // A skin tone, a combining accent and a flag: 6 code points, 3 graphemes
    id: "18-dblclick-after-graphemes",
    act: (p) => dblclickWord(p, sp("125:1-125:34"), "ok"),
    quote: "ok",
    crossBlock: false,
    at: { off: 19, tl: 21 },
  },
  {
    // A range can start and end inside an atomic, which no mouse gesture does reliably
    id: "19-range-inside-footnote-ref",
    act: (p) =>
      useRange(p, () => {
        const t = document.querySelector('[data-sourcepos="5:1-7:70"] sup.footnote-ref a')!.firstChild!;
        const r = document.createRange();
        r.setStart(t, 0);
        r.setEnd(t, 1);
        return r;
      }),
    quote: OBJ,
    crossBlock: false,
  },
  {
    id: "20-range-from-inside-math",
    act: (p) =>
      useRange(p, () => {
        const math = document.querySelector('[data-sourcepos="5:1-7:70"] [data-math-style]')!;
        const walker = document.createTreeWalker(math.querySelector(".katex-html") ?? math, NodeFilter.SHOW_TEXT);
        const inside = walker.nextNode()!;
        const after = math.nextSibling as Text;
        const r = document.createRange();
        r.setStart(inside, 0);
        r.setEnd(after, " end".length);
        return r;
      }),
    quote: `${OBJ} end`,
    crossBlock: false,
  },
];

/** Hands `make`'s range to `selectionQuote` in place of the selection, which engines may move. */
export async function useRange(page: Page, make: () => Range) {
  await page.evaluate(`window.__range = (${make.toString()})()`);
}

/** Waits until diagrams and math are drawn, since drawing moves every block below them. */
export async function waitDrawn(page: Page) {
  await expect(page.locator("#content pre.mermaid:not(:has(svg))")).toHaveCount(0, { timeout: CDN_TIMEOUT });
  await expect(page.locator("#content [data-math-style]:not([data-math-rendered])")).toHaveCount(0, {
    timeout: CDN_TIMEOUT,
  });
}
