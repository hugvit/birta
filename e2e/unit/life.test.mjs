import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FILE = path.join(HERE, "..", "..", "assets", "review", "80-life.js");
const ctx = vm.createContext({});
vm.runInContext(fs.readFileSync(FILE, "utf-8"), ctx, { filename: FILE });
const { pure } = ctx.BR;

const RESTART = "BIRTA_REVIEW=1 birta '/home/u/a b.md'";

test("the banner names the restart command as the server gave it", () => {
  assert.equal(pure.stoppedText(RESTART), "birta stopped · run BIRTA_REVIEW=1 birta '/home/u/a b.md' to reconnect");
});

test("the copied command restarts without opening a second tab", () => {
  assert.equal(pure.restartLine(RESTART), "BIRTA_REVIEW=1 birta '/home/u/a b.md' --no-open");
});
