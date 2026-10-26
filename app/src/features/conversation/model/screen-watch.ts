// What the conversation screen does while it has the focus, as watches of its stream
// store (plan section 2.2, design sections 3.9 and 5, R4c.4 step 9 of the research):
//
// - VoiceOver hears "Approval needed" at once and "Reply done" when a turn ends
//   (model/announce.ts). The app never announces tokens.
// - A turn that fails gives the error haptic once, only while its conversation is on
//   screen (design 3.9).
// - A screen that opens with `focus=approval` (the strip, a pick in the sheet) scrolls to
//   the approval that waits, then clears the request.
//
// The first load of the stream (the cache, then the snapshot of efrd) is not news: it
// gives no announcement and no haptic. The events after it are.
//
// Each watch starts when the screen gets the focus and stops when it loses it. Pure:
// the effects come in as parameters.

import type { Readable } from "@tanstack/store";

import type { FrameScheduler } from "@/lib/store/frame-store";
import { pendingApprovals, type StreamState } from "@/lib/stream/reducer";
import { announcements, type Announcement } from "./announce";

/** The effects of a focused conversation screen. */
export interface FocusedEffects {
  /** Says `announcement` with VoiceOver. */
  readonly announce: (announcement: Announcement) => void;
  /** Plays the error haptic of a failed turn. */
  readonly failedTurn: () => void;
}

/** The turns that failed between `before` and `after`, oldest first. */
export function failedTurns(before: StreamState, after: StreamState): string[] {
  if (before.turns === after.turns) {
    return [];
  }
  const failed: string[] = [];
  for (const turnId of after.order) {
    const turn = after.turns.get(turnId);
    if (turn?.end?.outcome !== "failed") {
      continue;
    }
    const old = before.turns.get(turnId);
    if (old !== undefined && old.end === undefined) {
      failed.push(turnId);
    }
  }
  return failed;
}

/**
 * Watches `store` for news while the screen has the focus. Returns the function that
 * stops the watch.
 */
export function watchFocusedStream(
  store: Readable<StreamState>,
  effects: FocusedEffects,
): () => void {
  let last = store.get();
  const subscription = store.subscribe(() => {
    const next = store.get();
    const before = last;
    last = next;
    if (!before.loaded) {
      return;
    }
    for (const announcement of announcements(before, next)) {
      effects.announce(announcement);
    }
    if (failedTurns(before, next).length > 0) {
      effects.failedTurn();
    }
  });
  return () => subscription.unsubscribe();
}

/** The key of the approval row of `turnId` and `callId`, as model/rows.ts makes it. */
export function approvalRowKey(turnId: string, callId: string): string {
  return `a:${turnId}:${callId}`;
}

/** The row key of the oldest approval that waits, or undefined when none waits. */
export function waitingApprovalKey(state: StreamState): string | undefined {
  const approval = pendingApprovals(state)[0];
  return approval === undefined ? undefined : approvalRowKey(approval.turnId, approval.callId);
}

/** What a request to scroll to the waiting approval needs. */
export interface ApprovalFocusDeps {
  /** Runs a callback on the next display frame, after the list drew the new rows. */
  readonly schedule: FrameScheduler;
  /** Scrolls the transcript to the row with `key`. */
  readonly scrollTo: (key: string) => void;
  /** The request is done: the screen clears its `focus` param. */
  readonly done: () => void;
}

/**
 * Scrolls to the approval that waits, once the stream has it. When a snapshot of efrd
 * comes after the request and no approval waits in it (the question was answered or
 * expired), the request ends with no scroll. Else it waits while the screen has the
 * focus: the events after the cursor of a kept stream can bring the question late.
 * Returns the function that stops the request.
 */
export function followApprovalFocus(
  store: Readable<StreamState>,
  deps: ApprovalFocusDeps,
): () => void {
  let cancel: (() => void) | undefined;
  let finished = false;
  let subscription: { unsubscribe(): void } | undefined;
  const asked = store.get().summary;
  const finish = (): void => {
    finished = true;
    subscription?.unsubscribe();
    subscription = undefined;
  };
  const check = (): void => {
    if (finished) {
      return;
    }
    const state = store.get();
    const key = waitingApprovalKey(state);
    if (key !== undefined) {
      finish();
      cancel = deps.schedule(() => {
        cancel = undefined;
        deps.scrollTo(key);
        deps.done();
      });
    } else if (state.summary !== asked) {
      finish();
      deps.done();
    }
  };
  subscription = store.subscribe(check);
  check();
  return () => {
    finish();
    cancel?.();
    cancel = undefined;
  };
}
