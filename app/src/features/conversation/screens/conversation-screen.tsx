// One conversation of one machine, pushed on the stack (plan sections 2.2 and 2.5, design
// boards R-Streaming, R-Approval, R-Other, C-Offline, C-Reconnecting, C-Failed and C-Jump).
//
// - The transcript of the stream, with the prompts of the outbox after it, under the
//   transparent bar. The content clears the bar, and the strip while the slot under the
//   bar shows (an approval elsewhere, or the offline line).
// - The docked composer at the bottom. Its height is the bottom inset of the transcript,
//   so "Jump to latest" sits 12 pt above it.
// - The screen holds the stream while it is in the stack, and keeps the subscription up
//   only while it has the focus (use-screen-stream.ts). With the focus it also gives the
//   VoiceOver announcements, the error haptic of a failed turn and the scroll to the
//   approval of `focus=approval`.
// - Until the stream is held and has events, the transcript shows its loading line. A
//   machine that does not answer keeps the transcript of the cache: the strip shows the
//   offline line, and the composer says why it cannot send.
// - A new conversation of the blank root shows its first prompt on its way until efrd
//   made the conversation (use-route-conversation.ts).
//
// The header belongs to the root stack (features/navigation), so this screen sets none.

import { useMutation } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import { createStore, type Readable } from "@tanstack/store";
import { useIsFocused } from "expo-router";
import { useHeaderHeight } from "expo-router/react-navigation";
import { useRef, useState } from "react";
import { View } from "react-native";

import { MonoText } from "@/components/ui/mono-text";
import { newCommandId } from "@/lib/native/database";
import { EMPTY_STREAM, runningTurn, type StreamState } from "@/lib/stream/reducer";
import { SPACING } from "@/theme/metrics";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import { conversationKey, type ConversationRef } from "@/features/machines/model/machine-ref";
import { isTailnetDown } from "@/features/machines/model/machine-status";
import type { ConversationFocus } from "@/features/navigation/model/routes";
import {
  useDrafts,
  useOutbox,
  usePreferences,
  useRuntime,
} from "@/features/navigation/use-app-services";
import { useHeaderSlot } from "@/features/navigation/use-header-slot";
import { useMachineConnections, useMachines } from "@/features/navigation/use-machines";
import { ApprovalBlock } from "../components/approval-block";
import { DockedComposer } from "../components/composer/composer";
import { Transcript } from "../components/transcript";
import type { TranscriptListHandle } from "../components/transcript-list";
import type { TranscriptActions, TranscriptScope } from "../components/use-transcript-scope";
import { withdrawPromptMutationOptions } from "../conversation-queries";
import { composerDraftKey, outboxTargetOf, type ComposerTarget } from "../model/composer-state";
import { createRowUiStore } from "../model/row-ui-state";
import { outboxPrompts, sameOutboxPrompts } from "../model/screen-outbox";
import type { ComposerDeps } from "../use-composer-actions";
import { useOlderHistory } from "./use-older-history";
import { useRouteConversation } from "./use-route-conversation";
import { useScreenStream } from "./use-screen-stream";

/**
 * The height of the composer until it measured itself: the transcript of the design is
 * anchored 124 pt above the bottom of the screen (design 3.1).
 */
export const COMPOSER_HEIGHT = 124;

/** The stream of a screen that holds none yet: no events, so the loading line shows. */
const NO_STREAM = { store: createStore(EMPTY_STREAM), rowUi: createRowUiStore() };

export interface ConversationScreenProps {
  /** The conversation of the route, or undefined when the params of the link are bad. */
  readonly conversation: ConversationRef | undefined;
  /** `approval`: scroll to the approval that waits. */
  readonly focus?: ConversationFocus | undefined;
}

/** The conversation screen. */
export function ConversationScreen(props: ConversationScreenProps) {
  const runtime = useRuntime();
  const machines = useMachines();
  const conversation = props.conversation;
  if (conversation === undefined) {
    return <Notice text="This link names no conversation." />;
  }
  const machine = machines.find((one) => one.id === conversation.machine);
  const supervisor = runtime.machine(conversation.machine);
  if (machine === undefined || supervisor === undefined) {
    return <Notice text="This machine is not paired with this iPhone." />;
  }
  return (
    <ConversationView
      machine={machine.id}
      routeId={conversation.id}
      machineName={machine.name}
      connection={supervisor.connection}
      focus={props.focus}
    />
  );
}

