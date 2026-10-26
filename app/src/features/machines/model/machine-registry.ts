// The registry of the paired machines: the list that the machine menu, Settings and the
// root read. The runtime keeps it in a TanStack Store atom; these functions make each new
// state. Pure, for the tests.

import type { Machine } from "./machine";
import type { MachineId } from "./machine-ref";

/** The paired machines. */
export interface RegistryState {
  /** False until the runtime read the pairing records. */
  readonly loaded: boolean;
  /** The machines, in the order of their first pairing. */
  readonly machines: readonly Machine[];
}

/** The registry before the runtime read the records. */
export const LOADING_REGISTRY: RegistryState = { loaded: false, machines: [] };

/** The registry once the records are read. */
export function loadedRegistry(machines: readonly Machine[]): RegistryState {
  return { loaded: true, machines };
}

/** `state` with `machine` in place of the machine with the same id, or added at the end. */
export function withMachine(state: RegistryState, machine: Machine): RegistryState {
  let replaced = false;
  const machines = state.machines.map((old) => {
    if (old.id !== machine.id) {
      return old;
    }
    replaced = true;
    return machine;
  });
  if (!replaced) {
    machines.push(machine);
  }
  return { loaded: state.loaded, machines };
}

/** `state` without machine `id`. */
export function withoutMachine(state: RegistryState, id: MachineId): RegistryState {
  return { loaded: state.loaded, machines: state.machines.filter((machine) => machine.id !== id) };
}

/** The machine `id` of `state`, or undefined. */
export function findMachine(state: RegistryState, id: MachineId): Machine | undefined {
  return state.machines.find((machine) => machine.id === id);
}
