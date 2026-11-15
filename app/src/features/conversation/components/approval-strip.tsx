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
// - When no approval waits, a quarantine question of the reviewer off takes the row
//   (Q-Quarantine 1, quarantine-sheet.tsx).
//
// `ApprovalStrip` reads the questions from the stream store and answers through
// `useAnswerApproval`. `ApprovalStripView` draws a model only, for the gallery and tests.

import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";
import { Pressable, Text, View } from "react-native";

import { Dot } from "@/components/ui/dot";
import { MonoText } from "@/components/ui/mono-text";
import { SmallFilledButton, SmallPlainButton } from "@/components/ui/small-buttons";
import type { Clock } from "@/lib/net/clock";
import type { MachineCaller } from "@/lib/query/machine-caller";
import type { StreamState } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import type { PhoneDecision } from "../conversation-queries";
import type { ApprovalButtons } from "../model/approval-block";
import { approvalStrip, type ApprovalMeta, type ApprovalStripModel } from "../model/approval-strip";
import { quarantineStrip } from "../model/quarantine";
import {
  compareApprovals,
  compareQuarantine,
  selectPendingApprovals,
  selectPendingQuarantine,
} from "../model/selectors";
import { useAnswerApproval, useExpiryLabel } from "../use-answer-approval";
import { QuarantineStripView } from "./quarantine-sheet";

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
}

/** The strip of the question that waits first, or nothing when none waits. */
export function ApprovalStrip(props: ApprovalStripProps) {
  const waiting = useSelector(props.stream, selectPendingApprovals, {
    compare: compareApprovals,
  });
  const quarantine = useSelector(props.stream, selectPendingQuarantine, {
    compare: compareQuarantine,
  });
  const first = waiting[0];
  // An approval comes first; a quarantine question takes the row when none waits.
  const asked = first === undefined ? quarantine[0] : undefined;
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
  if (asked !== undefined) {
    const strip = quarantineStrip(asked, cwd, props.machineName);
    const open = props.onOpen;
    return strip === undefined ? null : (
      <QuarantineStripView model={strip} onOpen={() => open(strip.callId)} />
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
    { index: 0, count: waiting.length },
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
