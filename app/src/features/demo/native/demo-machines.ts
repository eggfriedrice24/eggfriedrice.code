// The demo server of the app (model/demo-server.ts) on the real clock, and the pairing
// effects of the root layout with the demo machines on it. A build without the dev tools
// (lib/dev/dev-tools.ts) gets its pairing effects unchanged, so no socket of it can reach
// the demo server.

import { DEV_TOOLS } from "@/lib/dev/dev-tools";
import { systemClock } from "@/lib/native/clock";
import type { PairingDeps } from "@/features/pairing/model/pair";
import { DemoServer } from "../model/demo-server";
import { withDemoMachines } from "../model/demo-services";

/** The one demo server of the app. */
export const demoServer = new DemoServer({ clock: systemClock });

/** `pairing` with the demo machines, in a build with the dev tools. */
export function withDevMachines(pairing: PairingDeps | null): PairingDeps | null {
  return pairing === null || !DEV_TOOLS ? pairing : withDemoMachines(pairing, demoServer);
}
