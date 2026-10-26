// Writes src/lib/protocol/generated.ts and src/lib/protocol/generated-zod.ts from
// docs/protocol.schema.json (`pnpm gen:protocol`). With `--check` it writes nothing and
// fails when a file is not what it would write (`pnpm check:protocol`, a gate of
// `just app-check` and of the CI app job).

import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { generate, generateZod } from "./protocol-codegen.mjs";

const SCHEMA = fileURLToPath(new URL("../../docs/protocol.schema.json", import.meta.url));
const OUTPUTS = [
  { name: "src/lib/protocol/generated.ts", write: generate },
  { name: "src/lib/protocol/generated-zod.ts", write: generateZod },
];

const check = process.argv.includes("--check");
const schema = await readFile(SCHEMA, "utf8");

/**
 * Writes one output, or with `--check` tells whether it is stale.
 *
 * @param {{ name: string, write: (schema: string) => Promise<string> }} output
 * @returns {Promise<boolean>} true when the output is stale
 */
async function run(output) {
  const path = fileURLToPath(new URL(`../${output.name}`, import.meta.url));
  const text = await output.write(schema);
  if (!check) {
    await writeFile(path, text);
    return false;
  }
  let current = "";
  try {
    current = await readFile(path, "utf8");
  } catch {
    // A missing file is stale too.
  }
  return current !== text;
}

const results = await Promise.all(OUTPUTS.map((output) => run(output)));
const stale = OUTPUTS.filter((_, index) => results[index]).map((output) => output.name);

if (stale.length > 0) {
  console.error(
    `${stale.join(" and ")} do not match docs/protocol.schema.json: run \`pnpm gen:protocol\` in app/ and commit the result`,
  );
  process.exit(1);
}
