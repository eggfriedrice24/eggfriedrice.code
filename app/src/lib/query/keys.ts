// The root of every query key and mutation key of the app, and the address of one
// conversation. Each feature builds its key factory on these roots (plan section 2.2),
// so one prefix holds all the data of a machine:
//
// - The supervisor invalidates `machineKey(id)` when the machine becomes ready.
// - Forget removes `machineKey(id)`, and the persister then drops it from the disk.

/** The prefix of the keys of all machines. */
export const ALL_MACHINES_KEY = ["machine"] as const;

/** The prefix of the keys of the data of one machine (its `daemonId`). */
export function machineKey(machine: string) {
  return ["machine", machine] as const;
}

/**
 * One conversation on one machine. A `ConversationRef` of the machines feature has the
 * same members, so it can go where this type goes.
 */
export interface ConversationAddress {
  /** The `daemonId` of the machine. */
  readonly machine: string;
  /** The conversation id. */
  readonly id: string;
}

/**
 * The scope of the mutations of one conversation. Query runs the mutations of one scope
 * one after the other, so the answers, steers and withdraws reach efrd in order.
 */
export function conversationScope(ref: ConversationAddress): { readonly id: string } {
  return { id: `${ref.machine}:${ref.id}` };
}
