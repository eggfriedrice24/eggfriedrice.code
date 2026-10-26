// One drain of the outbox for each paired machine (plan section 2.2, "Outbox for
// prompts"). The drain of a machine sends its prompts while the machine is connected:
//
// - It starts when the connection of the machine becomes ready, and only after the outbox
//   read the entries of the last launch, so the old prompts go first and in order.
// - It stops when the connection goes down. A prompt in flight still ends.
// - A machine that the user pairs gets a drain; a machine that the user forgets loses it.
//
// Pure: the runtime, the clock and the random value come in.

import type { Clock } from "@/lib/net/clock";
import type { PromptSend, PromptSendResult } from "@/lib/protocol/generated";
import { createOutboxDrain, type OutboxDrain } from "@/features/conversation/model/outbox-drain";
import type { Outbox } from "@/features/conversation/model/outbox";
import type { MachineId } from "@/features/machines/model/machine-ref";
import type { Runtime } from "@/features/machines/model/runtime";

/** What the drains need. */
export interface OutboxDrainsDeps {
  readonly runtime: Pick<Runtime, "registry" | "machine" | "call">;
  readonly outbox: Outbox;
  /** Settles when the outbox read the entries of the last launch. */
  readonly hydrated: Promise<void>;
  readonly clock: Clock;
  /** A random value in [0, 1) for the backoff ladders. */
  readonly random: () => number;
}

interface MachineDrain {
  readonly drain: OutboxDrain;
  online: boolean;
  stop(): void;
}

/** Starts a drain for each paired machine. Returns the function that stops them all. */
export function wireOutboxDrains(deps: OutboxDrainsDeps): () => void {
  const { runtime, outbox } = deps;
  const drains = new Map<MachineId, MachineDrain>();
  let hydrated = false;
  let stopped = false;

  const update = (entry: MachineDrain): void => {
    if (hydrated && entry.online) {
      entry.drain.start();
    } else {
      entry.drain.stop();
    }
  };

  const add = (machine: MachineId): void => {
    const supervisor = runtime.machine(machine);
    if (supervisor === undefined) {
      return;
    }
    const drain = createOutboxDrain({
      outbox,
      machine,
      send: (params: PromptSend): Promise<PromptSendResult> =>
        runtime.call(machine, "prompt.send", params),
      clock: deps.clock,
      random: deps.random,
    });
    const isOnline = (): boolean => supervisor.connection.get().view.kind === "online";
    const entry: MachineDrain = {
      drain,
      online: isOnline(),
      stop: () => {
        subscription.unsubscribe();
        drain.stop();
      },
    };
    const subscription = supervisor.connection.subscribe(() => {
      const online = isOnline();
      if (online !== entry.online) {
        entry.online = online;
        update(entry);
      }
    });
    drains.set(machine, entry);
    update(entry);
  };

  const sync = (): void => {
    if (stopped) {
      return;
    }
    const paired = new Set(runtime.registry.get().machines.map((machine) => machine.id));
    for (const [machine, entry] of drains) {
      if (!paired.has(machine) || runtime.machine(machine) === undefined) {
        entry.stop();
        drains.delete(machine);
      }
    }
    for (const machine of paired) {
      if (!drains.has(machine)) {
        add(machine);
      }
    }
  };

  const registry = runtime.registry.subscribe(sync);
  sync();
  // A failed read leaves the outbox empty but usable, so the drains start after it too.
  const open = async (): Promise<void> => {
    await deps.hydrated.catch(() => undefined);
    hydrated = true;
    if (!stopped) {
      for (const entry of drains.values()) {
        update(entry);
      }
    }
  };
  void open();

  return () => {
    stopped = true;
    registry.unsubscribe();
    for (const entry of drains.values()) {
      entry.stop();
    }
    drains.clear();
  };
}
