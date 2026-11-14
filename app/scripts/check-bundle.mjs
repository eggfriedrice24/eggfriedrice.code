// Fails when the iOS bundle of `expo export` (in dist/) holds the dev tools: the copy of
// the recordings (src/lib/dev/recordings.ts) or the boards of the dev gallery
// (src/features/demo). `pnpm bundle:check` runs it after the export, which builds
// without EXPO_PUBLIC_EFR_DEV_TOOLS, as a store build does.
//
// Hermes keeps an ASCII string of the code as plain text in its bytecode, so a plain
// search finds it. The markers are the first line of each recording (only the copy holds
// it) and two section titles of the gallery (src/features/demo/model/boards.ts).

import { readFile, readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const BUNDLES = fileURLToPath(new URL("../dist/_expo/static/js/ios/", import.meta.url));
const RECORDINGS = fileURLToPath(new URL("../test/recordings/", import.meta.url));

const GALLERY = ["Undrawn states (item 8.19)", "Component sheets"];

const names = (await readdir(RECORDINGS)).filter((name) => name.endsWith(".ndjson"));
const texts = await Promise.all(names.map((name) => readFile(RECORDINGS + name, "utf8")));
const lines = [];
for (const [at, text] of texts.entries()) {
  const first = text.split("\n", 1)[0] ?? "";
  // NOTE: Hermes keeps a string with a non-ASCII character as UTF-16.
  if (first.length > 0 && /^[\x20-\x7e]*$/.test(first)) {
    lines.push([`the first line of ${names[at]}`, first]);
  }
}
const files = (await readdir(BUNDLES)).filter((name) => /\.(hbc|js)$/.test(name));
if (files.length === 0 || lines.length === 0) {
  console.error(`check-bundle: no bundle in ${BUNDLES} or no recording in ${RECORDINGS}`);
  process.exit(1);
}

const markers = [...lines, ...GALLERY.map((title) => [`the gallery title "${title}"`, title])];
const bundles = await Promise.all(files.map((file) => readFile(BUNDLES + file)));
let found = 0;
for (const [at, bundle] of bundles.entries()) {
  const file = files[at];
  const text = bundle.toString("latin1");
  for (const [what, marker] of markers) {
    if (text.includes(marker)) {
      console.error(`check-bundle: ${file} holds ${what}, a part of the dev tools`);
      found += 1;
    }
  }
}
if (found > 0) {
  process.exit(1);
}
console.log(`check-bundle: ${files.length} bundle(s) hold none of the dev tools`);
