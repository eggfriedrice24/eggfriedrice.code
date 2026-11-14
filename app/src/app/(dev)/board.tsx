import { Redirect, useLocalSearchParams } from "expo-router";
import type { ComponentType } from "react";

import { boardParams, type BoardParams } from "@/features/demo/model/board-schemas";

export { ErrorBoundary } from "expo-router";

// One component board of the dev gallery: `?name=<board>`. A build without the dev tools
// leaves the boards out of its bundle, as the gallery route tells.
const Screen: ComponentType<BoardParams> | undefined =
  (typeof __DEV__ === "boolean" && __DEV__) || process.env["EXPO_PUBLIC_EFR_DEV_TOOLS"] === "1"
    ? (() => {
        const screen: typeof import("@/features/demo/screens/board-screen") = require("@/features/demo/screens/board-screen");
        return screen.BoardScreen;
      })()
    : undefined;

export default function Board() {
  const params = boardParams(useLocalSearchParams());
  return Screen === undefined ? <Redirect href="/" /> : <Screen {...params} />;
}
