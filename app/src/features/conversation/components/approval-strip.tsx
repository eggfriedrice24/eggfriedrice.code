// The approval strip (boards C-Waiting, S-Approvals and R-Approval, round 8 "minimal"):
// one row in the composer, above the status line. The yolk dot, the command on one line,
// small text only when it matters, then Deny (plain) and Allow (the small filled
// capsule), or "answer on <machine>" for a question that only the terminal can answer.
// model/approval-strip.ts says what each state shows.
//
// - A tap on the row, but not on a button, opens the approval sheet. It never opens by
//   itself.
// - It answers the question that waits first. Once efrd confirms an answer, the next
//   question takes the row ("1 of 3" while more wait).
// - A question of the model (S-Ask) takes the row too: the oldest wait of an approval and
//   a question goes first, and "1 of 2" counts both. A tap on the row or on Answer opens
//   the native action sheet of its options.
// - When no approval and no question of the model waits, a quarantine question of the
//   reviewer off takes the row (Q-Quarantine 1, quarantine-sheet.tsx).
//
// `ApprovalStrip` reads the questions from the stream store and answers through
// `useAnswerApproval`. `ApprovalStripView` draws a model only, for the gallery and tests.

import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";
import { useRef } from "react";
import { Pressable, Text, View, type HostInstance } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { Dot } from "@/components/ui/dot";
import { MonoText } from "@/components/ui/mono-text";
import { SmallFilledButton, SmallPlainButton } from "@/components/ui/small-buttons";
import type { Clock } from "@/lib/net/clock";
import type { MachineCaller } from "@/lib/query/machine-caller";
import type { ApprovalView, StreamState, UserQuestionView } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import type { PhoneDecision } from "../conversation-queries";
import type { ApprovalButtons } from "../model/approval-block";
import { approvalStrip, type ApprovalMeta, type ApprovalStripModel } from "../model/approval-strip";
import { quarantineStrip } from "../model/quarantine";
import { questionStrip, type QuestionStripModel } from "../model/question";
import { comparePendingWaits, selectPendingWaits } from "../model/selectors";
import { useAnswerApproval, useExpiryLabel } from "../use-answer-approval";
import type { AnswerQuestion } from "../use-answer-question";
import { useLeaveQuarantine } from "../use-leave-quarantine";
import { QuarantineStripView } from "./quarantine-sheet";

/**
 * True when `question` waits longer than `approval`, so it takes the row first. Without
 * both times the approval goes first.
 */
function askedFirst(question: UserQuestionView, approval: ApprovalView | undefined): boolean {
  if (approval === undefined) {
    return true;
  }
  return (
    question.askedAt !== undefined &&
    approval.askedAt !== undefined &&
    question.askedAt < approval.askedAt
  );
}

export interface ApprovalStripProps {
  /** The stream store of the conversation. */
  readonly stream: Readable<StreamState>;
  readonly conversation: ConversationRef;
  /** The short name of the machine, such as `desktop`. */
  readonly machineName: string;
  /** The phone has a ready connection to the machine now. */
  readonly connected: boolean;
  /** The runtime, which sends `approval.respond`. */
  readonly caller: MachineCaller;
  /** The clock of the seconds left. The default is the real one. */
  readonly clock?: Clock;
  /** Opens the approval sheet of the question of tool call `callId`. */
  readonly onOpen: (callId: string) => void;
  /**
   * The answers to the questions of the model. Without it the strip shows none: the
   * conversation screen gives it.
   */
  readonly questions?: AnswerQuestion;
}

/** The strip of the question that waits first, or nothing when none waits. */
export function ApprovalStrip(props: ApprovalStripProps) {
  // One walk of the turns per store update gives the three kinds of waits.
  const waits = useSelector(props.stream, selectPendingWaits, { compare: comparePendingWaits });
  const waiting = waits.approvals;
  const quarantine = waits.quarantine;
  const questions = props.questions === undefined ? [] : waits.questions;
  const first = waiting[0];
  const question = questions[0];
  const count = waiting.length + questions.length;
  // The older of an approval and a question of the model comes first; a quarantine
  // question takes the row when neither waits.
  const ask = question !== undefined && askedFirst(question, first) ? question : undefined;
  const asked = first === undefined && question === undefined ? quarantine[0] : undefined;
  const turnId = first?.turnId ?? asked?.turnId;
  const cwd = useSelector(props.stream, (state) =>
    turnId === undefined ? undefined : state.turns.get(turnId)?.cwd,
  );
  const answering = useAnswerApproval({
    caller: props.caller,
    conversation: props.conversation,
    approval: first,
  });
  const expiry = useExpiryLabel(first?.expiresAt, props.clock);
  const leave = useLeaveQuarantine(props.caller, props.conversation);
  const answers = props.questions;
  if (ask !== undefined && answers !== undefined) {
    const model = questionStrip(
      ask,
      {
        connected: props.connected,
        sending: answers.sending === ask.questionId,
        failed: answers.failed === ask.questionId,
      },
      { index: 0, count },
    );
    return model === undefined ? null : (
      <QuestionStripView model={model} onOpen={(anchor) => answers.open(ask, anchor)} />
    );
  }
  if (asked !== undefined) {
    const strip = quarantineStrip(asked, cwd, props.machineName);
    const open = props.onOpen;
    return strip === undefined ? null : (
      <QuarantineStripView
        model={strip}
        leaving={leave.leaving}
        failed={leave.failed}
        offline={!props.connected}
        onOpen={() => open(strip.callId)}
        onLeave={() => leave.leave(strip.questionId)}
      />
    );
  }
  if (first === undefined) {
    return null;
  }
  const model = approvalStrip(
    first,
    {
      machine: props.machineName,
      cwd,
      connected: props.connected,
      expiry,
      answer: answering.state,
    },
    { index: 0, count },
  );
  if (model === undefined) {
    return null;
  }
  const onOpen = props.onOpen;
  return (
    <ApprovalStripView
      model={model}
      onOpen={() => onOpen(model.callId)}
      onAnswer={answering.answer}
    />
  );
}

