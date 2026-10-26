// The stream of the conversation on a screen, and what the screen does while it has the
// focus (plan section 2.2, "Subscriptions", design sections 3.9 and 5):
//
// - In the stack, the screen keeps the stream; with the focus, it follows it, so the
//   subscription is up only for the screen that shows. A return to the focus subscribes
//   again with `after_seq` from the stream's cursor.
// - With the focus: VoiceOver announcements, the error haptic of a failed turn, and the
//   scroll to the approval of a `focus=approval` request.
//
// The hold is an external store for useSyncExternalStore (model/screen-stream.ts): the
// subscribe takes the hold and starts the watches, and the unsubscribe stops them. So the
// screen has no effect, and React ends the hold with the screen.

import { useSyncExternalStore, useState, type RefObject } from "react";
import { AccessibilityInfo } from "react-native";

import { haptic } from "@/lib/native/haptics";
import { reduceMotion } from "@/lib/native/reduce-motion";
import { animationFrames } from "@/lib/store/frame-store";
import type { ConversationId, MachineId } from "@/features/machines/model/machine-ref";
import type { ConversationFocus } from "@/features/navigation/model/routes";
import { useRuntime } from "@/features/navigation/use-app-services";
import type { TranscriptListHandle } from "../components/transcript-list";
import type { Announcement } from "../model/announce";
import type { ConversationStream } from "../model/conversation-stream";
import { ScreenStream } from "../model/screen-stream";
import { followApprovalFocus, watchFocusedStream } from "../model/screen-watch";

/** What the stream of a screen reads. */
export interface ScreenStreamOptions {
  /** The daemon id of the machine. */
  readonly machine: MachineId;
  /** The conversation id, or undefined while efrd has not made it: then no stream. */
  readonly id: ConversationId | undefined;
  /** True while the screen has the focus. */
  readonly focused: boolean;
  /** The `focus` param of the route: `approval` scrolls to the approval that waits. */
  readonly focus: ConversationFocus | undefined;
  /** The handle of the transcript list. */
  readonly list: RefObject<TranscriptListHandle | null>;
  /** Called when the `focus` request is done, to clear the param. */
  readonly onFocusDone: () => void;
}

/** Says an announcement with VoiceOver. iOS has no live region in React Native. */
function announce(announcement: Announcement): void {
  AccessibilityInfo.announceForAccessibilityWithOptions(announcement.text, {
    queue: announcement.queue,
  });
}

function failedTurn(): void {
  haptic("error");
}

/** The stream of the screen, or undefined until the screen holds it. */
export function useScreenStream(options: ScreenStreamOptions): ConversationStream | undefined {
  const runtime = useRuntime();
  const [screen] = useState(() => new ScreenStream(runtime));
  // The subscribe reads plain values and the stable callbacks of the screen, so it
  // changes only when one of them does, and a render of the screen does not take the hold
  // again.
  const { machine, id, focused, focus, list, onFocusDone } = options;
  const subscribe = (onChange: () => void): (() => void) => {
    if (id === undefined) {
      return () => undefined;
    }
    const conversation = { machine, id };
    const stops = [screen.hold(conversation, focused ? "follow" : "keep")];
    const stream = screen.stream(conversation);
    if (focused && stream !== undefined) {
      stops.push(watchFocusedStream(stream.store, { announce, failedTurn }));
      if (focus === "approval") {
        stops.push(
          followApprovalFocus(stream.store, {
            schedule: animationFrames,
            scrollTo: (key) => list.current?.scrollToKey(key, !reduceMotion()),
            done: onFocusDone,
          }),
        );
      }
    }
    // The snapshot has the stream now: the screen renders with it.
    onChange();
    return () => {
      for (let i = stops.length - 1; i >= 0; i--) {
        stops[i]?.();
      }
    };
  };
  return useSyncExternalStore(subscribe, () =>
    id === undefined ? undefined : screen.stream({ machine, id }),
  );
}
