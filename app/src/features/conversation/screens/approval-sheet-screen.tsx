// The approval sheet route (board AP-Sheets): the root stack presents it as a form sheet
// that fits its content, with the grabber and the native header with no title
// (APPROVAL_OPTIONS). It opens only from a tap on the approval strip or on the waiting
// tool card of a conversation.
//
// The sheet holds the stream of its conversation and keeps the subscription up while it
// shows, because the conversation screen under it loses the focus and lets its own go.
// Both holds share the one stream of the runtime, so the sheet and the transcript show
// the same question. Close (the native header, the hard rule of app/AGENTS.md) and an
// answer that efrd took go back to the conversation; Next, in the header after the
// question closed, shows the next question that waits in the same sheet.

import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";
import { Stack, useRouter } from "expo-router";

import { findApproval, type StreamState } from "@/lib/stream/reducer";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { useRuntime } from "@/features/navigation/use-app-services";
import { useMachines } from "@/features/navigation/use-machines";
import { ApprovalSheet } from "../components/approval-sheet";
import { nextQuestion } from "../model/approval-strip";
import { selectPendingApprovals } from "../model/selectors";
import { useSheetStream } from "./use-sheet-stream";

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
  const next = useSelector(props.store, (state) =>
    nextQuestion(findApproval(state, target.call), selectPendingApprovals(state)),
  );
  return (
    <>
      <Stack.Toolbar placement="right">
        {next === undefined ? null : (
          <Stack.Toolbar.Button
            accessibilityLabel="Next question"
            variant="done"
            onPress={() => props.onNext(next)}
          >
            Next
          </Stack.Toolbar.Button>
        )}
        <Stack.Toolbar.Button icon="xmark" accessibilityLabel="Close" onPress={props.onClose} />
      </Stack.Toolbar>
      <ApprovalSheet
        stream={props.store}
        conversation={{ machine: target.machine, id: target.id }}
        callId={target.call}
        machineName={props.machineName}
        connected={connected}
        caller={runtime}
        onClose={props.onClose}
      />
    </>
  );
}
