import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const QUOTE_JS = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "assets",
  "review",
  "quote.js",
);

/** Runs quote.js in a context with no DOM, as it must touch none at load. */
function load(globals = {}) {
  const ctx = vm.createContext(globals);
  vm.runInContext(fs.readFileSync(QUOTE_JS, "utf-8"), ctx, { filename: QUOTE_JS });
  return ctx;
}

test("starts by reusing the shared BR namespace", () => {
  const first = fs.readFileSync(QUOTE_JS, "utf-8").split("\n")[0];
  assert.equal(first, "var BR = typeof BR === 'undefined' ? {} : BR;");
});

test("loads without a DOM and exposes fromRange and paint", () => {
  const { BR } = load();
  assert.equal(typeof BR.quote.fromRange, "function");
  assert.equal(typeof BR.quote.paint, "function");
});

test("keeps what other review scripts put on BR", () => {
  const other = { keep: 1 };
  const { BR } = load({ BR: other });
  assert.equal(BR, other);
  assert.equal(BR.keep, 1);
  assert.equal(typeof BR.quote.fromRange, "function");
});
