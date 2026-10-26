import { useLocalSearchParams } from "expo-router";

import { boardParams } from "@/features/demo/model/board-schemas";
import { BoardScreen } from "@/features/demo/screens/board-screen";

export { ErrorBoundary } from "expo-router";

// One component board of the dev gallery: `?name=<board>`.
export default function Board() {
  return <BoardScreen {...boardParams(useLocalSearchParams())} />;
}
