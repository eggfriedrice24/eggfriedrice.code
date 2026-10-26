// Settings over a sandbox of the gallery (native/sandboxes.ts): the real Settings screen
// with only the demo machines of the board, for the boards that the app's own runtime
// cannot show, such as Forget the last machine.

import { QueryClientProvider } from "@tanstack/react-query";
import { Suspense, use, useState } from "react";

import { AppServicesContext } from "@/features/navigation/use-app-services";
import { SettingsScreen } from "@/features/settings/screens/settings-screen";
import type { DemoMachineKey } from "../model/demo-machines";
import type { Sandbox } from "../model/demo-services";
import { sandboxFor } from "../native/sandboxes";

export interface SandboxSettingsProps {
  /** The board, which keys the sandbox. */
  readonly board: string;
  /** The visit of the gallery that opened the board. */
  readonly visit: string;
  readonly machines: readonly DemoMachineKey[];
}

/** Settings with the demo machines `machines` only. */
export function SandboxSettings(props: SandboxSettingsProps) {
  const [sandbox] = useState(() => sandboxFor(props.board, props.visit, props.machines));
  return (
    <Suspense fallback={null}>
      <LoadedSettings sandbox={sandbox} />
    </Suspense>
  );
}

function LoadedSettings(props: { readonly sandbox: Promise<Sandbox> }) {
  const { services } = use(props.sandbox);
  return (
    <QueryClientProvider client={services.queryClient}>
      <AppServicesContext value={services}>
        <SettingsScreen />
      </AppServicesContext>
    </QueryClientProvider>
  );
}
