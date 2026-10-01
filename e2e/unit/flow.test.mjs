import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ASSETS = path.join(HERE, "..", "..", "assets", "review");
const SHARED_BR = "var BR = BR && BR.__birta === true ? BR : { __birta: true };";
const ORDER = ["quote.js", "00-core.js", "10-keys.js", "20-pointing.js", "30-flow.js", "40-lane.js"];

/** Runs the review files in a context with no DOM, as they must touch none at load. */
function load(files = ORDER) {
  const ctx = vm.createContext({});
  for (const f of files) {
    const file = path.join(ASSETS, f);
    vm.runInContext(fs.readFileSync(file, "utf-8"), ctx, { filename: file });
  }
  return ctx;
}

test("30-flow.js starts by reusing the shared BR namespace", () => {
  assert.equal(fs.readFileSync(path.join(ASSETS, "30-flow.js"), "utf-8").split("\n")[0], SHARED_BR);
});

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the flow loads without a DOM, ${name}`, () => {
    const { BR } = load(files);
    assert.equal(typeof BR.pure.parseTypedQuote, "function");
    assert.equal(typeof BR.pure.laneMarks, "function");
    assert.equal(typeof BR.boot, "function");
  });
}

// --- parseTypedQuote: a leading "quote" in a note, and the words after it -------------------

const QUOTES = [
  ['"Stacked on" remove', { quote: "Stacked on", rest: "remove" }],
  ["“Stacked on” remove", { quote: "Stacked on", rest: "remove" }],
  ['"Stacked on"', { quote: "Stacked on", rest: "" }],
  ['"Stacked on"   remove it\nand this', { quote: "Stacked on", rest: "remove it\nand this" }],
  ['"a" "b"', { quote: "a", rest: '"b"' }],
  ['"it\'s" fine', { quote: "it's", rest: "fine" }],
  ["plain words", null],
  ['say "quoted" later', null],
  ['"unterminated words', null],
  ['""', null],
  ['"" remove', null],
  ["", null],
  ['  "a" b', null],
  ['"a"b', null],
  ['"   " b', null],
  ['"a\nb" c', null],
  ['"a” b', null],
  ['“a" b', null],
  ["'a' b", null],
];

for (const [text, want] of QUOTES) {
  test(`parseTypedQuote(${JSON.stringify(text)})`, () => {
    const { BR } = load();
    const got = BR.pure.parseTypedQuote(text);
    assert.deepEqual(got === null ? null : JSON.parse(JSON.stringify(got)), want);
  });
}
