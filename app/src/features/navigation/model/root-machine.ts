// The machine of the blank root (design section 1): the machine that the user picked last
// in the machine menu, else the first paired machine. With no machine paired, the root is
// the pairing intro. Pure.

import type { Machine } from "@/features/machines/model/machine";
import type { MachineId } from "@/features/machines/model/machine-ref";
import type { RegistryState } from "@/features/machines/model/machine-registry";

/** The machine of the blank root, or undefined when no machine is paired. */
export function rootMachine(
  registry: RegistryState,
  lastMachine: MachineId | null,
): Machine | undefined {
  const last = registry.machines.find((machine) => machine.id === lastMachine);
  return last ?? registry.machines[0];
}
