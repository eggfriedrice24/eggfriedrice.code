// The approval sheet (board AP-Sheets and S-Approvals, round 8 "minimal"): no title, no
// labels, no boxes. The full command in mono 13/19, wrapped and never cut (or the edit
// line and the diff, full width with no frame), one quiet line in 13/18 text_subtle, then
// the answer row: small text on the left only when it matters, Deny and Allow on the
// right, or "answer on <machine>" for a question that only the terminal can answer.
// model/approval-strip.ts says what each state shows.
//
// - It opens only from a tap on the strip or on the waiting tool card.
// - When efrd takes an answer of this sheet, the sheet closes.
// - A question that closed while the sheet is open dims and says how it ended and how
//   many more wait. Close and Next are items of the native header of the sheet
//   (screens/approval-sheet-screen.tsx); Deny and Allow, the decision of the sheet, stay
//   large buttons in the sheet.
//
// `ApprovalSheet` reads the question from the stream store; `ApprovalSheetView` draws a
// model only, for the gallery and the tests.

import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";
import { ScrollView, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { BodyText } from "@/components/ui/body-text";
import { cn } from "@/components/ui/cn";
import { MonoText } from "@/components/ui/mono-text";
import type { Clock } from "@/lib/net/clock";
import type { MachineCaller } from "@/lib/query/machine-caller";
import { findApproval, findQuarantine, type StreamState } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import type { PhoneDecision } from "../conversation-queries";
import type { Subject } from "../model/approval-block-text";
import { approvalSheet, type ApprovalSheetModel } from "../model/approval-strip";
import { compareApprovals, selectPendingApprovals } from "../model/selectors";
import { useAnswerApproval, useExpiryLabel } from "../use-answer-approval";
import { ApprovalDiff } from "./approval-block-diff";
import { AnswerButtons, MetaTexts } from "./approval-strip";
import { QuarantineSheet } from "./quarantine-sheet";

/** The smallest space under the answer row, on a screen without a home indicator. */
const MIN_BOTTOM = 12;

export interface ApprovalSheetProps {
  /** The stream store of the conversation. */
  readonly stream: Readable<StreamState>;
  readonly conversation: ConversationRef;
  /** The tool call whose question the sheet shows. */
  readonly callId: string;
  /** The short name of the machine, such as `desktop`. */
  readonly machineName: string;
  /** The phone has a ready connection to the machine now. */
  readonly connected: boolean;
  /** The runtime, which sends `approval.respond`. */
  readonly caller: MachineCaller;
  /** The clock of the seconds left. The default is the real one. */
  readonly clock?: Clock;
  /** Closes the sheet, after efrd took an answer. */
  readonly onClose: () => void;
}

/** The sheet of the question of `callId`. */
export function ApprovalSheet(props: ApprovalSheetProps) {
  const { callId } = props;
  const approval = useSelector(props.stream, (state) => findApproval(state, callId));
  const asksQuarantine = useSelector(
    props.stream,
    (state) => findQuarantine(state, callId) !== undefined,
  );
  const waiting = useSelector(props.stream, selectPendingApprovals, {
    compare: compareApprovals,
  });
  const turnId = approval?.turnId;
  const cwd = useSelector(props.stream, (state) =>
    turnId === undefined ? undefined : state.turns.get(turnId)?.cwd,
  );
  const answering = useAnswerApproval({
    caller: props.caller,
    conversation: props.conversation,
    approval,
    onAnswered: props.onClose,
  });
  const expiry = useExpiryLabel(
    approval?.status === "pending" ? approval.expiresAt : undefined,
    props.clock,
  );
  if (approval === undefined) {
    // The waiting card or the strip of a quarantine question opens this sheet too.
    return asksQuarantine ? (
      <QuarantineSheet
        stream={props.stream}
        callId={callId}
        machineName={props.machineName}
        connected={props.connected}
        conversation={props.conversation}
        caller={props.caller}
      />
    ) : (
      <GoneSheet />
    );
  }
  const model = approvalSheet(
    approval,
    {
      machine: props.machineName,
      cwd,
      connected: props.connected,
      expiry,
      answer: answering.state,
    },
    waiting,
  );
  return <ApprovalSheetView model={model} onAnswer={answering.answer} />;
}

export interface ApprovalSheetViewProps {
  readonly model: ApprovalSheetModel;
  readonly onAnswer: (decision: PhoneDecision) => void;
}

/** The sheet of a model. */
export function ApprovalSheetView(props: ApprovalSheetViewProps) {
  const { model } = props;
  const bottom = Math.max(useSafeAreaInsets().bottom, MIN_BOTTOM);
  const open = model.open;
  const closed = model.closed;
  return (
    <View testID="approval-sheet" role="dialog" aria-label="Approval" className="bg-surface-1">
      <ScrollView bounces={false} className="grow-0" contentContainerClassName="px-22 pt-14">
        <View className={cn(closed !== undefined && "opacity-50")}>
          <SubjectText subject={model.subject} />
          {model.diff === undefined ? null : <ApprovalDiff rows={model.diff} bare />}
          {model.quiet === undefined ? null : (
            <BodyText size="reason" tone="text_subtle" className="mt-8">
              {model.quiet}
            </BodyText>
          )}
          {model.notes.map((note) => (
            <MonoText key={note} className="mt-4">
              {note}
            </MonoText>
          ))}
        </View>
      </ScrollView>
      {open?.error === undefined ? null : (
        <View className="mt-10 flex-row gap-6 px-22" role="alert">
          <MonoText tone="state_error">✗</MonoText>
          <MonoText tone="text_muted" className="flex-1">
            {open.error}
          </MonoText>
        </View>
      )}
      <View
        className="min-h-44 flex-row items-center gap-8 pt-8 pr-14 pl-22"
        style={{ paddingBottom: bottom }}
      >
        {open === undefined ? null : <MetaTexts meta={open.meta} />}
        {closed === undefined
          ? null
          : closed.parts.map((part, index) => (
              <MonoText key={part}>{index === 0 ? part : `· ${part}`}</MonoText>
            ))}
        <View className="flex-1" />
        {open?.terminal === undefined ? null : (
          <MonoText tone="accent_ui">{open.terminal}</MonoText>
        )}
        {open?.buttons === undefined ? null : (
          <AnswerButtons buttons={open.buttons} onAnswer={props.onAnswer} />
        )}
      </View>
    </View>
  );
}

/** The command: `$ <line>` or `edit <path> +2 −1`, wrapped anywhere, never cut. */
function SubjectText(props: { readonly subject: Subject }) {
  const subject = props.subject;
  return (
    <Text className="text-mono-shell text-text-bright">
      {subject.kind === "command" ? (
        <>
          <Text className="text-text-subtle">$ </Text>
          {subject.line}
        </>
      ) : subject.kind === "edit" ? (
        <>
          <Text className="text-text-subtle">edit </Text>
          {subject.path} <Text className="text-diff-added">+{subject.added}</Text>{" "}
          <Text className="text-diff-removed">
            {"−"}
            {subject.removed}
          </Text>
        </>
      ) : (
        subject.text
      )}
    </Text>
  );
}

/** The sheet of a question that the stream does not hold (any more). Close is in the header. */
function GoneSheet() {
  const bottom = Math.max(useSafeAreaInsets().bottom, MIN_BOTTOM);
  return (
    <View
      testID="approval-sheet"
      className="min-h-44 flex-row items-center gap-8 bg-surface-1 pt-14 pr-14 pl-22"
      style={{ paddingBottom: bottom }}
    >
      <MonoText className="flex-1">The question is not open now.</MonoText>
    </View>
  );
}
