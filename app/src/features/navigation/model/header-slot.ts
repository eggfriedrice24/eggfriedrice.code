// What the 34 pt slot under the header of a conversation shows (design section 2, items
// 8.6 and 8.19, plan section 2.5). Pure.
//
// - One approval in another conversation: "Approval waiting · machine · title", and a tap
//   opens that conversation at its approval.
// - Two or more: "2 approvals waiting", and a tap opens the sheet with the waiting section
//   on top (iOS cannot open a menu from code).
// - Else, when the machine of the screen has no connection: the offline line, such as
//   "laptop offline · seen 13:58 · retrying". It is quiet and takes no tap.
// - An approval strip wins the slot over the offline line.
//
// The offline line waits until the reconnect failed for a while (design section 1, "After
// a few failed tries, the offline line replaces reconnecting…"): for OFFLINE_GRACE_MS
// after the connection went down, and while the client connects again, the composer's
// status line says "reconnecting…" instead.

import { machineStateWords, type ZoneOffset } from "@/features/machines/model/machine-status";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { ConversationRef, MachineId } from "@/features/machines/model/machine-ref";
import { rowTitle } from "@/features/conversations/model/list-format";
import { manyApprovalsText, type ApprovalsElsewhere } from "@/features/conversations/model/waiting";

/** What the slot shows. */
export type SlotContent =
  | { readonly kind: "none" }
  | {
      readonly kind: "approval";
      readonly conversation: ConversationRef;
      readonly machineName: string;
      readonly title: string;
    }
  | { readonly kind: "approvals"; readonly count: number; readonly text: string }
  | { readonly kind: "offline"; readonly machineName: string; readonly words: string };

/** How long after the connection went down the offline line waits. */
export const OFFLINE_GRACE_MS = 10_000;

/** The machine of the screen. */
export interface SlotMachine {
  readonly name: string;
  readonly connection: MachineConnection | undefined;
  /** True once the time of `offlineFrom` is reached. */
  readonly graceOver: boolean;
}

/** What the slot reads. */
export interface SlotInput {
  readonly elsewhere: ApprovalsElsewhere;
  /** The name of a machine, such as `desktop`, for the strip. */
  readonly machineName: (machine: MachineId) => string;
  /** The machine of the screen; undefined when it is not paired any more. */
  readonly machine: SlotMachine | undefined;
  /** The time now, ms since the epoch, for "seen 13:58". */
  readonly now: number;
  readonly zone: ZoneOffset;
  /** True when no machine of the phone answers (see `isTailnetDown`). */
  readonly tailnetDown: boolean;
}

/** The slot of a conversation screen. */
export function slotContent(input: SlotInput): SlotContent {
  const { elsewhere } = input;
  if (elsewhere.kind === "one") {
    const { machine, id, title } = elsewhere.conversation;
    return {
      kind: "approval",
      conversation: { machine, id },
      machineName: input.machineName(machine),
      title: rowTitle(title),
    };
  }
  if (elsewhere.kind === "many") {
    return { kind: "approvals", count: elsewhere.count, text: manyApprovalsText(elsewhere.count) };
  }
  const machine = input.machine;
  if (machine?.connection === undefined || !machine.graceOver) {
    return { kind: "none" };
  }
  const words = offlineWords(machine.connection, input.now, input.zone, input.tailnetDown);
  return words === undefined
    ? { kind: "none" }
    : { kind: "offline", machineName: machine.name, words };
}

/**
 * The words of the offline line after the machine name, such as "offline · seen 13:58 ·
 * retrying", or undefined when the line does not show: the machine is connected, it
 * connects, or the client only connects again after a closed connection.
 */
export function offlineWords(
  connection: MachineConnection,
  now: number,
  zone: ZoneOffset,
  tailnetDown: boolean,
): string | undefined {
  const { view } = connection;
  switch (view.kind) {
    case "online":
    case "connecting":
    case "retrying":
      return undefined;
    default: {
      const { line } = machineStateWords(connection, now, zone, tailnetDown);
      const retrying = "retryAt" in view && view.retryAt !== undefined;
      return retrying ? `${line} · retrying` : line;
    }
  }
}

/**
 * When the offline line may show for `connection`, in the time of the clock of the
 * machine: OFFLINE_GRACE_MS after the connection went down, or at once (undefined) for a
 * machine that was not connected in this run of the app.
 */
export function offlineFrom(connection: MachineConnection | undefined): number | undefined {
  if (connection === undefined || connection.seenAt === undefined) {
    return undefined;
  }
  return connection.seenAt + OFFLINE_GRACE_MS;
}

/** The VoiceOver label of the slot. */
export function slotLabel(content: SlotContent): string | undefined {
  switch (content.kind) {
    case "approval":
      return `Approval waiting, ${content.machineName}, ${content.title}`;
    case "approvals":
      return content.text;
    case "offline":
      return `${content.machineName} ${content.words}`;
    default:
      return undefined;
  }
}
