// The queries and mutations of the machines (plan section 2.2): the key factory, the status
// of efrd from the last hello, Forget, and pairing (a new machine or a re-pair). Every key of
// a machine starts with `["machine", id]`, so `invalidateOnReady` refreshes all the
// machine's queries, and Forget removes them, with one key.

import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import { ALL_MACHINES_KEY, machineKey } from "@/lib/query/keys";
import type { Machine } from "./model/machine";
import type { MachineId } from "./model/machine-ref";
import type { PairOptions, Runtime } from "./model/runtime";

/** The query keys of the machines. */
export const machineKeys = {
  all: ALL_MACHINES_KEY,
  machine: (machine: MachineId) => machineKey(machine),
  status: (machine: MachineId) => [...machineKey(machine), "status"] as const,
};

/** The mutation keys of the machines. */
export const machineMutationKeys = {
  forget: ["machine", "forget"] as const,
  pair: ["machine", "pair"] as const,
};

/**
 * The status of efrd on `machine` (version, protocol, scopes, when it was seen). It is
 * kept on the phone, so Settings shows "efrd 0.0.5 · protocol 1" while the machine is
 * offline. It fails while the machine is not connected; the kept value stays.
 */
export function machineStatusQueryOptions(runtime: Runtime, machine: MachineId) {
  return queryOptions({
    queryKey: machineKeys.status(machine),
    queryFn: () => runtime.status(machine),
    meta: { persist: true },
  });
}

/** Forgets a machine. After it, the machine's queries are gone. */
export function forgetMachineMutationOptions(runtime: Runtime) {
  return mutationOptions({
    mutationKey: machineMutationKeys.forget,
    mutationFn: (machine: MachineId) => runtime.forget(machine),
    onSuccess: (_result, machine, _onMutate, context) => {
      context.client.removeQueries({ queryKey: machineKeys.machine(machine) });
    },
  });
}

/** What a pairing needs: the text of the QR code and the options of the flow. */
export interface PairVariables extends PairOptions {
  readonly code: string;
}

/**
 * Pairs a machine, or pairs one again when `repair` names it. It rejects with a
 * `PairingError` (src/features/pairing/model/pair.ts) when the flow gives no device.
 */
export function pairMachineMutationOptions(runtime: Runtime) {
  return mutationOptions({
    mutationKey: machineMutationKeys.pair,
    mutationFn: ({ code, ...options }: PairVariables): Promise<Machine> =>
      runtime.pair(code, options),
    onSuccess: (machine, _variables, _onMutate, context) =>
      context.client.invalidateQueries({ queryKey: machineKeys.machine(machine.id) }),
  });
}

/**
 * Refreshes the queries of each machine whose connection becomes ready, so that what the
 * phone shows catches up after a reconnect. Returns the function that stops it.
 */
export function invalidateOnReady(runtime: Runtime, client: QueryClient): () => void {
  return runtime.onReady((machine) => {
    void client.invalidateQueries({ queryKey: machineKeys.machine(machine) });
  });
}
