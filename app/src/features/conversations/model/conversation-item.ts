// One row of a conversation list, from efr-protocol's `ConversationSummary`, with the
// machine that holds it. Pure, for the tests.

import type { ConversationStatus, ConversationSummary } from "@/lib/protocol/generated";

/** One conversation of a list (from efr-protocol's `ConversationSummary`). */
export interface ConversationItem {
  /** The `daemonId` of the machine that holds it. */
  readonly machine: string;
  readonly id: string;
  readonly title?: string;
  /** What it does now. A newer efrd can send another word; it reads as idle. */
  readonly status: ConversationStatus;
  /** When its last event was recorded, in ms since the epoch. */
  readonly updatedAt: number;
  /** The working directory at the last prompt. */
  readonly cwd?: string;
}

function statusOf(status: string): ConversationStatus {
  switch (status) {
    case "running":
    case "awaiting_approval":
      return status;
    default:
      return "idle";
  }
}

/** One row of a conversation list of `machine`. */
export function conversationItem(machine: string, summary: ConversationSummary): ConversationItem {
  const parsed = Date.parse(summary.updated_at);
  const item: {
    machine: string;
    id: string;
    title?: string;
    status: ConversationStatus;
    updatedAt: number;
    cwd?: string;
  } = {
    machine,
    id: summary.id,
    status: statusOf(summary.status),
    updatedAt: Number.isNaN(parsed) ? 0 : parsed,
  };
  if (summary.title !== undefined && summary.title !== null && summary.title.length > 0) {
    item.title = summary.title;
  }
  if (summary.cwd !== undefined && summary.cwd !== null && summary.cwd.length > 0) {
    item.cwd = summary.cwd;
  }
  return item;
}

/** True when `a` and `b` name the same conversation on the same machine. */
export function sameConversation(
  a: { readonly machine: string; readonly id: string },
  b: { readonly machine: string; readonly id: string },
): boolean {
  return a.machine === b.machine && a.id === b.id;
}

/** Newest first; a tie goes by id, so the order never depends on the input order. */
export function newestFirst(a: ConversationItem, b: ConversationItem): number {
  if (a.updatedAt !== b.updatedAt) {
    return b.updatedAt - a.updatedAt;
  }
  if (a.machine !== b.machine) {
    return a.machine < b.machine ? -1 : 1;
  }
  if (a.id === b.id) {
    return 0;
  }
  return a.id < b.id ? -1 : 1;
}
