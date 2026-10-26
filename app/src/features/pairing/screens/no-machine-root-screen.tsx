// The root when no machine is paired (design 3.18, R-NoMachine). The root route renders it
// in place of the blank root while the registry has no machine; it shows again after
// Forget removes the last machine. The header has Settings only.
//
// "Scan code" opens the system scanner here. When it finds a code, or when it cannot
// scan, the pairing modal opens and shows the rest of the session.

import { Stack, useRouter } from "expo-router";

import { NoMachineRoot } from "../components/no-machine-root";
import { usePairingSession } from "../use-pairing";

/** The no-machine root. */
export function NoMachineRootScreen() {
  const session = usePairingSession();
  const router = useRouter();
  const scan = async (): Promise<void> => {
    const outcome = await session.scan(null);
    if (outcome === "pairing" || outcome === "failed") {
      router.push("/pair");
    }
  };
  return (
    <>
      <Stack.Screen options={{ title: "efr", headerTitle: "" }} />
      <Stack.Toolbar placement="right">
        <Stack.Toolbar.Button
          icon="gearshape"
          accessibilityLabel="Settings"
          onPress={() => router.push("/settings")}
        />
      </Stack.Toolbar>
      <NoMachineRoot onScan={() => void scan()} />
    </>
  );
}
