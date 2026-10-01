import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const PATCH_JS = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "assets",
  "patch.js",
);

/** Runs patch.js in a context with no DOM, as it must touch none at load. */
function load() {
  const ctx = vm.createContext({});
  vm.runInContext(fs.readFileSync(PATCH_JS, "utf-8"), ctx, { filename: PATCH_JS });
  return ctx.birtaPatch;
}

function plan(oldKeys, newKeys) {
  const { prefix, suffix } = load().planPatch(oldKeys, newKeys);
  return [prefix, suffix];
}

/** What the page holds after keeping the prefix and suffix and swapping the middle. */
function applyPlan(oldKeys, newKeys, prefix, suffix) {
  return [
    ...oldKeys.slice(0, prefix),
    ...newKeys.slice(prefix, newKeys.length - suffix),
    ...oldKeys.slice(oldKeys.length - suffix),
  ];
}

test("loads without a DOM and exposes planPatch", () => {
  const patch = load();
  assert.equal(typeof patch, "object");
  assert.equal(typeof patch.planPatch, "function");
});

test("identical lists are all prefix", () => {
  assert.deepEqual(plan(["a", "b", "c"], ["a", "b", "c"]), [3, 0]);
});

test("empty lists", () => {
  assert.deepEqual(plan([], []), [0, 0]);
  assert.deepEqual(plan([], ["a"]), [0, 0]);
  assert.deepEqual(plan(["a"], []), [0, 0]);
});

test("insert at the start, middle and end", () => {
  assert.deepEqual(plan(["a", "b"], ["x", "a", "b"]), [0, 2]);
  assert.deepEqual(plan(["a", "b", "c"], ["a", "x", "b", "c"]), [1, 2]);
  assert.deepEqual(plan(["a", "b"], ["a", "b", "c"]), [2, 0]);
});

test("delete at the start, middle and end", () => {
  assert.deepEqual(plan(["x", "a", "b"], ["a", "b"]), [0, 2]);
  assert.deepEqual(plan(["a", "x", "b"], ["a", "b"]), [1, 1]);
  assert.deepEqual(plan(["a", "b", "x"], ["a", "b"]), [2, 0]);
});

test("a changed block is the whole middle", () => {
  assert.deepEqual(plan(["a", "b", "c"], ["a", "y", "c"]), [1, 1]);
  assert.deepEqual(plan(["a", "b", "c"], ["x", "y", "z"]), [0, 0]);
});

test("repeated keys never let the prefix and suffix overlap", () => {
  assert.deepEqual(plan(["a", "a"], ["a", "a", "a"]), [2, 0]);
  assert.deepEqual(plan(["a", "a", "a"], ["a", "a"]), [2, 0]);
  assert.deepEqual(plan(["a", "a", "a"], ["a"]), [1, 0]);
  assert.deepEqual(plan(["a", "b", "a"], ["a", "a"]), [1, 1]);
  assert.deepEqual(plan(["x", "a"], ["a", "a"]), [0, 1]);
});

test("the inputs are not modified", () => {
  const oldKeys = ["a", "b", "c"];
  const newKeys = ["a", "x", "c"];
  load().planPatch(oldKeys, newKeys);
  assert.deepEqual(oldKeys, ["a", "b", "c"]);
  assert.deepEqual(newKeys, ["a", "x", "c"]);
});

test("random lists: the plan rebuilds the new list with a maximal prefix", () => {
  // Small alphabet, so repeats and overlaps are common
  let seed = 1;
  const rand = (n) => {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    return seed % n;
  };
  const list = () => Array.from({ length: rand(7) }, () => "abc"[rand(3)]);
  const { planPatch } = load();
  for (let i = 0; i < 2000; i++) {
    const oldKeys = list();
    const newKeys = list();
    const { prefix, suffix } = planPatch(oldKeys, newKeys);
    const where = JSON.stringify({ oldKeys, newKeys, prefix, suffix });
    assert.ok(prefix >= 0 && suffix >= 0, where);
    assert.ok(prefix + suffix <= Math.min(oldKeys.length, newKeys.length), where);
    assert.deepEqual(applyPlan(oldKeys, newKeys, prefix, suffix), newKeys, where);
    // Maximal: the next block along either end would not match
    if (prefix < Math.min(oldKeys.length, newKeys.length)) {
      assert.notEqual(oldKeys[prefix], newKeys[prefix], where);
    }
    const room = Math.min(oldKeys.length, newKeys.length) - prefix;
    if (suffix < room) {
      assert.notEqual(
        oldKeys[oldKeys.length - 1 - suffix],
        newKeys[newKeys.length - 1 - suffix],
        where,
      );
    }
  }
});

test("cost stays linear in the list lengths", () => {
  const { planPatch } = load();
  const counted = (arr, counter) =>
    new Proxy(arr, {
      get(target, prop, receiver) {
        if (typeof prop === "string" && /^\d+$/.test(prop)) counter.reads++;
        return Reflect.get(target, prop, receiver);
      },
    });
  const n = 1000;
  const same = Array.from({ length: n }, (_, i) => `k${i}`);
  const cases = [
    [same, same],
    [same, ["x", ...same]],
    [same, [...same.slice(0, n / 2), "x", ...same.slice(n / 2)]],
    [same, same.map((k) => k + "!")],
    [Array(n).fill("a"), Array(n + 1).fill("a")],
  ];
  for (const [oldKeys, newKeys] of cases) {
    const counter = { reads: 0 };
    planPatch(counted(oldKeys, counter), counted(newKeys, counter));
    assert.ok(
      counter.reads <= 4 * (oldKeys.length + newKeys.length),
      `${counter.reads} reads for ${oldKeys.length}+${newKeys.length} keys`,
    );
  }
});

test("never reads a global BR, which a doc element can be; review hands its check over as skip", () => {
  const src = fs.readFileSync(PATCH_JS, "utf-8");
  assert.doesNotMatch(src, /\bBR\b/);
  const patch = load();
  assert.equal(typeof patch.skip, "function");
  assert.equal(patch.skip({}), false);
});
