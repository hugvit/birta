import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ASSETS = path.join(HERE, "..", "..", "assets", "review");
const SHARED_BR = "var BR = BR && BR.__birta === true ? BR : { __birta: true };";
const OURS = ["10-keys.js", "20-pointing.js"];
const ORDER = ["quote.js", "00-core.js", "10-keys.js", "20-pointing.js", "40-lane.js"];

/** The server's default bindings, the viewer's and review's together. */
const KB = {
  toggle_reading: "r",
  exit_reading: "Escape",
  toggle_dark: "d",
  focus_theme: "t",
  toggle_raw: "u",
  focus_next: "j",
  focus_prev: "k",
  note: "c",
  ask: "q",
  yes: "y",
  no: "x",
  later: "l",
  your_call: ".",
  next_item: "n",
  prev_item: "p",
  next_file: "Shift+J",
  prev_file: "Shift+K",
  undo: "z",
  hide_marks: "m",
  open: "Enter",
  turn: "Mod+Enter",
};

function load(files = ORDER) {
  const ctx = vm.createContext({});
  for (const f of files) {
    const file = path.join(ASSETS, f);
    vm.runInContext(fs.readFileSync(file, "utf-8"), ctx, { filename: file });
  }
  return ctx;
}

for (const f of OURS) {
  test(`${f} starts by reusing the shared BR namespace`, () => {
    assert.equal(fs.readFileSync(path.join(ASSETS, f), "utf-8").split("\n")[0], SHARED_BR);
  });
}

for (const [name, files] of [
  ["in bundle order", ORDER],
  ["in reverse order", [...ORDER].reverse()],
]) {
  test(`the keys and pointing load without a DOM, ${name}`, () => {
    const { BR } = load(files);
    assert.equal(typeof BR.pure.keyAction, "function");
    assert.equal(typeof BR.boot, "function");
    assert.equal(typeof BR.pure.laneMarks, "function");
  });
}

// --- keyAction: which review action a keydown asks for, before any DOM target check ----------

/** `[event, bindings?, action]`: the event is a plain keydown with only these fields set. */
const CASES = [
  // Plain letters, and Caps Lock letters, which arrive upper case without Shift
  [{ key: "j" }, "focus_next"],
  [{ key: "J" }, "focus_next"],
  [{ key: "k" }, "focus_prev"],
  [{ key: "c" }, "note"],
  [{ key: "q" }, "ask"],
  [{ key: "y" }, "yes"],
  [{ key: "x" }, "no"],
  [{ key: "X" }, "no"],
  [{ key: "l" }, "later"],
  [{ key: "." }, "your_call"],
  [{ key: "n" }, "next_item"],
  [{ key: "p" }, "prev_item"],
  [{ key: "z" }, "undo"],
  [{ key: "m" }, "hide_marks"],
  [{ key: "M" }, "hide_marks"],
  [{ key: "Enter" }, "open"],
  // Shift is part of a letter binding
  [{ key: "J", shiftKey: true }, "next_file"],
  [{ key: "K", shiftKey: true }, "prev_file"],
  [{ key: "M", shiftKey: true }, null],
  [{ key: "Y", shiftKey: true }, null],
  // A punctuation key is its character, whatever Shift took to type it (AZERTY `.`)
  [{ key: ".", shiftKey: true }, "your_call"],
  // Option on macOS types another glyph, and any held modifier leaves the key to the browser
  [{ key: "j", altKey: true }, null],
  [{ key: "∆", code: "KeyJ", altKey: true }, null],
  [{ key: "j", ctrlKey: true }, null],
  [{ key: "j", metaKey: true }, null],
  [{ key: "m", metaKey: true }, null],
  [{ key: "c", metaKey: true }, null],
  [{ key: "x", ctrlKey: true }, null],
  // Mod is Meta or Ctrl, for the one binding that takes a modifier
  [{ key: "Enter", metaKey: true }, "turn"],
  [{ key: "Enter", ctrlKey: true }, "turn"],
  [{ key: "Enter", altKey: true }, null],
  [{ key: "Enter", shiftKey: true }, null],
  [{ key: "Enter", metaKey: true, shiftKey: true }, null],
  // IME composition, in both of the ways engines report it
  [{ key: "x", isComposing: true }, null],
  [{ key: "j", isComposing: true }, null],
  [{ key: "x", keyCode: 229 }, null],
  [{ key: "Process", keyCode: 229 }, null],
  [{ key: "Dead" }, null],
  // A held verdict key sends once; a held move key keeps moving
  [{ key: "y", repeat: true }, null],
  [{ key: "x", repeat: true }, null],
  [{ key: "l", repeat: true }, null],
  [{ key: ".", repeat: true }, null],
  [{ key: "j", repeat: true }, "focus_next"],
  [{ key: "k", repeat: true }, "focus_prev"],
  // The typed character names the key, never the physical one (Dvorak, AZERTY)
  [{ key: "j", code: "KeyC" }, "focus_next"],
  [{ key: "c", code: "KeyJ" }, "note"],
  [{ key: "a", code: "KeyQ" }, null],
  // Caps Lock with Shift can arrive lower case; a binding written upper case is one letter
  [{ key: "j", shiftKey: true }, "next_file"],
  [{ key: "g" }, { ...KB, yes: "G" }, "yes"],
  [{ key: "Unidentified" }, null],
  // A held undo, hand-back or marks toggle acts once
  [{ key: "Enter", metaKey: true, repeat: true }, null],
  [{ key: "Enter", ctrlKey: true, repeat: true }, null],
  [{ key: "z", repeat: true }, null],
  [{ key: "m", repeat: true }, null],
  // The viewer's own keys are not review actions
  [{ key: "r" }, null],
  [{ key: "Escape" }, null],
  // Rebinding moves an action, and an empty binding switches it off
  [{ key: "g" }, { ...KB, yes: "g" }, "yes"],
  [{ key: "y" }, { ...KB, yes: "g" }, null],
  [{ key: "y" }, { ...KB, yes: "" }, null],
  [{ key: "Enter", ctrlKey: true }, { ...KB, turn: "Ctrl+Enter" }, "turn"],
  [{ key: "Enter", metaKey: true }, { ...KB, turn: "Ctrl+Enter" }, null],
  // An older server's bindings lack the review actions
  [{ key: "j" }, { toggle_reading: "r" }, null],
];

const EVENT_DEFAULTS = {
  code: "",
  shiftKey: false,
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  repeat: false,
  isComposing: false,
  keyCode: 0,
};

test("keyAction maps each keydown to its review action", () => {
  const { BR } = load();
  const wrong = [];
  for (const c of CASES) {
    const [init, kb, want] = c.length === 2 ? [c[0], KB, c[1]] : c;
    const got = BR.pure.keyAction({ ...EVENT_DEFAULTS, ...init }, kb);
    if ((got ?? null) !== want) wrong.push(`${JSON.stringify(init)} -> ${got} (want ${want})`);
  }
  assert.deepEqual(wrong, []);
});

test("keyAction reads the event and bindings without changing them", () => {
  const { BR } = load();
  const e = Object.freeze({ ...EVENT_DEFAULTS, key: "J" });
  const kb = Object.freeze({ ...KB });
  assert.equal(BR.pure.keyAction(e, kb), "focus_next");
});
