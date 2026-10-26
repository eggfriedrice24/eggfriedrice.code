import { useLocalSearchParams } from "expo-router";

import { pairRouteMachine } from "@/features/pairing/model/pair-route-schemas";
import { PairScreen } from "@/features/pairing/screens/pair-screen";

export { ErrorBoundary } from "expo-router";

// `?machine=<daemon id>` pairs that machine again.
export default function Pair() {
  return <PairScreen machine={pairRouteMachine(useLocalSearchParams())} />;
}
