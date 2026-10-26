// The VoiceOver announcements of a conversation (R4c.4 step 9 of the research). The app
// never announces tokens: a reader hears "Reply done" when a turn ends, in the queue at
// low priority, and "Approval needed" at once, at high priority. iOS has no live region
// in React Native, so the screen sends each announcement with
// `AccessibilityInfo.announceForAccessibilityWithOptions`.
//
// Pure: the screen passes the stream state before and after a change.

import type { StreamState, TurnOutcome } from "@/lib/stream/reducer";

/** One announcement. */
export interface Announcement {
  readonly text: string;
  /** `high` interrupts what VoiceOver says now; `low` waits for it. */
  readonly priority: "high" | "low";
  /** Wait for the current speech to end. */
  readonly queue: boolean;
}

/**
 * The announcements for the change from `before` to `after`: each new approval question,
 * and the end of each turn that ran while the screen was open.
 */
export function announcements(before: StreamState, after: StreamState): Announcement[] {
  if (before.turns === after.turns) {
    return [];
  }
  const out: Announcement[] = [];
  for (const turnId of after.order) {
    const turn = after.turns.get(turnId);
    if (turn === undefined) {
      continue;
    }
    const old = before.turns.get(turnId);
    if (old === turn) {
      continue;
    }
    for (const [callId, approval] of turn.approvals) {
      if (approval.status !== "pending" || old?.approvals.get(callId)?.status === "pending") {
        continue;
      }
      out.push({
        text: approval.localOnly
          ? `Approval needed on your computer. ${approval.summary}`
          : `Approval needed. ${approval.summary}`,
        priority: "high",
        queue: false,
      });
    }
    if (turn.end !== undefined && old !== undefined && old.end === undefined && old.started) {
      const text = endText(turn.end.outcome, turn.end.error);
      if (text !== undefined) {
        out.push({ text, priority: "low", queue: true });
      }
    }
  }
  return out;
}

function endText(outcome: TurnOutcome, error: string | undefined): string | undefined {
  switch (outcome) {
    case "completed":
      return "Reply done.";
    case "failed":
      return error === undefined ? "The turn failed." : `The turn failed. ${error}`;
    case "interrupted":
      return "The turn stopped.";
    case "cancelled":
      return "efr cancelled the turn.";
    default:
      return undefined;
  }
}
