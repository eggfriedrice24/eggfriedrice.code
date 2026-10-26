// The connections of all paired machines as one store, for the header: the subtitles of
// the machine menu, the offline line and which recent lists count for approvals
// elsewhere. Each supervisor keeps its connection in an atom; this derived atom reads the
// registry and each of those atoms, so it changes when a machine comes or goes and when a
// connection changes. Pure.

import { createAtom, type Readable } from "@tanstack/store";

import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { MachineId } from "@/features/machines/model/machine-ref";
import type { Runtime } from "@/features/machines/model/runtime";

/** The connection of each paired machine, by daemon id. */
export type MachineConnections = ReadonlyMap<MachineId, MachineConnection>;

/** The parts of the runtime that the store reads. */
export type ConnectionSource = Pick<Runtime, "registry" | "machine">;

const STORES = new WeakMap<ConnectionSource, Readable<MachineConnections>>();

/** The store of the connections of the machines of `runtime`. One store per runtime. */
export function machineConnections(runtime: ConnectionSource): Readable<MachineConnections> {
  const known = STORES.get(runtime);
  if (known !== undefined) {
    return known;
  }
  const store = createAtom<MachineConnections>(() => {
    const connections = new Map<MachineId, MachineConnection>();
    for (const machine of runtime.registry.get().machines) {
      const supervisor = runtime.machine(machine.id);
      if (supervisor !== undefined) {
        connections.set(machine.id, supervisor.connection.get());
      }
    }
    return connections;
  });
  STORES.set(runtime, store);
  return store;
}

/** True when the two maps hold the same connection objects. */
export function sameConnections(a: MachineConnections, b: MachineConnections): boolean {
  if (a.size !== b.size) {
    return false;
  }
  for (const [id, connection] of a) {
    if (b.get(id) !== connection) {
      return false;
    }
  }
  return true;
}

/** The machines whose connection is up now. */
export function onlineMachines(connections: MachineConnections): ReadonlySet<MachineId> {
  const online = new Set<MachineId>();
  for (const [id, connection] of connections) {
    if (connection.view.kind === "online") {
      online.add(id);
    }
  }
  return online;
}
