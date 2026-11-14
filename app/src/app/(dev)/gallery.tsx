import { Redirect } from "expo-router";
import type { ComponentType } from "react";

export { ErrorBoundary } from "expo-router";

// The dev gallery of every design board. Settings opens it in a dev or preview build.
//
// The test is the one of lib/dev/dev-tools.ts, written here in full: Metro folds it when
// it builds without the dev tools, and then drops the require. So the gallery and the
// demo stay out of that bundle (`pnpm bundle:check` fails when they are in it).
const Screen: ComponentType | undefined =
  (typeof __DEV__ === "boolean" && __DEV__) || process.env["EXPO_PUBLIC_EFR_DEV_TOOLS"] === "1"
    ? (() => {
        const screen: typeof import("@/features/demo/screens/gallery-screen") = require("@/features/demo/screens/gallery-screen");
        return screen.GalleryScreen;
      })()
    : undefined;

export default function Gallery() {
  return Screen === undefined ? <Redirect href="/" /> : <Screen />;
}
