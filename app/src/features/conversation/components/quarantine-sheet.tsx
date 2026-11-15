// The quarantine question with the reviewer off (Q-Quarantine 1 to 6): the strip row
// above the status line and the sheet that the strip or the waiting card opens. The
// sheet lists only the changes that went to quarantine: the path in mono 13/18
// text_bright, what it does in 12/16 text_subtle, a hairline between them, then one quiet
// line. model/quarantine.ts says what each state shows.
//
// efrd takes the answer only from the terminal, so both say "answer on <machine>" in
// place of Keep and Leave, as for an approval that only the terminal can answer. After
// the answer the sheet dims, says how the question ended and offers Close.
//
// `QuarantineSheet` reads the question from the stream store; the views draw a model
// only, for the gallery and the tests.

import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";
import { Pressable, ScrollView, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { cn } from "@/components/ui/cn";
import { Dot } from "@/components/ui/dot";
import { MonoText } from "@/components/ui/mono-text";
import { SmallPlainButton } from "@/components/ui/small-buttons";
import { findQuarantine, type StreamState } from "@/lib/stream/reducer";
import {
  quarantineSheet,
  type QuarantineSheetModel,
  type QuarantineStripModel,
} from "../model/quarantine";

/** The smallest space under the answer row, on a screen without a home indicator. */
const MIN_BOTTOM = 12;

export interface QuarantineSheetProps {
  /** The stream store of the conversation. */
  readonly stream: Readable<StreamState>;
  /** The tool call whose question the sheet shows. */
  readonly callId: string;
  /** The short name of the machine, such as `desktop`. */
  readonly machineName: string;
  readonly onClose: () => void;
}

/** The sheet of the quarantine question of `callId`. */
export function QuarantineSheet(props: QuarantineSheetProps) {
  const { callId } = props;
  const question = useSelector(props.stream, (state) => findQuarantine(state, callId));
  const turnId = question?.turnId;
  const turn = useSelector(
    props.stream,
    (state) => {
      const held = turnId === undefined ? undefined : state.turns.get(turnId);
      return held === undefined ? undefined : { cwd: held.cwd, ended: held.end !== undefined };
    },
    { compare: sameTurn },
  );
  if (question === undefined) {
    return null;
  }
  const model = quarantineSheet(question, turn?.cwd, props.machineName, turn?.ended === true);
  return <QuarantineSheetView model={model} onClose={props.onClose} />;
}

function sameTurn(
  a: { readonly cwd: string | undefined; readonly ended: boolean } | undefined,
  b: { readonly cwd: string | undefined; readonly ended: boolean } | undefined,
): boolean {
  return a === b || (a !== undefined && b !== undefined && a.cwd === b.cwd && a.ended === b.ended);
}

export interface QuarantineSheetViewProps {
  readonly model: QuarantineSheetModel;
  readonly onClose: () => void;
}

/** The sheet of a model. */
export function QuarantineSheetView(props: QuarantineSheetViewProps) {
  const { model } = props;
  const bottom = Math.max(useSafeAreaInsets().bottom, MIN_BOTTOM);
  const closed = model.closed;
  return (
    <View
      testID="quarantine-sheet"
      role="dialog"
      aria-label="Changed git settings"
      className="bg-surface-1"
    >
      <ScrollView bounces={false} className="grow-0" contentContainerClassName="px-22 pt-14">
        <View className={cn(closed !== undefined && "opacity-50")}>
          {model.rows.map((row, index) => (
            <View
              key={`${row.path} ${row.what}`}
              accessible
              accessibilityLabel={`${row.path}, ${row.what}`}
              className={cn("py-9", index > 0 && "border-t-hairline border-border-subtle")}
            >
              <Text className="text-mono-shell text-text-bright">{row.path}</Text>
              <Text className="mt-1 text-caption text-text-subtle">{row.what}</Text>
            </View>
          ))}
          <Text className="mt-4 text-caption text-text-subtle">{model.quiet}</Text>
        </View>
      </ScrollView>
      <View
        className="min-h-44 flex-row items-center gap-8 pt-8 pr-14 pl-22"
        style={{ paddingBottom: bottom }}
      >
        {closed === undefined ? null : <MonoText>{closed}</MonoText>}
        <View className="flex-1" />
        {model.terminal === undefined ? null : (
          <MonoText tone="accent_ui">{model.terminal}</MonoText>
        )}
        {closed === undefined ? null : <SmallPlainButton label="Close" onPress={props.onClose} />}
      </View>
    </View>
  );
}

export interface QuarantineStripViewProps {
  readonly model: QuarantineStripModel;
  readonly onOpen: () => void;
}

/** The strip of a quarantine question: the dot, the question, then where to answer. */
export function QuarantineStripView(props: QuarantineStripViewProps) {
  const { model } = props;
  return (
    <View testID="quarantine-strip" className="min-h-44 flex-row items-center gap-4 pr-12 pl-16">
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={model.label}
        accessibilityHint="Opens the changed git settings."
        onPress={props.onOpen}
        className="min-h-44 flex-1 flex-row items-center gap-8 active:opacity-60"
      >
        <Dot size="large" />
        <Text numberOfLines={1} className="flex-1 text-mono-sm text-text-bright">
          {model.line}
        </Text>
      </Pressable>
      <MonoText tone="accent_ui">{model.terminal}</MonoText>
    </View>
  );
}
