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

// --- Back to the tab: what changed while it was away -------------------------------------------

/** A state at `stage`, with Claude's reply on `turn` and `toCheck` stops left, as the fold gives it. */
function at(stage, { turn = null, toCheck = 0, claim = null } = {}) {
  const phrase = { stage };
  if (stage !== "drafts" && stage !== "idle") phrase.sent = { n: 7, ts: 1, items: 1, can_withdraw: stage === "sent" };
  if (claim != null || turn != null) phrase.picked_up = { n: claim ?? 8, ts: 2, by: "claude" };
  if (turn != null) phrase.replied = { turn, ts: 3, by: "claude" };
  return { phrase, counts: { to_check: toCheck } };
}

test("a reply that came while the tab was away is named with its count and where n starts", () => {
  assert.equal(
    pure.backText(at("picked_up", { claim: 8 }), at("replied", { turn: 8, toCheck: 9 }), "n"),
    "Claude replied · 9 to check · n starts at the top",
  );
  // From the hand-back straight to the reply, the pickup unseen
  assert.equal(pure.backText(at("sent"), at("replied", { turn: 8, toCheck: 2 }), "n"), "Claude replied · 2 to check · n starts at the top");
});

test("the walk key is the one the reader bound", () => {
  assert.equal(pure.backText(at("sent"), at("replied", { turn: 8, toCheck: 1 }), "J"), "Claude replied · 1 to check · J starts at the top");
});

test("a pickup while away, with no reply yet, says it is still with Claude", () => {
  assert.equal(pure.backText(at("sent"), at("picked_up", { claim: 8 }), "n"), "still with Claude");
});

test("nothing changed while away says nothing", () => {
  assert.equal(pure.backText(at("sent"), at("sent"), "n"), null);
  assert.equal(pure.backText(at("picked_up", { claim: 8 }), at("picked_up", { claim: 8 }), "n"), null);
  assert.equal(pure.backText(at("replied", { turn: 8, toCheck: 9 }), at("replied", { turn: 8, toCheck: 9 }), "n"), null);
  assert.equal(pure.backText(at("drafts"), at("drafts"), "n"), null);
});

test("a reply the tab already showed is no news on the way back, even when its count moved", () => {
  // Another tab checked some of it: the same reply, fewer to check
  assert.equal(pure.backText(at("replied", { turn: 8, toCheck: 9 }), at("replied", { turn: 8, toCheck: 4 }), "n"), null);
});

test("a second reply while away is news again", () => {
  assert.equal(
    pure.backText(at("replied", { turn: 8, toCheck: 0 }), at("replied", { turn: 20, toCheck: 2 }), "n"),
    "Claude replied · 2 to check · n starts at the top",
  );
});

test("with no state on either side there is nothing to say", () => {
  assert.equal(pure.backText(null, null, "n"), null);
  assert.equal(pure.backText(null, at("sent"), "n"), null);
});
