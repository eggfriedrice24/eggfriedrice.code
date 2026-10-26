// A rough micro-bench of the decoder in Node: it decodes every item of the recordings
// (test/recordings/*.ndjson) many times and prints the cost per item. It is not a
// number for the iPhone: Hermes has no JIT for JS, so the Hermes bench of the dev build
// (plan section 2.3 item 12) decides. Use it to compare the modes and to see a change.
//
//   node scripts/bench-decode.mjs              all three modes, each in a fresh process
//   node scripts/bench-decode.mjs --mode=zod   one mode: zod, jitless or hand
//
// Run it alone and at nice 19: the numbers of a busy machine are not valid.
//
// The modes: `zod` is the decoder of the app (src/lib/protocol/envelope.ts) with zod's
// compiled object parsers, `jitless` is the same with `z.config({ jitless: true })`, and
// `hand` is the hand-written guard from before the schemas (hand-guard.ts). The script
// reads the TypeScript sources with the type stripping of Node 22.18 or later.

import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { performance } from "node:perf_hooks";
import { fileURLToPath, pathToFileURL } from "node:url";

const APP = fileURLToPath(new URL("..", import.meta.url));
const MODES = ["zod", "jitless", "hand"];
const ROUNDS = 200;

const mode = process.argv.find((arg) => arg.startsWith("--mode="))?.slice("--mode=".length);

if (mode === undefined) {
  for (const each of MODES) {
    // The sources are TypeScript files in a package with no "type", so Node warns that
    // it reads them as ES modules. That is what they are.
    const args = [
      "--disable-warning=MODULE_TYPELESS_PACKAGE_JSON",
      fileURLToPath(import.meta.url),
      `--mode=${each}`,
    ];
    const output = execFileSync(process.execPath, args, { encoding: "utf8" });
    process.stdout.write(output);
  }
  process.exit(0);
}
if (!MODES.includes(mode)) {
  throw new Error(`the mode ${mode} is not one of ${MODES.join(", ")}`);
}

// The sources import "./x" for x.ts and "@/x" for src/x.ts, as Metro and Vitest resolve
// them. Node needs the file name.
registerHooks({
  resolve(specifier, context, nextResolve) {
    const parent = context.parentURL ?? "";
    if (specifier.startsWith("@/")) {
      return nextResolve(pathToFileURL(`${APP}src/${specifier.slice(2)}.ts`).href, context);
    }
    if (specifier.startsWith(".") && parent.includes("/src/") && !/\.[cm]?[jt]s$/.test(specifier)) {
      return nextResolve(new URL(`${specifier}.ts`, parent).href, context);
    }
    return nextResolve(specifier, context);
  },
});

if (mode === "jitless") {
  const { z } = await import("zod");
  // Before the schemas exist: zod reads the setting when it makes each object schema.
  z.config({ jitless: true });
}

const loadStart = performance.now();
const decoder =
  mode === "hand"
    ? await import(pathToFileURL(`${APP}src/lib/protocol/hand-guard.ts`).href)
    : await import(pathToFileURL(`${APP}src/lib/protocol/envelope.ts`).href);
const loadMs = performance.now() - loadStart;

const recordings = `${APP}test/recordings/`;
/** @type {{ kind: string, raw: unknown }[]} */
const items = [];
for (const name of readdirSync(recordings)
  .filter((file) => file.endsWith(".ndjson"))
  .sort()) {
  for (const line of readFileSync(`${recordings}${name}`, "utf8").split("\n")) {
    if (line.trim() !== "") {
      const raw = JSON.parse(line);
      items.push({ kind: kindOf(raw), raw });
    }
  }
}

/**
 * The kind of an item, with the kind of its event or draft part.
 *
 * @param {any} raw
 * @returns {string}
 */
function kindOf(raw) {
  if (raw?.kind === "event") {
    return `event ${raw.event?.kind}`;
  }
  if (raw?.kind === "draft") {
    return `draft ${raw.draft?.kind}`;
  }
  return String(raw?.kind);
}

/**
 * Decodes one item and fails on an error, so that a broken decoder cannot look fast.
 *
 * @param {unknown} raw
 */
function decode(raw) {
  const decoded = decoder.decodeSubscribeItem(raw);
  if (!decoded.ok) {
    throw new Error(`the ${mode} decoder refused an item: ${decoded.reason}`);
  }
}

const firstStart = performance.now();
decode(items.find((item) => item.kind.startsWith("draft"))?.raw);
const firstMs = performance.now() - firstStart;

/** @type {Map<string, number[]>} */
const times = new Map();
for (let round = 0; round < ROUNDS; round += 1) {
  for (const item of items) {
    const start = performance.now();
    decode(item.raw);
    const took = performance.now() - start;
    const group = item.kind.startsWith("draft") ? "draft (all parts)" : item.kind;
    for (const key of [group, "all items"]) {
      const list = times.get(key) ?? [];
      list.push(took);
      times.set(key, list);
    }
  }
}

/**
 * The value at fraction `p` of the sorted list.
 *
 * @param {number[]} sorted
 * @param {number} p
 * @returns {number}
 */
function quantile(sorted, p) {
  return sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] ?? Number.NaN;
}

/** @param {number} ms */
const us = (ms) => `${(ms * 1000).toFixed(1)} us`;

console.log(`mode ${mode}: ${items.length} items x ${ROUNDS} rounds, node ${process.version}`);
console.log(`  load of the decoder module: ${loadMs.toFixed(1)} ms; first draft: ${us(firstMs)}`);
const rows = [...times.entries()].sort(([a], [b]) => a.localeCompare(b));
for (const [key, list] of rows) {
  const sorted = [...list].sort((a, b) => a - b);
  const mean = list.reduce((sum, value) => sum + value, 0) / list.length;
  console.log(
    `  ${key.padEnd(36)} n ${String(list.length).padStart(7)}  mean ${us(mean).padStart(9)}  p50 ${us(quantile(sorted, 0.5)).padStart(9)}  p95 ${us(quantile(sorted, 0.95)).padStart(9)}`,
  );
}
