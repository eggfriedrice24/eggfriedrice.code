// The names of a machine and of a conversation on it. A machine is one paired efrd; its id
// is the daemon id (a UUID), which is also its route slug (`c/[machine]/[id]`). A
// conversation id is unique only on its machine, so the app keys every conversation by both.

/** The daemon id of a paired machine. */
export type MachineId = string;

/** The id of a conversation on its machine. */
export type ConversationId = string;

/** One conversation on one machine. */
export interface ConversationRef {
  readonly machine: MachineId;
  readonly id: ConversationId;
}

/** The key of a conversation in maps and in mutation scopes: `<machine>:<id>`. */
export function conversationKey(ref: ConversationRef): string {
  return `${ref.machine}:${ref.id}`;
}
