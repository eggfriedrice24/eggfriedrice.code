// One row of a conversation list, from efr-protocol's `ConversationSummary`. Pure, for
// the tests.

import type { ConversationSummary } from "@/lib/protocol/generated";

/** One conversation of the list (from efr-protocol's `ConversationSummary`). */
export interface ConversationItem {
  readonly id: string;
  readonly title?: string;
  /** `idle`, `running` or `awaiting_approval`; another value is shown as idle. */
  readonly status: string;
  /** When its last event was recorded, in ms since the epoch. */
  readonly updatedAt: number;
  /** The working directory at the last prompt. */
  readonly cwd?: string;
}

/** One row of the conversation list. */
export function conversationItem(summary: ConversationSummary): ConversationItem {
  const updatedAt = Date.parse(summary.updated_at);
  return {
    id: summary.id,
    ...(summary.title === undefined || summary.title === null ? {} : { title: summary.title }),
    status: summary.status,
    updatedAt: Number.isNaN(updatedAt) ? 0 : updatedAt,
    ...(summary.cwd === undefined || summary.cwd === null ? {} : { cwd: summary.cwd }),
  };
}
