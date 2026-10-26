// What the slot under the header of a conversation shows (model/header-slot.ts). The
// conversation screen can read it too, to keep its content clear of the strip.

import { isTailnetDown } from "@/features/machines/model/machine-status";
import type { ConversationRef, MachineId } from "@/features/machines/model/machine-ref";
import { offlineFrom, slotContent, type SlotContent } from "./model/header-slot";
import { useApprovalsElsewhere } from "./use-approvals-elsewhere";
import { useMachineConnections, useMachines } from "./use-machines";
import { zoneAt } from "@/features/machines/model/machine-status";
import { useNow, useReached } from "./use-now";

/** The words for a machine that is not paired any more. */
const UNKNOWN_MACHINE = "another machine";

/** The slot of the conversation screen of `current`. */
export function useHeaderSlot(current: ConversationRef): SlotContent {
  const machines = useMachines();
  const connections = useMachineConnections();
  const elsewhere = useApprovalsElsewhere(current);
  const now = useNow();
  const nameOf = (id: MachineId): string =>
    machines.find((machine) => machine.id === id)?.name ?? UNKNOWN_MACHINE;
  const machine = machines.find((one) => one.id === current.machine);
  const connection = connections.get(current.machine);
  const graceOver = useReached(offlineFrom(connection));
  return slotContent({
    elsewhere,
    machineName: nameOf,
    machine: machine === undefined ? undefined : { name: machine.name, connection, graceOver },
    now,
    zone: zoneAt(now),
    tailnetDown: isTailnetDown([...connections.values()]),
  });
}
