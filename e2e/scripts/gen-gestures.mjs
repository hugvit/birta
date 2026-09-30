// Regenerates the browser-derived anchor oracles that `cargo test` reads:
// tests/fixtures/anchor_gestures.json and tests/fixtures/anchor_goldens/<fixture>.txt.
// Runs selection.spec.ts in Chromium and WebKit in record mode, and writes nothing
// unless both pass and agree on every owner's text.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const E2E = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const FIXTURES = path.resolve(E2E, "..", "tests", "fixtures");
const ENGINES = ["chromium", "webkit"];

const out = fs.mkdtempSync(path.join(os.tmpdir(), "birta-gestures-"));
const run = spawnSync(
  "npx",
  ["playwright", "test", "tests/selection.spec.ts", ...ENGINES.map((e) => `--project=${e}`)],
  { cwd: E2E, stdio: "inherit", env: { ...process.env, BIRTA_GESTURES_OUT: out } },
);
if (run.status !== 0) {
  console.error("selection.spec.ts failed in record mode; nothing written");
  process.exit(1);
}

const read = (name) => fs.readFileSync(path.join(out, name), "utf-8");
const names = fs.readdirSync(FIXTURES).filter((n) => n.endsWith(".md")).sort();
const goldens = {};
for (const name of names) {
  const [a, b] = ENGINES.map((e) => read(`${e}.golden.${name}.txt`));
  if (a !== b) {
    const [la, lb] = [a.split("\n"), b.split("\n")];
    for (let i = 0; i < Math.max(la.length, lb.length); i++) {
      if (la[i] !== lb[i]) console.error(`${name}\n  chromium: ${la[i]}\n  webkit:   ${lb[i]}`);
    }
    console.error("the engines disagree on owner text; nothing written");
    process.exit(1);
  }
  goldens[name] = a;
}

const engines = Object.fromEntries(ENGINES.map((e) => [e, JSON.parse(read(`${e}.json`))]));
fs.writeFileSync(
  path.join(FIXTURES, "anchor_gestures.json"),
  JSON.stringify({ fixture: "anchor.md", engines }, null, 2) + "\n",
);
const dir = path.join(FIXTURES, "anchor_goldens");
fs.rmSync(dir, { recursive: true, force: true });
fs.mkdirSync(dir);
for (const [name, text] of Object.entries(goldens)) fs.writeFileSync(path.join(dir, `${name}.txt`), text);
fs.rmSync(out, { recursive: true, force: true });
console.log(`wrote tests/fixtures/anchor_gestures.json and ${names.length} goldens in tests/fixtures/anchor_goldens/`);
