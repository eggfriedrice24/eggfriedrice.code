import { defineConfig } from "vitest/config";

// The tests cover the pure core of the app (src/core). They run in Node and import
// nothing from React Native.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts", "test/**/*.test.ts"],
    environment: "node",
  },
});
