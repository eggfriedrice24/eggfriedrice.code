import { defineConfig } from "vitest/config";

// The tests cover the pure code of the app (src/core, src/protocol, src/net and
// src/store, but not their native/ adapters) and the code generator. They run in Node
// and import nothing from React Native.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts", "test/**/*.test.ts"],
    environment: "node",
  },
});
