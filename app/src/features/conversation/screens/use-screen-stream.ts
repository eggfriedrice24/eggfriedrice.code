// The stream of the conversation on a screen, and what the screen does while it has the
// focus (plan section 2.2, "Subscriptions", design sections 3.9 and 5):
//
// - In the stack, the screen keeps the stream; with the focus, it follows it, so the
//   subscription is up only for the screen that shows. A return to the focus subscribes
//   again with `after_seq` from the stream's cursor.
// - With the focus: VoiceOver announcements, the error haptic of a failed turn, and the
//   scroll to the approval of a `focus=approval` request, which then leaves the route.
//
// The hold is an external store for useSyncExternalStore (model/screen-stream.ts): the
// subscribe takes the hold and starts the watches, and the unsubscribe stops them. So the
// screen has no effect, and React ends the hold with the screen.

import {
  useNavigation,
  type NavigationProp,
  type ParamListBase,
} from "expo-router/react-navigation";
import { useState, useSyncExternalStore, type RefObject } from "react";
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
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const { machine, id, focused, focus, list } = options;
  const [screen] = useState(
    () =>
      new ScreenStream(runtime, {
        announce,
        failedTurn,
        schedule: animationFrames,
        scrollTo: (key) => list.current?.scrollToKey(key, !reduceMotion()),
        // The params of the route without `focus`: a merge would keep the key, as
        // undefined, and the params would not parse.
        focusDone: (ref) => navigation.replaceParams({ machine: ref.machine, id: ref.id }),
      }),
  );
  return useSyncExternalStore(screen.subscriber({ machine, id, focused, focus }), () =>
    id === undefined ? undefined : screen.stream({ machine, id }),
  );
}
