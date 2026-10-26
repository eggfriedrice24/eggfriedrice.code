import { useLocalSearchParams } from "expo-router";

import { PairScreen } from "@/features/pairing/screens/pair-screen";

export { ErrorBoundary } from "expo-router";

// `?machine=<daemon id>` pairs that machine again.
export default function Pair() {
  const { machine } = useLocalSearchParams<{ machine?: string }>();
  return machine === undefined ? <PairScreen /> : <PairScreen machine={machine} />;
}