interface ConversationViewProps {
  readonly machine: string;
  /** The id in the route: a conversation id, or the local id of a new conversation. */
  readonly routeId: string;
  readonly machineName: string;
  readonly connection: Readable<MachineConnection>;
  readonly focus: ConversationFocus | undefined;
}

function ConversationView(props: ConversationViewProps) {
  const { machine, machineName } = props;
  const runtime = useRuntime();
  const outbox = useOutbox();
  const drafts = useDrafts();
  const preferences = usePreferences();
  const route = useRouteConversation(machine, props.routeId, props.focus);
  const ref: ConversationRef = { machine, id: route.id };
  const list = useRef<TranscriptListHandle>(null);
  const focused = useIsFocused();
  const stream = useScreenStream({
    machine,
    id: route.pending ? undefined : route.id,
    focused,
    focus: props.focus,
    list,
  });
  const held = stream ?? NO_STREAM;
  const loadOlder = useOlderHistory(stream, held.store, ref);
  const runningTurnId = useSelector(held.store, (state) => runningTurn(state)?.turnId);
  const pending = useSelector(
    outbox.store,
    (snapshot) => outboxPrompts(snapshot, machine, props.routeId),
    { compare: sameOutboxPrompts },
  );
  const connected = useSelector(
    props.connection,
    (connection) => connection.view.kind === "online",
  );
  const connections = useMachineConnections();
  const tailnetDown = isTailnetDown([...connections.values()]);
  const slot = useHeaderSlot(ref);
  const top = useHeaderHeight() + (slot.kind === "none" ? 0 : SPACING.strip);
  const [composerHeight, setComposerHeight] = useState(COMPOSER_HEIGHT);
  const withdraw = useMutation(withdrawPromptMutationOptions(runtime, ref));

  const target: ComposerTarget = { machine, id: route.id, isBlankRoot: false };
  const deps: ComposerDeps = { caller: runtime, outbox, drafts, preferences, newCommandId };
  const actions: TranscriptActions = {
    withdraw: (turnId) => withdraw.mutate({ commandId: newCommandId(), turnId }),
    // Decision D8: efrd has no retry of a turn, so the prompt goes again as a new one.
    retry: (_turnId, text) => {
      outbox.enqueue({ machine, target: outboxTargetOf(outbox.store.get(), target), text });
    },
    editAndResend: (text) => drafts.set(composerDraftKey(target), text),
    resend: (key) => {
      outbox.retry(key);
    },
  };
  const scope: TranscriptScope = {
    store: held.store,
    rowUi: held.rowUi,
    machineName,
    actions,
    renderApproval: (row) => (
      <ApprovalBlock
        stream={held.store}
        conversation={ref}
        turnId={row.turnId}
        callId={row.callId}
        machineName={machineName}
        connected={connected}
        caller={runtime}
      />
    ),
  };
  const composer: { stream?: Readable<StreamState>; runningTurnId?: string } = {};
  if (stream !== undefined) {
    composer.stream = stream.store;
  }
  if (runningTurnId !== undefined) {
    composer.runningTurnId = runningTurnId;
  }
  return (
    <View className="flex-1 bg-surface-1">
      <Transcript
        key={stream === undefined ? "none" : conversationKey(ref)}
        ref={list}
        scope={scope}
        pending={pending}
        insets={{ top, bottom: composerHeight }}
        onReachTop={loadOlder}
      />
      <View className="absolute right-0 bottom-0 left-0">
        <DockedComposer
          {...composer}
          deps={deps}
          target={target}
          connection={props.connection}
          machineName={machineName}
          tailnetDown={tailnetDown}
          onHeightChange={setComposerHeight}
        />
      </View>
    </View>
  );
}

interface NoticeProps {
  readonly text: string;
}

/** One quiet line under the bar, for a link that the screen cannot show. */
function Notice(props: NoticeProps) {
  const top = useHeaderHeight();
  return (
    <View className="flex-1 bg-surface-1 px-gutter" style={{ paddingTop: top + SPACING.item }}>
      <MonoText size="monoShell" tone="text_muted" accessibilityRole="text">
        {props.text}
      </MonoText>
    </View>
  );
}
