// Writes src/lib/protocol/generated.ts from docs/protocol.schema.json (`pnpm gen:protocol`).
// With `--check` it writes nothing and fails when the file is not what it would write
// (`pnpm check:protocol`, gate 6 of `just app-check` and of the CI app job).

import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { generate } from "./protocol-codegen.mjs";

const SCHEMA = fileURLToPath(new URL("../../docs/protocol.schema.json", import.meta.url));
const OUTPUT = fileURLToPath(new URL("../src/lib/protocol/generated.ts", import.meta.url));

const check = process.argv.includes("--check");
const text = await generate(await readFile(SCHEMA, "utf8"));

if (check) {
  let current = "";
  try {
    current = await readFile(OUTPUT, "utf8");
  } catch {
    // A missing file is stale too.
  }
  if (current !== text) {
    console.error(
      "src/lib/protocol/generated.ts does not match docs/protocol.schema.json: run `pnpm gen:protocol` in app/ and commit the result",
    );
    process.exit(1);
  }
} else {
  await writeFile(OUTPUT, text);
}
