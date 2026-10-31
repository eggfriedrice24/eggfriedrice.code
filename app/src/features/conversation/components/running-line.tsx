// The running line under the last block of a running turn (design 3.10): the braille
// cell in yolk, "running" in yolk, and "· 1m 12s" in text_subtle, mono 11/16. The time
// shows from 2 s on. While efrd compacts the context it says "compacting".
//
// Above it sit the writing blocks: a tool input that the model still writes ("writing
// patch" and the size that grows, design 3.7). The tool block takes its place when the
// call starts.
//
// While an approval of the turn waits, nothing runs (design R4 item 5): the line goes
// away, the waiting tool block says why, and the composer's status line says "waiting
// for you".

import { useSelector } from "@tanstack/react-store";
import { Text, View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import type { StreamState, ToolInputView } from "@/lib/stream/reducer";
import { elapsed } from "../model/durations";
import { compareShallow } from "../model/selectors";
import { WritingBlockView } from "./tool-block";
import { useNow } from "./use-now";
import { useTranscriptScope } from "./use-transcript-scope";

export interface RunningLineViewProps {
  readonly startedAt: number | undefined;
  readonly compacting: boolean;
}

/** The running line. */
export function RunningLineView(props: RunningLineViewProps) {
  const now = useNow(true);
  const time = elapsed(props.startedAt, now);
  const word = props.compacting ? "compacting" : "running";
  return (
    <View
      accessible
      accessibilityLabel={time === undefined ? word : `${word}, ${time}`}
      className="min-h-24 flex-row items-center gap-6"
    >
      <BrailleCell />
      <Text className="text-mono-xs text-accent-primary">{word}</Text>
      {time === undefined ? null : (
        <Text className="text-mono-xs text-text-subtle">{`· ${time}`}</Text>
      )}
    </View>
  );
}

interface StatusRowView {
  readonly inputs: ReadonlyMap<number, ToolInputView> | undefined;
  readonly startedAt: number | undefined;
  readonly compacting: boolean;
  /** An approval of the turn waits for an answer. */
  readonly waiting: boolean;
}

function selectStatusRow(turnId: string): (state: StreamState) => StatusRowView {
  return (state) => {
    const turn = state.turns.get(turnId);
    let waiting = false;
    for (const approval of turn?.approvals.values() ?? []) {
      waiting ||= approval.status === "pending";
    }
    return {
      inputs: turn?.toolInputs,
      startedAt: turn?.startedAt,
      compacting: state.compacting,
      waiting,
    };
  };
}

export interface RunningRowProps {
  readonly turnId: string;
}

/** The status row of the running turn `turnId`: its writing blocks and its running line. */
export function RunningRow(props: RunningRowProps) {
  const scope = useTranscriptScope();
  const row = useSelector(scope.store, selectStatusRow(props.turnId), {
    compare: compareShallow,
  });
  const inputs = [...(row.inputs?.entries() ?? [])];
  if (row.waiting && inputs.length === 0) {
    return null;
  }
  return (
    <View className="gap-item">
      {inputs.map(([position, input]) => (
        <WritingBlockView key={position} input={input} />
      ))}
      {row.waiting ? null : (
        <RunningLineView startedAt={row.startedAt} compacting={row.compacting} />
      )}
    </View>
  );
}
