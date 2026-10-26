// The machines as the header reads them: the paired machines, the machine of the blank
// root, and the connection of each machine. Each hook selects only what it returns, so a
// change of one connection renders only the views that show it.

import { useSelector } from "@tanstack/react-store";

import type { Machine } from "@/features/machines/model/machine";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { MachineId } from "@/features/machines/model/machine-ref";
import {
  machineConnections,
  sameConnections,
  type MachineConnections,
} from "./model/machine-connections";
import { rootMachine } from "./model/root-machine";
import { usePreferences, useRuntime } from "./use-app-services";

/** The paired machines, in the order of their first pairing. */
export function useMachines(): readonly Machine[] {
  const runtime = useRuntime();
  return useSelector(runtime.registry, (state) => state.machines);
}

/** The machine of the blank root, or undefined when no machine is paired. */
export function useRootMachine(): Machine | undefined {
  const runtime = useRuntime();
  const preferences = usePreferences();
  const registry = useSelector(runtime.registry);
  const lastMachine = useSelector(preferences.store, (state) => state.lastMachine);
  return rootMachine(registry, lastMachine);
}

/** The connection of every paired machine. */
export function useMachineConnections(): MachineConnections {
  const runtime = useRuntime();
  return useSelector(machineConnections(runtime), (connections) => connections, {
    compare: sameConnections,
  });
}

/** The connection of machine `machine`, or undefined when it is not paired. */
export function useMachineConnection(machine: MachineId): MachineConnection | undefined {
  const runtime = useRuntime();
  return useSelector(machineConnections(runtime), (connections) => connections.get(machine));
}