export interface ApprovalStripViewProps {
  readonly model: ApprovalStripModel;
  readonly onOpen: () => void;
  readonly onAnswer: (decision: PhoneDecision) => void;
}

/** The strip of a model. */
export function ApprovalStripView(props: ApprovalStripViewProps) {
  const { model } = props;
  return (
    <View testID="approval-strip" className="min-h-44 flex-row items-center gap-4 pr-6 pl-16">
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={model.label}
        accessibilityHint="Opens the approval."
        onPress={props.onOpen}
        className="min-h-44 flex-1 flex-row items-center gap-8 active:opacity-60"
      >
        <Dot size="large" />
        <Text numberOfLines={1} className="flex-1 text-mono-sm text-text-bright">
          {model.line}
        </Text>
        <MetaTexts meta={model.meta} />
      </Pressable>
      {model.terminal === undefined ? null : (
        <MonoText tone="accent_ui" className="pr-6">
          {model.terminal}
        </MonoText>
      )}
      {model.buttons === undefined ? null : (
        <AnswerButtons buttons={model.buttons} onAnswer={props.onAnswer} />
      )}
    </View>
  );
}

export interface QuestionStripViewProps {
  readonly model: QuestionStripModel;
  /** Opens the action sheet, out of `anchor` (the strip). */
  readonly onOpen: (anchor: HostInstance | null) => void;
}

/**
 * The strip of a question of the model (S-Ask 1): the yolk dot, the question on one line
 * in SF Pro 13, the small texts, then Answer, the small filled capsule. A tap on the row
 * or on Answer opens the native action sheet.
 */
export function QuestionStripView(props: QuestionStripViewProps) {
  const { model } = props;
  const anchor = useRef<HostInstance>(null);
  const open = (): void => props.onOpen(anchor.current);
  return (
    <View
      ref={anchor}
      testID="question-strip"
      className="min-h-44 flex-row items-center gap-4 pr-6 pl-16"
    >
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={model.label}
        accessibilityHint="Opens the choices."
        disabled={!model.enabled}
        onPress={open}
        className="min-h-44 flex-1 flex-row items-center gap-8 active:opacity-60"
      >
        <Dot size="large" />
        <BodyText size="rowDetail" tone="text_primary" numberOfLines={1} className="flex-1">
          {model.line}
        </BodyText>
        <MetaTexts meta={model.meta} />
      </Pressable>
      <SmallFilledButton
        label="Answer"
        pending={model.sending}
        pendingLabel="Sending the answer"
        disabled={!model.enabled && !model.sending}
        onPress={open}
      />
    </View>
  );
}

/** The small texts of an open question: mono 11, each in its role. */
export function MetaTexts(props: { readonly meta: readonly ApprovalMeta[] }) {
  return props.meta.map((part) => (
    <MonoText key={part.key} tone={part.tone} numberOfLines={1}>
      {part.text}
    </MonoText>
  ));
}

export interface AnswerButtonsProps {
  readonly buttons: ApprovalButtons;
  readonly onAnswer: (decision: PhoneDecision) => void;
}

/** Deny (plain) and Allow (filled), on the thumb side. */
export function AnswerButtons(props: AnswerButtonsProps) {
  const { enabled, sending } = props.buttons;
  return (
    <View className="flex-row items-center gap-4">
      <SmallPlainButton
        label="Deny"
        pending={sending === "deny"}
        pendingLabel="Denying"
        disabled={!enabled && sending !== "deny"}
        onPress={() => props.onAnswer("deny")}
      />
      <SmallFilledButton
        label="Allow"
        pending={sending === "allow"}
        pendingLabel="Allowing"
        disabled={!enabled && sending !== "allow"}
        onPress={() => props.onAnswer("allow")}
      />
    </View>
  );
}
