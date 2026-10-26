// The rows of the transcript list, by kind (rows.ts). Each row reads only its own part
// from the stores, through a selector, so it renders only when that part changes. The
// stream store that the rows other than messages read is frame-gated: it changes at
// most once per frame.

import { memo, useCallback, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { StreamState } from "../core/reducer";
import { semantic } from "@/theme/palette";

import { ApprovalCard } from "./approval_card";
import type { TranscriptDetails } from "./details";
import type { ExternalStore } from "./external_store";
import { useStore, useStoreSelector } from "./hooks";
import { MessageRow } from "./message_row";
import type { Row } from "./rows";
import { endLine, turnStatus } from "./status";
import { COLLAPSED, EXPANDED, GAUGE } from "./styles";
import { ToolCard } from "./tool_card";

/** The stores and facts that every row of one transcript reads. */
export interface RowContext {
  readonly conversationId: string;
  /** The stream store itself, for the message rows and their pacer. */
  readonly stream: ExternalStore<StreamState>;
  /** The stream store, frame-gated, for the other rows. */
  readonly gatedStream: ExternalStore<StreamState>;
  readonly details: ExternalStore<TranscriptDetails>;
  readonly connected: boolean;
  readonly reduceMotion: boolean;
}

/** One row. */
export const TranscriptRow = memo(function TranscriptRow(props: {
  readonly row: Row;
  readonly context: RowContext;
}) {
  const { row, context } = props;
  switch (row.kind) {
    case "prompt":
      return <PromptRow details={context.details} turnId={row.turnId} />;
    case "reasoning":
      return <ReasoningRow stream={context.gatedStream} turnId={row.turnId} />;
    case "message":
      return (
        <MessageRow
          stream={context.stream}
          turnId={row.turnId}
          index={row.index}
          reduceMotion={context.reduceMotion}
        />
      );
    case "tool":
      return (
        <ToolCard
          stream={context.gatedStream}
          details={context.details}
          turnId={row.turnId}
          callId={row.callId}
        />
      );
    case "approval":
      return (
        <ApprovalCard
          details={context.details}
          conversationId={context.conversationId}
          turnId={row.turnId}
          callId={row.callId}
          connected={context.connected}
        />
      );
    case "steer":
      return <SteerRow details={context.details} turnId={row.turnId} seq={row.seq} />;
    case "status":
      return (
        <StatusRow stream={context.gatedStream} details={context.details} turnId={row.turnId} />
      );
    case "end":
      return <EndRow details={context.details} turnId={row.turnId} />;
    default:
      return null;
  }
});

function PromptRow(props: {
  readonly details: ExternalStore<TranscriptDetails>;
  readonly turnId: string;
}) {
  const { turnId } = props;
  const prompt = useStoreSelector(
    props.details,
    useCallback((state: TranscriptDetails) => state.turns.get(turnId)?.prompt, [turnId]),
  );
  if (prompt === undefined) {
    return null;
  }
  const from = prompt.origin === "phone" ? "from a phone" : "from your computer";
  return (
    <View style={styles.prompt} accessibilityLabel={`You asked ${from}: ${prompt.text}`}>
      <Text style={styles.promptText} selectable>
        {prompt.text}
      </Text>
      <Text style={styles.promptFrom}>{from}</Text>
    </View>
  );
}

function ReasoningRow(props: {
  readonly stream: ExternalStore<StreamState>;
  readonly turnId: string;
}) {
  const { turnId } = props;
  const turn = useStoreSelector(
    props.stream,
    useCallback((state: StreamState) => state.turns.get(turnId), [turnId]),
  );
  const [open, setOpen] = useState(false);
  const toggle = useCallback(() => setOpen((value) => !value), []);
  if (turn === undefined || turn.reasoning.text.length === 0) {
    return null;
  }
  const label = turn.ended ? "Thought" : "Thinking";
  const title = turn.title === undefined ? label : `${label}: ${turn.title}`;
  return (
    <Pressable
      onPress={toggle}
      accessibilityRole="button"
      accessibilityState={open ? EXPANDED : COLLAPSED}
      accessibilityHint={open ? "Hides the reasoning." : "Shows the reasoning."}
      style={styles.reasoning}
    >
      <Text style={styles.reasoningTitle}>
        {open ? "▾" : "▸"} {title}
      </Text>
      {open ? <Text style={styles.reasoningText}>{turn.reasoning.text}</Text> : null}
    </Pressable>
  );
}

function SteerRow(props: {
  readonly details: ExternalStore<TranscriptDetails>;
  readonly turnId: string;
  readonly seq: number;
}) {
  const { turnId, seq } = props;
  const text = useStoreSelector(
    props.details,
    useCallback(
      (state: TranscriptDetails) => state.turns.get(turnId)?.steers.get(seq),
      [turnId, seq],
    ),
  );
  if (text === undefined) {
    return null;
  }
  return (
    <View style={styles.steer}>
      <Text style={styles.steerLabel}>You added</Text>
      <Text style={styles.promptText}>{text}</Text>
    </View>
  );
}

function StatusRow(props: {
  readonly stream: ExternalStore<StreamState>;
  readonly details: ExternalStore<TranscriptDetails>;
  readonly turnId: string;
}) {
  const { turnId } = props;
  const turn = useStoreSelector(
    props.stream,
    useCallback((state: StreamState) => state.turns.get(turnId), [turnId]),
  );
  // The status reads the whole details (the context, the compaction, the questions);
  // only the one status row of the running turn does so.
  const details = useStore(props.details);
  const status = turnStatus(turn, details.turns.get(turnId), details);
  return (
    <View
      style={styles.status}
      accessible
      accessibilityLabel={statusLabel(status.text, status.gauge?.text, status.gauge?.detail)}
    >
      <Text style={styles.statusText}>{status.text}</Text>
      {status.gauge === undefined ? null : (
        <Text style={STATUS_GAUGE[status.gauge.level]}>{status.gauge.text}</Text>
      )}
    </View>
  );
}

function statusLabel(text: string, gauge: string | undefined, detail: string | undefined): string {
  return gauge === undefined
    ? text
    : `${text}, context ${gauge.replace("ctx ", "")} full, ${detail ?? ""}`;
}

function EndRow(props: {
  readonly details: ExternalStore<TranscriptDetails>;
  readonly turnId: string;
}) {
  const { turnId } = props;
  const end = useStoreSelector(
    props.details,
    useCallback((state: TranscriptDetails) => state.turns.get(turnId)?.end, [turnId]),
  );
  if (end === undefined) {
    return null;
  }
  const line = endLine(end);
  return (
    <View
      style={styles.end}
      accessible
      accessibilityLabel={statusLabel(line.text, line.gauge?.text, line.gauge?.detail)}
    >
      <Text style={line.tone === "error" ? styles.endError : styles.endText}>{line.text}</Text>
      {line.gauge === undefined ? null : (
        <Text style={styles.endText}>
          {", "}
          <Text style={GAUGE[line.gauge.level]}>{line.gauge.text}</Text>
          {` ${line.detail ?? ""}`}
        </Text>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  prompt: {
    alignSelf: "flex-end",
    maxWidth: "88%",
    backgroundColor: semantic.surface_2,
    borderRadius: 14,
    paddingHorizontal: 12,
    paddingVertical: 8,
    marginTop: 14,
    marginBottom: 6,
    gap: 2,
  },
  promptText: { color: semantic.text_bright, fontSize: 16, lineHeight: 22 },
  promptFrom: { color: semantic.text_subtle, fontSize: 11, alignSelf: "flex-end" },
  reasoning: { paddingVertical: 4, gap: 4 },
  reasoningTitle: { color: semantic.text_subtle, fontSize: 14, fontStyle: "italic" },
  reasoningText: { color: semantic.text_muted, fontSize: 14, lineHeight: 20 },
  steer: {
    alignSelf: "flex-end",
    maxWidth: "88%",
    borderColor: semantic.border_default,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: 14,
    paddingHorizontal: 12,
    paddingVertical: 8,
    marginVertical: 4,
    gap: 2,
  },
  steerLabel: { color: semantic.text_subtle, fontSize: 11 },
  status: { flexDirection: "row", gap: 10, paddingVertical: 6 },
  statusText: { color: semantic.text_muted, fontSize: 13, fontStyle: "italic" },
  end: { flexDirection: "row", flexWrap: "wrap", paddingTop: 2, paddingBottom: 8 },
  endText: { color: semantic.text_subtle, fontSize: 12 },
  endError: { color: semantic.state_error, fontSize: 12 },
});

const STATUS_GAUGE = StyleSheet.create({
  success: { ...GAUGE.success, fontSize: 13 },
  warning: { ...GAUGE.warning, fontSize: 13 },
  error: { ...GAUGE.error, fontSize: 13 },
});
