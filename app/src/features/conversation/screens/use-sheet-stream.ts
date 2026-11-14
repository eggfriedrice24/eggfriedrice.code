// The stream of the conversation of a sheet (the approval sheet, the model sheet), held
// while the sheet shows, with the subscription up. The conversation screen under the
// sheet loses the focus and lets its own hold go; both holds share the one stream of the
// runtime, so the sheet and the transcript show the same state. The sheet speaks no
// announcement of its own and scrolls nothing.

import { useState, useSyncExternalStore } from "react";

import { haptic } from "@/lib/native/haptics";
import { animationFrames } from "@/lib/store/frame-store";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { useRuntime } from "@/features/navigation/use-app-services";
import { ScreenStream } from "../model/screen-stream";

/** The stream of `target`, or undefined for none. */
export function useSheetStream(target: ConversationRef | undefined) {
  const runtime = useRuntime();
  const [screen] = useState(
    () =>
      new ScreenStream(runtime, {
        announce: () => undefined,
        failedTurn: () => haptic("error"),
        schedule: animationFrames,
        scrollTo: () => undefined,
        focusDone: () => undefined,
      }),
  );
  const machine = target?.machine ?? "";
  const id = target?.id;
  return useSyncExternalStore(
    screen.subscriber({ machine, id, focused: true, focus: undefined }),
    () => (id === undefined ? undefined : screen.stream({ machine, id })),
  );
}
