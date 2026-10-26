// A tool call: its tool and one line of its input, its state, and a card of a fixed
// height with the last 3 lines of its output, as the terminal shows a running call. A
// call that changed files shows its diff behind a tap.

import { memo, useCallback, useState } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";

import type { StreamState } from "@/lib/stream/reducer";
import { MONO_FONT } from "@/theme/markdown";
import { semantic } from "@/theme/palette";

import type { TranscriptDetails } from "./details";
import { DiffView } from "./diff_view";
import type { ExternalStore } from "@/lib/store/frame-store";
import { lastLines, size } from "./format";
import { useStoreSelector } from "./hooks";
import { COLLAPSED, EXPANDED, PLAIN } from "./styles";

/** How many lines of output the card shows. */
const TAIL_LINES = 3;

/** The card of tool call `callId` of turn `turnId`. */
export const ToolCard = memo(function ToolCard(props: {
  readonly stream: ExternalStore<StreamState>;
  readonly details: ExternalStore<TranscriptDetails>;
  readonly turnId: string;
  readonly callId: string;
}) {
  const { turnId, callId } = props;
  const call = useStoreSelector(
    props.stream,
    useCallback(
      (state: StreamState) => state.turns.get(turnId)?.toolCalls.get(callId),
      [turnId, callId],
    ),
  );
  const tool = useStoreSelector(
    props.details,
    useCallback(
      (state: TranscriptDetails) => state.turns.get(turnId)?.tools.get(callId),
      [turnId, callId],
    ),
  );
  const [open, setOpen] = useState(false);
  const toggle = useCallback(() => setOpen((value) => !value), []);
  const name = tool?.tool ?? call?.tool ?? "tool";
  const done = tool?.done === true || call?.done === true;
  const lines = lastLines(call?.tail ?? "", TAIL_LINES);
  const state = toolState(done, tool?.isError, tool?.exitCode, tool?.refusal, tool?.leftRunning);
  return (
    <View
      style={styles.card}
      accessible
      accessibilityLabel={`${name} ${tool?.summary ?? ""}, ${state.text}`}
    >
      <View style={styles.header}>
        <Text style={styles.name}>{name}</Text>
        <Text style={styles.summary} numberOfLines={2}>
          {tool?.summary ?? ""}
        </Text>
        {done ? null : <ActivityIndicator size="small" color={semantic.text_muted} />}
      </View>
      {lines.length > 0 ? (
        <View style={styles.tail}>
          {SLOTS.slice(0, lines.length).map((slot) => (
            <Text key={slot} style={styles.line} numberOfLines={1}>
              {lines[slot] === "" ? " " : lines[slot]}
            </Text>
          ))}
        </View>
      ) : null}
      <Text style={STATE[state.tone]}>
        {state.text}
        {call !== undefined && call.bytes > 0 ? `, ${size(call.bytes)} of output` : ""}
      </Text>
      {tool?.diff === undefined ? null : (
        <>
          <Pressable
            onPress={toggle}
            accessibilityRole="button"
            accessibilityState={open ? EXPANDED : COLLAPSED}
            hitSlop={8}
            style={PLAIN}
          >
            <Text style={styles.toggle}>{open ? "Hide the changes" : "Show the changes"}</Text>
          </Pressable>
          {open ? <DiffView diff={tool.diff} /> : null}
        </>
      )}
    </View>
  );
});

function toolState(
  done: boolean,
  isError: boolean | undefined,
  exitCode: number | undefined,
  refusal: string | undefined,
  leftRunning: boolean | undefined,
): { text: string; tone: keyof typeof STATE } {
  if (!done) {
    return { text: "running", tone: "muted" };
  }
  if (refusal !== undefined) {
    return { text: `refused by ${refusal}`, tone: "error" };
  }
  if (leftRunning === true) {
    return { text: "still runs in the shell", tone: "warning" };
  }
  if (isError === true) {
    return { text: exitCode === undefined ? "failed" : `failed, exit ${exitCode}`, tone: "error" };
  }
  return { text: exitCode === undefined ? "done" : `done, exit ${exitCode}`, tone: "muted" };
}

/** The places of the tail's lines, oldest first. */
const SLOTS = Array.from({ length: TAIL_LINES }, (_, slot) => slot);

const STATE = StyleSheet.create({
  muted: { fontSize: 12, color: semantic.text_muted },
  warning: { fontSize: 12, color: semantic.state_warning },
  error: { fontSize: 12, color: semantic.state_error },
});

const styles = StyleSheet.create({
  card: {
    backgroundColor: semantic.surface_0,
    borderColor: semantic.border_default,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: 8,
    padding: 10,
    gap: 6,
    marginVertical: 4,
  },
  header: { flexDirection: "row", alignItems: "center", gap: 8 },
  name: { color: semantic.accent_ui, fontFamily: MONO_FONT, fontSize: 13, fontWeight: "600" },
  summary: { flex: 1, color: semantic.text_primary, fontFamily: MONO_FONT, fontSize: 13 },
  tail: { height: 17 * TAIL_LINES, overflow: "hidden" },
  line: { color: semantic.text_muted, fontFamily: MONO_FONT, fontSize: 12, lineHeight: 17 },
  toggle: { color: semantic.link, fontSize: 13 },
});
