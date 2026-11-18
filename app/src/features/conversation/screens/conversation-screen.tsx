// One conversation of one machine, pushed on the stack (plan sections 2.2 and 2.5, design
// boards R-Streaming, R-Approval, R-Other, C-Offline, C-Reconnecting, C-Failed and C-Jump).
//
// - The transcript of the stream, with the prompts of the outbox after it, under the
//   transparent bar. The content clears the bar, and the strip while the slot under the
//   bar shows (an approval elsewhere, or the offline line).
// - The docked composer at the bottom. Its height is the bottom inset of the transcript,
//   so "Jump to latest" sits 12 pt above it. While a question waits, the approval strip
//   is its first row; a tap on the strip or on the waiting tool card opens the approval
//   sheet (C-Waiting, R-Approval). Nothing opens it by itself.
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

import { useMutation, useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import { createStore, type Readable } from "@tanstack/store";
import { useIsFocused, useRouter } from "expo-router";
import { useHeaderHeight } from "expo-router/react-navigation";
import { useRef, useState } from "react";
import { View } from "react-native";

import { MonoText } from "@/components/ui/mono-text";
import { newCommandId } from "@/lib/native/database";
import { EMPTY_STREAM, runningTurn, type StreamState } from "@/lib/stream/reducer";
import { SPACING } from "@/theme/metrics";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { isTailnetDown } from "@/features/machines/model/machine-status";
import { selectLockOn } from "@/features/navigation/model/app-lock";
import {
  approvalHref,
  type ConversationFocus,
  modelSheetHref,
  type ModelSheetTarget,
} from "@/features/navigation/model/routes";
import {
  useDrafts,
  useAppLock,
  useAttach,
  useModelChoices,
  useOutbox,
  usePreferences,
  useRuntime,
  useVoice,
} from "@/features/navigation/use-app-services";
import { useHeaderSlot } from "@/features/navigation/use-header-slot";
import { useMachineConnections, useMachines } from "@/features/navigation/use-machines";
import { ApprovalStrip } from "../components/approval-strip";
import { DockedComposer } from "../components/composer/composer";
import type { ComposerAnswer } from "../components/composer/composer-context";
import { Transcript } from "../components/transcript";
import type { TranscriptListHandle } from "../components/transcript-list";
import type { TranscriptActions, TranscriptScope } from "../components/use-transcript-scope";
import { modelsQueryOptions, withdrawPromptMutationOptions } from "../conversation-queries";
import { composerDraftKey, type ComposerTarget } from "../model/composer-state";
import { createRowUiStore } from "../model/row-ui-state";
import { canAnswer, questionLine } from "../model/question";
import { outboxPrompts, sameOutboxPrompts } from "../model/screen-outbox";
import { selectWaitingQuestion } from "../model/selectors";
import { sendPrompt } from "../model/send-prompt";
import { useAnswerQuestion } from "../use-answer-question";
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
  const modelChoices = useModelChoices();
  const appLock = useAppLock();
  const attach = useAttach();
  const voice = useVoice();
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
  // The capsule under the bar floats 8 pt under it, so the content clears both.
  const top = useHeaderHeight() + (slot.kind === "none" ? 0 : SPACING.float + SPACING.strip);
  const [composerHeight, setComposerHeight] = useState(COMPOSER_HEIGHT);
  // Short local UI state: the question of the model that the composer answers after
  // "Type an answer" (S-Ask 3). The state ends by itself once the question ends. The
  // typed answer has a store of its own, not the message draft, so typing renders only
  // the composer and the text goes when the question ends (ADR 0025).
  const [typing, setTyping] = useState<string | undefined>(undefined);
  const [answerDraft] = useState(() => createStore(""));
  const answers = useAnswerQuestion(runtime, ref, (question) => {
    answerDraft.setState(() => "");
    setTyping(question.questionId);
  });
  const answered = useSelector(held.store, selectWaitingQuestion(typing));
  // Only `mutate` is stable: the whole result of useMutation is new on each render.
  const { mutate: withdraw } = useMutation(withdrawPromptMutationOptions(runtime, ref));
  const models = useQuery(modelsQueryOptions(runtime, machine)).data;
  const router = useRouter();
  // The sheet opens only from a tap on the strip or on the waiting card (round 8).
  const openApproval = (callId: string): void => {
    router.push(approvalHref(ref, callId));
  };

  const target: ComposerTarget = { machine, id: route.id, isBlankRoot: false };
  const deps: ComposerDeps = {
    caller: runtime,
    outbox,
    drafts,
    preferences,
    modelChoices,
    newCommandId,
    lock: appLock,
    attach,
    voice,
  };
  const openModels = (select?: { readonly model: string; readonly provider?: string }): void => {
    const sheet: { -readonly [K in keyof ModelSheetTarget]: ModelSheetTarget[K] } = {
      machine,
      id: route.id,
      isBlankRoot: false,
    };
    if (select !== undefined) {
      sheet.select = select;
    }
    router.push(modelSheetHref(sheet));
  };
  const actions: TranscriptActions = {
    withdraw: (turnId) => withdraw({ commandId: newCommandId(), turnId }),
    // Decision D8: efrd has no retry of a turn, so the prompt goes again as a new one,
    // by the rule of a send: the model that the sheet chose since, and the mode now.
    retry: (_turnId, text) => {
      const lockOn = selectLockOn(appLock.store.get());
      sendPrompt({ outbox, modelChoices, lockOn }, models, target, text);
    },
    editAndResend: (text) => drafts.set(composerDraftKey(target), text),
    resend: (key) => {
      outbox.retry(key);
    },
    openApproval,
    openModels: (model, provider) => openModels({ model, provider }),
  };
  const scope: { -readonly [K in keyof TranscriptScope]: TranscriptScope[K] } = {
    store: held.store,
    rowUi: held.rowUi,
    machineName,
    actions,
    attach,
    choice: { store: modelChoices.store, key: composerDraftKey(target) },
    questions: {
      open: answers.open,
      sending: answers.sending,
      enabled: (questionId) => canAnswer(connected, answers.sending === questionId),
    },
  };
  if (models !== undefined) {
    scope.catalog = { models: models.models, catalogs: models.catalogs ?? [] };
  }
  const strip =
    stream === undefined ? undefined : (
      <ApprovalStrip
        stream={stream.store}
        conversation={ref}
        machineName={machineName}
        connected={connected}
        caller={runtime}
        onOpen={openApproval}
        questions={answers}
      />
    );
  const composer: {
    stream?: Readable<StreamState>;
    runningTurnId?: string;
    answering?: ComposerAnswer;
  } = {};
  if (stream !== undefined) {
    composer.stream = stream.store;
  }
  if (runningTurnId !== undefined) {
    composer.runningTurnId = runningTurnId;
  }
  if (answered !== undefined) {
    const questionId = answered.questionId;
    composer.answering = {
      question: questionLine(answered),
      draft: answerDraft,
      setText: (text) => answerDraft.setState(() => text),
      sending: answers.sending === questionId,
      failed: answers.failed === questionId,
      send: (text) =>
        answers.answer(questionId, { kind: "text", text }, () => answerDraft.setState(() => "")),
      // NOTE: only the xmark, a choice of the user, keeps the typed text as a message.
      leave: () => {
        const kept = answerDraft.state;
        if (kept.length > 0) {
          const key = composerDraftKey(target);
          const draft = drafts.text(key);
          drafts.set(key, draft.length > 0 ? `${draft} ${kept}` : kept);
        }
        answerDraft.setState(() => "");
        setTyping(undefined);
      },
    };
  }
  return (
    <View className="flex-1 bg-surface-1">
      {/* The transcript comes first, and keeps its scroll view while the screen shows.
          react-native-screens finds the scroll view of a screen through the first child
          of each view, and only when the screen appears, to set the scroll edge effects
          of iOS 26 on it. A view before it, or a new scroll view later, would keep the
          default effects of iOS on the list. */}
      <Transcript
        ref={list}
        scope={scope}
        pending={pending}
        insets={{ top, bottom: composerHeight }}
        onReachTop={loadOlder}
      />
      {/* The wrapper fills the screen, so the composer stays inside it when
          KeyboardStickyView moves it up over the keyboard. Fabric hit-tests a view with
          no layout overflow as if it clipped, so a shorter wrapper would drop every tap
          on the moved composer. box-none lets the other touches through. */}
      <View pointerEvents="box-none" className="absolute inset-0 justify-end">
        <DockedComposer
          {...composer}
          strip={strip}
          deps={deps}
          target={target}
          connection={props.connection}
          machineName={machineName}
          tailnetDown={tailnetDown}
          onHeightChange={setComposerHeight}
          onOpenModels={() => openModels()}
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
