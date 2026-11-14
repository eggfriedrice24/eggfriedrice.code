import { Redirect } from "expo-router";
import type { ComponentType } from "react";

export { ErrorBoundary } from "expo-router";

// The decode bench of the dev builds (plan section 2.3, item 12). A build without the dev
// tools leaves the bench and the copy of the recordings out of its bundle, as the gallery
// route tells.
const Screen: ComponentType | undefined =
  (typeof __DEV__ === "boolean" && __DEV__) || process.env["EXPO_PUBLIC_EFR_DEV_TOOLS"] === "1"
    ? (() => {
        const screen: typeof import("@/features/demo/screens/bench-screen") = require("@/features/demo/screens/bench-screen");
        return screen.BenchScreen;
      })()
    : undefined;

export default function Bench() {
  return Screen === undefined ? <Redirect href="/" /> : <Screen />;
}
