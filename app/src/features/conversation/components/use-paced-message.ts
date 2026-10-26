// The paced view of one message row (model/paced-message.ts) as a hook. The row reads
// it with useSyncExternalStore: the store follows the stream and the display frames
// only while the row is mounted, and the row renders only when its shown text changes.
//
// The paced view is made once per component instance. The list recycles row instances,
// so the caller must key the component by the turn and the index of the message
// (message-row.tsx): then one instance always shows one message.

import type { Readable } from "@tanstack/store";
import { useState, useSyncExternalStore } from "react";

import { reduceMotion } from "@/lib/native/reduce-motion";
import { animationFrames } from "@/lib/store/frame-store";
import type { StreamState } from "@/lib/stream/reducer";
import { createPacedMessage, type ShownMessage } from "../model/paced-message";

/** What the message row shows of message `index` of turn `turnId` on this frame. */
export function usePacedMessage(
  store: Readable<StreamState>,
  turnId: string,
  index: number,
): ShownMessage {
  const [paced] = useState(() =>
    createPacedMessage(store, turnId, index, {
      schedule: animationFrames,
      now: () => Date.now(),
      reduceMotion,
    }),
  );
  return useSyncExternalStore(paced.subscribe, paced.snapshot);
}
