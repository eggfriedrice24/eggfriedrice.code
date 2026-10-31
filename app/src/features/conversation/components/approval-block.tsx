// The approval block of the transcript (design 3.11, board S-Approvals): "the only gold
// outline in the app", and only while an answer is still possible.
// model/approval-block.ts says what each state shows; this file draws it.
//
// `ApprovalBlock` reads its question from the stream store of the conversation and
// answers through `useAnswerApproval`. `ApprovalBlockView` draws a model only, so the dev
// gallery and the tests can show every state without a machine.

import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";
import { Text, View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { BorderedButton } from "@/components/ui/bordered-button";
import { cn } from "@/components/ui/cn";
import { Dot } from "@/components/ui/dot";
import { MonoText } from "@/components/ui/mono-text";
import { PrimaryButton } from "@/components/ui/primary-button";
import { SymbolIcon } from "@/components/ui/symbol";
import { TYPE_CLASS } from "@/components/ui/text-classes";
import type { Clock } from "@/lib/net/clock";
import type { MachineCaller } from "@/lib/query/machine-caller";
import type { StreamState } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import type { PhoneDecision } from "../conversation-queries";
import { selectApproval } from "../model/selectors";
import { useAnswerApproval, useExpiryLabel } from "../use-answer-approval";
import { ApprovalDiff } from "./approval-block-diff";
import {
  approvalBlock,
  type ApprovalBlockModel,
  type ApprovalButtons,
  type ClosedApproval,
  type OpenApproval,
} from "../model/approval-block";
import type { ReasonSpan, Subject } from "../model/approval-block-text";

export interface ApprovalBlockProps {
  /** The stream store of the conversation. */
  readonly stream: Readable<StreamState>;
  readonly conversation: ConversationRef;
  readonly turnId: string;
  readonly callId: string;
  /** The short name of the machine, such as `desktop`. */
  readonly machineName: string;
  /** The phone has a ready connection to the machine now. */
  readonly connected: boolean;
  /** The runtime, which sends `approval.respond`. */
  readonly caller: MachineCaller;
  /** The clock of the countdown. The default is the real one. */
  readonly clock?: Clock;
  /**
   * The tool block of the call is right above, joined by a line: the open block does
   * not repeat the command (design R4 item 5). The sheet and the strip still show it.
   */
  readonly linked?: boolean;
}

/** The block of the approval `callId` of turn `turnId`. */
export function ApprovalBlock(props: ApprovalBlockProps) {
  const { turnId, callId } = props;
  const approval = useSelector(props.stream, selectApproval(turnId, callId));
  const cwd = useSelector(props.stream, (state) => state.turns.get(turnId)?.cwd);
  const answering = useAnswerApproval({
    caller: props.caller,
    conversation: props.conversation,
    approval,
  });
  const expiry = useExpiryLabel(
    approval?.status === "pending" ? approval.expiresAt : undefined,
    props.clock,
  );
  if (approval === undefined) {
    return null;
  }
  const model = approvalBlock(approval, {
    machine: props.machineName,
    cwd,
    connected: props.connected,
    expiry,
    answer: answering.state,
  });
  return (
    <ApprovalBlockView model={model} onAnswer={answering.answer} linked={props.linked === true} />
  );
}

export interface ApprovalBlockViewProps {
  readonly model: ApprovalBlockModel;
  readonly onAnswer: (decision: PhoneDecision) => void;
  /** The tool block above shows the command: the open block leaves it out. */
  readonly linked?: boolean;
}

/** The block of a model. */
export function ApprovalBlockView(props: ApprovalBlockViewProps) {
  const model = props.model;
  return model.kind === "open" ? (
    <OpenBlock model={model} onAnswer={props.onAnswer} linked={props.linked === true} />
  ) : (
    <ClosedBlock model={model} />
  );
}

interface OpenBlockProps {
  readonly model: OpenApproval;
  readonly onAnswer: (decision: PhoneDecision) => void;
  readonly linked: boolean;
}

function OpenBlock(props: OpenBlockProps) {
  const { model } = props;
  const terminal = model.terminal;
  return (
    <View
      testID="approval-block"
      role="group"
      accessibilityLabel={model.label}
      className="rounded-box border border-border-accent p-12"
    >
      <View className="flex-row items-center gap-7">
        <Dot size="large" />
        <MonoText tone="accent_primary" className="flex-1">
          needs approval
        </MonoText>
        {model.expiry === undefined ? null : <MonoText>{model.expiry}</MonoText>}
      </View>
      {props.linked ? null : <SubjectText subject={model.subject} />}
      {props.linked || model.cwd === undefined ? null : (
        <MonoText className="mt-2">in {model.cwd}</MonoText>
      )}
      {model.details.map((detail) => (
        <MonoText key={detail} className="mt-2">
          {detail}
        </MonoText>
      ))}
      {model.diff === undefined ? null : <ApprovalDiff rows={model.diff} />}
      {model.reason === undefined ? null : <Reason spans={model.reason} />}
      {terminal === undefined ? null : (
        <>
          <View className="mt-10 flex-row items-center gap-6">
            <SymbolIcon name="terminal" size={13} tone="accent_ui" />
            <MonoText tone="accent_ui">{terminal.line}</MonoText>
          </View>
          <BodyText size="reason" className="mt-4">
            {terminal.note}
          </BodyText>
        </>
      )}
      {model.error === undefined ? null : (
        <View className="mt-10 flex-row gap-6" role="alert">
          <MonoText tone="state_error">✗</MonoText>
          <MonoText tone="text_muted" className="flex-1">
            {model.error}
          </MonoText>
        </View>
      )}
      {model.hint === undefined ? null : <MonoText className="mt-10">{model.hint}</MonoText>}
      {model.buttons === undefined ? null : (
        <AnswerButtons buttons={model.buttons} onAnswer={props.onAnswer} />
      )}
    </View>
  );
}

/** The command line: `$ <line>` or `edit <path> +2 −1`, wrapped anywhere, never cut. */
function SubjectText(props: { readonly subject: Subject }) {
  const subject = props.subject;
  return (
    <Text className={cn("mt-10", TYPE_CLASS.monoCommand, "text-text-bright")}>
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

/** The reason line in SF Pro 13/18, with mode names and paths in mono 12. */
function Reason(props: { readonly spans: readonly ReasonSpan[] }) {
  return (
    <BodyText size="reason" className="mt-10">
      {props.spans.map((span) =>
        span.mono ? (
          <Text key={span.at} className={cn(TYPE_CLASS.monoCode, "text-text-muted")}>
            {span.text}
          </Text>
        ) : (
          span.text
        ),
      )}
    </BodyText>
  );
}

interface AnswerButtonsProps {
  readonly buttons: ApprovalButtons;
  readonly onAnswer: (decision: PhoneDecision) => void;
}

/** Deny on the left, Allow on the thumb side (design 3.11). */
function AnswerButtons(props: AnswerButtonsProps) {
  const { enabled, sending } = props.buttons;
  return (
    <View className="mt-14 flex-row gap-10">
      <BorderedButton
        label={sending === "deny" ? "Denying" : "Deny"}
        size="medium"
        grow
        disabled={!enabled}
        onPress={() => props.onAnswer("deny")}
      />
      <PrimaryButton
        label="Allow"
        size="medium"
        grow
        pending={sending === "allow"}
        pendingLabel="Allowing"
        disabled={!enabled && sending !== "allow"}
        onPress={() => props.onAnswer("allow")}
      />
    </View>
  );
}

const GLYPH_TONE = {
  allowed: "state_success",
  denied: "state_error",
} as const;

/** The quiet line of a closed question: glyph, what happened and when, then the command. */
function ClosedBlock(props: { readonly model: ClosedApproval }) {
  const { model } = props;
  return (
    <View testID="approval-block" accessible accessibilityLabel={model.label} className="gap-2">
      <View className="flex-row items-center gap-6">
        {model.glyph === "expired" ? (
          <SymbolIcon name="clock" size={12} tone="text_subtle" weight="semibold" />
        ) : (
          <MonoText tone={GLYPH_TONE[model.glyph]}>
            {model.glyph === "allowed" ? "✓" : "✗"}
          </MonoText>
        )}
        <MonoText tone="text_muted" className="flex-1">
          {model.text}
        </MonoText>
      </View>
      <MonoText numberOfLines={1}>{model.subject}</MonoText>
    </View>
  );
}
