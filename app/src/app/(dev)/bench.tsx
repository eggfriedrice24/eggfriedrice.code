import { BenchScreen } from "@/features/demo/screens/bench-screen";

export { ErrorBoundary } from "expo-router";

// The decode bench of the dev builds (plan section 2.3, item 12).
export default function Bench() {
  return <BenchScreen />;
}
