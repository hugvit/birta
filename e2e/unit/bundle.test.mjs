import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ASSETS = path.join(HERE, "..", "..", "assets", "review");
const SHARED_BR = "var BR = BR && BR.__birta === true ? BR : { __birta: true };";
// Every review script, whatever is added later, so a new file can't skip the shared line
const ALL = fs
  .readdirSync(ASSETS)
  .filter((f) => f.endsWith(".js"))
  .sort();

/** Runs the files in order in one context with no DOM, as they must touch none at load. */
function load(files, globals = {}) {
  const ctx = vm.createContext(globals);
  for (const f of files) {
    const file = path.join(ASSETS, f);
    vm.runInContext(fs.readFileSync(file, "utf-8"), ctx, { filename: file });
  }
  return ctx;
}

test("there are review scripts to check", () => {
  assert.ok(ALL.includes("quote.js") && ALL.includes("00-core.js"), ALL.join(", "));
});

for (const f of ALL) {
  test(`${f} starts by reusing the shared BR namespace`, () => {
    assert.equal(fs.readFileSync(path.join(ASSETS, f), "utf-8").split("\n")[0], SHARED_BR);
  });
}

for (const [name, files] of [
  ["quote.js after 00-core.js", ["00-core.js", "quote.js"]],
  ["quote.js before 00-core.js", ["quote.js", "00-core.js"]],
]) {
  test(`${name}: one BR keeps what each file put on it`, () => {
    const seen = [];
    const ctx = load([files[0]]);
    seen.push(ctx.BR);
    vm.runInContext(fs.readFileSync(path.join(ASSETS, files[1]), "utf-8"), ctx);
    assert.equal(ctx.BR, seen[0], "the second file replaced BR");
    const { BR } = ctx;
    assert.equal(typeof BR.quote.fromRange, "function");
    assert.equal(typeof BR.quote.paint, "function");
    assert.equal(typeof BR.boot, "function");
    assert.equal(typeof BR.op, "function");
    assert.equal(typeof BR.isHost, "function");
    assert.equal(typeof BR.pure, "object");
    assert.ok(BR.FLOW_HOSTS instanceof vm.runInContext("WeakSet", ctx));
  });
}

for (const [name, files] of [
  ["in bundle order", ALL],
  ["in reverse order", [...ALL].reverse()],
]) {
  test(`every review script loads into one BR ${name}`, () => {
    const { BR } = load(files);
    assert.equal(typeof BR.quote.fromRange, "function");
    assert.equal(typeof BR.boot, "function");
    assert.equal(typeof BR.pure.laneMarks, "function");
    assert.equal(typeof BR.pure.keyAction, "function");
    assert.equal(typeof BR.pure.formatPhrase, "function");
  });
}

test("a BR made before the bundle is the one every file extends", () => {
  const before = { __birta: true, keep: 1 };
  const { BR } = load(ALL, { BR: before });
  assert.equal(BR, before);
  assert.equal(BR.keep, 1);
  assert.equal(typeof BR.quote.fromRange, "function");
  assert.equal(typeof BR.boot, "function");
});

test("a BR that isn't review's own, like a doc element named by its id, is never extended", () => {
  const element = { localName: "a", id: "BR" };
  for (const files of [ALL, [...ALL].reverse()]) {
    const { BR } = load(files, { BR: element });
    assert.notEqual(BR, element);
    assert.equal(BR.__birta, true);
    assert.equal(typeof BR.boot, "function");
    assert.equal(typeof BR.quote.fromRange, "function");
    assert.equal(element.boot, undefined);
  }
});

// K10: the page gets review_ws.rs's own list, joined into one script
const RS = fs.readFileSync(path.join(HERE, "..", "..", "src", "review_ws.rs"), "utf-8");
const SERVED = [...RS.matchAll(/include_str!\("\.\.\/assets\/review\/([^"]+\.js)"\)/g)].map((m) => m[1]);

test("the served bundle lists every review script", () => {
  assert.deepEqual([...SERVED].sort(), ALL);
});

test("the served bundle loads as the one script the page gets", () => {
  const js = SERVED.map((f) => fs.readFileSync(path.join(ASSETS, f), "utf-8")).join("\n") + '\nBR.shadowCss = "";\n';
  const ctx = vm.createContext({});
  vm.runInContext(js, ctx);
  assert.equal(typeof ctx.BR.boot, "function");
  assert.equal(typeof ctx.BR.quote.fromRange, "function");
});
