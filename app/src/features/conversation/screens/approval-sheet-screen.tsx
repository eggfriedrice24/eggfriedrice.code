// The approval sheet route (board AP-Sheets): the root stack presents it as a form sheet
// that fits its content, with the grabber and no bar (APPROVAL_OPTIONS). It opens only
// from a tap on the approval strip or on the waiting tool card of a conversation.
//
// The sheet holds the stream of its conversation and keeps the subscription up while it
// shows, because the conversation screen under it loses the focus and lets its own go.
// Both holds share the one stream of the runtime, so the sheet and the transcript show
// the same question. Close and an answer that efrd took go back to the conversation;
// Next shows the next question that waits in the same sheet.

import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";
import { useRouter } from "expo-router";
import { useState, useSyncExternalStore } from "react";

import { haptic } from "@/lib/native/haptics";
import { animationFrames } from "@/lib/store/frame-store";
import type { StreamState } from "@/lib/stream/reducer";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { useRuntime } from "@/features/navigation/use-app-services";
import { useMachines } from "@/features/navigation/use-machines";
import { ApprovalSheet } from "../components/approval-sheet";
import { ScreenStream } from "../model/screen-stream";

export interface ApprovalSheetScreenProps {
  /** The conversation and the call of the route, or undefined when the params are bad. */
  readonly target: (ConversationRef & { readonly call: string }) | undefined;
}

/** The approval sheet. */
export function ApprovalSheetScreen(props: ApprovalSheetScreenProps) {
  const runtime = useRuntime();
  const machines = useMachines();
  const router = useRouter();
  const target = props.target;
  const machine = machines.find((one) => one.id === target?.machine);
  const supervisor = target === undefined ? undefined : runtime.machine(target.machine);
  const stream = useSheetStream(target);
  const close = (): void => {
    if (router.canGoBack()) {
      router.back();
    }
  };
  if (target === undefined || machine === undefined || supervisor === undefined) {
    return null;
  }
  if (stream === undefined) {
    return null;
  }
  return (
    <SheetBody
      target={target}
      machineName={machine.name}
      connection={supervisor.connection}
      store={stream.store}
      onClose={close}
      onNext={(call) => router.setParams({ call })}
    />
  );
}

interface SheetBodyProps {
  readonly target: ConversationRef & { readonly call: string };
  readonly machineName: string;
  readonly connection: Readable<MachineConnection>;
  readonly store: Readable<StreamState>;
  readonly onClose: () => void;
  readonly onNext: (call: string) => void;
}

function SheetBody(props: SheetBodyProps) {
  const runtime = useRuntime();
  const connected = useSelector(
    props.connection,
    (connection) => connection.view.kind === "online",
  );
  const { target } = props;
  return (
    <ApprovalSheet
      stream={props.store}
      conversation={{ machine: target.machine, id: target.id }}
      callId={target.call}
      machineName={props.machineName}
      connected={connected}
      caller={runtime}
      onClose={props.onClose}
      onNext={props.onNext}
    />
  );
}

/**
 * The stream of the conversation of `target`, held while the sheet shows, with the
 * subscription up. The sheet speaks no announcement of its own and scrolls nothing.
 */
function useSheetStream(target: ConversationRef | undefined) {
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
