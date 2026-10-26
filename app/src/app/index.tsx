import { BlankRootScreen } from "@/features/conversation/screens/blank-root-screen";
import { NoMachineRootScreen } from "@/features/pairing/screens/no-machine-root-screen";

export { ErrorBoundary } from "expo-router";

// The blank root, or the pairing intro while no machine is paired.
export default function Index() {
  return <BlankRootScreen noMachine={<NoMachineRootScreen />} />;
}
