import path from "node:path";
import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

// Vitest runs the tests of the pure code (`*.test.ts` in src/ and test/) in Node. A
// test imports nothing from React Native. The component tests (`*.test.tsx`) run in
// jest-expo instead. `@/` is the alias of src/, as in tsconfig.json and Metro.
export default defineConfig({
  resolve: {
    alias: { "@": path.join(path.dirname(fileURLToPath(import.meta.url)), "src") },
  },
  test: {
    include: ["src/**/*.test.ts", "test/**/*.test.ts"],
    environment: "node",
  },
});
