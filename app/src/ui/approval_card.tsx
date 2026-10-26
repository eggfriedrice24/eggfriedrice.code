// An approval question of the transcript (approval.ts says what it shows). The phone
// answers with `approval.respond`; efrd's first answer wins, so the card waits for the
// `approval_resolved` event and does not guess. A tap gives a light haptic, a confirmed
// answer a success haptic, and a failed one an error haptic with the reason.

import * as Haptics from "expo-haptics";
import { memo, useCallback, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { semantic } from "@/theme/palette";

import { type ApprovalButton, approvalCard, nextTick, type PhoneDecision } from "./approval";
import type { ApprovalView, TranscriptDetails } from "./details";
import { DiffView } from "./diff_view";
import type { ExternalStore } from "@/lib/store/frame-store";
import { useStoreSelector, useWallClock } from "./hooks";
import { useSession } from "./session_context";
import { BUTTON, BUTTON_OFF, BUTTON_QUIET, BUTTON_QUIET_OFF, common } from "./styles";

/** The card of the question of call `callId`. */
export const ApprovalCard = memo(function ApprovalCard(props: {
  readonly details: ExternalStore<TranscriptDetails>;
  readonly conversationId: string;
  readonly turnId: string;
  readonly callId: string;
  readonly connected: boolean;
}) {
  const { turnId, callId } = props;
  const approval = useStoreSelector(
    props.details,
    useCallback(
      (state: TranscriptDetails) => state.turns.get(turnId)?.approvals.get(callId),
      [turnId, callId],
    ),
  );
  if (approval === undefined) {
    return null;
  }
  return (
    <ApprovalBody
      approval={approval}
      conversationId={props.conversationId}
      connected={props.connected}
    />
  );
});

function ApprovalBody(props: {
  readonly approval: ApprovalView;
  readonly conversationId: string;
  readonly connected: boolean;
}) {
  const { approval, conversationId } = props;
  const session = useSession();
  const [sending, setSending] = useState<PhoneDecision | undefined>(undefined);
  const [error, setError] = useState<string | undefined>(undefined);
  const now = useWallClock(useCallback((at: number) => nextTick(approval, at), [approval]));
  const card = approvalCard(approval, now, {
    connected: props.connected,
    ...(approval.state === "pending" && sending !== undefined ? { sending } : {}),
    ...(error === undefined ? {} : { error }),
  });

  const answer = useCallback(
    async (decision: PhoneDecision) => {
      setSending(decision);
      setError(undefined);
      void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light).catch(() => {});
      let message: string | undefined;
      try {
        const result = await session.answerApproval(conversationId, approval.callId, decision);
        message = result.ok ? undefined : result.message;
      } catch {
        message = "The answer did not reach your computer.";
      }
      setSending(undefined);
      setError(message);
      void Haptics.notificationAsync(
        message === undefined
          ? Haptics.NotificationFeedbackType.Success
          : Haptics.NotificationFeedbackType.Error,
      ).catch(() => {});
    },
    [session, conversationId, approval.callId],
  );

  return (
    <View
      style={approval.state === "pending" ? styles.pending : styles.card}
      accessibilityLabel={card.label}
    >
      <Text style={TITLE[card.tone]} accessibilityRole="header">
        {card.title}
      </Text>
      <Text style={styles.summary}>{approval.summary}</Text>
      {approval.reason === undefined ? null : (
        <Text style={common.muted}>The model says: {approval.reason}</Text>
      )}
      {approval.diffPreview === undefined ? null : <DiffView diff={approval.diffPreview} />}
      {card.notes.map((note) => (
        <Text key={note} style={common.muted}>
          {note}
        </Text>
      ))}
      {card.countdown === undefined ? null : <Text style={styles.countdown}>{card.countdown}</Text>}
      {card.buttons.length === 0 ? null : (
        <View style={styles.buttons}>
          {card.buttons.map((button) => (
            <AnswerButton key={button.decision} button={button} onAnswer={answer} />
          ))}
        </View>
      )}
    </View>
  );
}

function AnswerButton(props: {
  readonly button: ApprovalButton;
  readonly onAnswer: (decision: PhoneDecision) => Promise<void>;
}) {
  const { button, onAnswer } = props;
  const allow = button.decision === "allow";
  const press = useCallback(() => {
    void onAnswer(button.decision);
  }, [onAnswer, button.decision]);
  const look = allow
    ? button.enabled
      ? BUTTON
      : BUTTON_OFF
    : button.enabled
      ? BUTTON_QUIET
      : BUTTON_QUIET_OFF;
  return (
    <Pressable
      onPress={press}
      disabled={!button.enabled}
      accessibilityRole="button"
      accessibilityState={button.enabled ? ENABLED : DISABLED}
      style={look}
    >
      <Text style={allow ? common.buttonText : common.buttonQuietText}>{button.label}</Text>
    </Pressable>
  );
}

const ENABLED = { disabled: false } as const;
const DISABLED = { disabled: true } as const;

const CARD = {
  backgroundColor: semantic.surface_2,
  borderColor: semantic.border_default,
  borderWidth: StyleSheet.hairlineWidth,
  borderRadius: 10,
  padding: 12,
  gap: 8,
  marginVertical: 6,
} as const;

const styles = StyleSheet.create({
  card: CARD,
  pending: { ...CARD, borderColor: semantic.border_accent, borderWidth: 1 },
  summary: { color: semantic.text_bright, fontSize: 16, lineHeight: 22 },
  countdown: { color: semantic.state_warning, fontSize: 13 },
  buttons: { flexDirection: "row", gap: 10, justifyContent: "flex-end" },
});

const TITLE_BASE = { fontSize: 15, fontWeight: "700" } as const;

const TITLE = StyleSheet.create({
  warning: { ...TITLE_BASE, color: semantic.state_warning },
  muted: { ...TITLE_BASE, color: semantic.text_muted },
  success: { ...TITLE_BASE, color: semantic.state_success },
  error: { ...TITLE_BASE, color: semantic.state_error },
});
