// Writes src/theme/theme.css, the uniwind theme, from the roles, the type scale and the
// metrics of src/theme (`pnpm gen:theme`). With `--check` it writes nothing and fails
// when the file is not what it would write (`pnpm check:theme`).
//
// Node strips the types of the theme files. Their imports have no extension, as the app
// and TypeScript want them, so a resolve hook adds `.ts` to a relative import of a
// TypeScript file. Run it with `node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON`,
// because package.json does not set "type".

import { readFile, writeFile } from "node:fs/promises";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";

registerHooks({
  resolve(specifier, context, nextResolve) {
    const fromTypeScript = context.parentURL?.endsWith(".ts") === true;
    const relative = specifier.startsWith("./") || specifier.startsWith("../");
    if (fromTypeScript && relative && !/\.[cm]?[jt]s$/.test(specifier)) {
      return nextResolve(`${specifier}.ts`, context);
    }
    return nextResolve(specifier, context);
  },
});

const { themeCss } = await import("../src/theme/theme-css.ts");

const OUTPUT = fileURLToPath(new URL("../src/theme/theme.css", import.meta.url));

const check = process.argv.includes("--check");
const text = themeCss();

if (check) {
  let current = "";
  try {
    current = await readFile(OUTPUT, "utf8");
  } catch {
    // A missing file is stale too.
  }
  if (current !== text) {
    console.error(
      "src/theme/theme.css does not match src/theme: run `pnpm gen:theme` in app/ and commit the result",
    );
    process.exit(1);
  }
} else {
  await writeFile(OUTPUT, text);
}
