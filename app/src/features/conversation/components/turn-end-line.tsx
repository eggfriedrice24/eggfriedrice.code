// The line at the end of a turn (design 3.9 and the C-Failed board): one small mono 11/15
// line in text_subtle right under the answer, left aligned as the terminal prints it, such
// as "14:32 · 3m 08s · 18k tokens". The user asked for it in place of the centred line
// between two hairlines.
//
// - Interrupted: "■" in text_muted, then "interrupted by you · 14:31 · 1m 02s".
// - Failed: "✗" in state_error and "turn failed" in text_primary, then the time; under
//   it the reason in mono 11/16 text_muted; under that the plain buttons
//   "retry" and "copy error", 18 pt apart. Retry sends the turn's prompt again as a new
//   prompt (decision D8). The composer goes back to idle by itself.
//
// The error haptic of a failed turn belongs to the screen: it plays only when the
// conversation is on screen.

import { useSelector } from "@tanstack/react-store";
import { Text, View } from "react-native";

import { MonoText } from "@/components/ui/mono-text";
import { PlainButton } from "@/components/ui/plain-button";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type { Origin } from "@/lib/protocol/generated";
import type { StreamState, TurnEnd } from "@/lib/stream/reducer";
import { compareShallow } from "../model/selectors";
import { errorText, type TurnEndView, turnEndView } from "../model/turn-end";
import { useTranscriptScope } from "./use-transcript-scope";

export interface TurnEndLineViewProps {
  readonly view: TurnEndView;
  /** Sends the prompt again; undefined when there is no prompt to send. */
  readonly onRetry: (() => void) | undefined;
}

/** The turn end line. */
export function TurnEndLineView(props: TurnEndLineViewProps) {
  const view = props.view;
  const onRetry = props.onRetry;
  return (
    <View>
      <View accessible accessibilityLabel={view.label}>
        <Text className="text-mono-xs-tight text-text-subtle">
          {view.glyph === undefined ? null : (
            <Text className={view.glyph === "✗" ? "text-state-error" : "text-text-muted"}>
              {`${view.glyph} `}
            </Text>
          )}
          {view.lead === undefined ? null : (
            <Text className="text-text-primary">{`${view.lead} `}</Text>
          )}
          {view.detail}
        </Text>
      </View>
      {view.reason === undefined ? null : (
        <MonoText tone="text_muted" className="mt-4">
          {view.reason}
        </MonoText>
      )}
      {view.canRetry ? (
        <View className="mt-2 flex-row gap-18">
          {onRetry === undefined ? null : (
            <PlainButton label="retry" accessibilityLabel="Retry the turn" onPress={onRetry} />
          )}
          <PlainButton
            label="copy error"
            onPress={() => {
              void copyText(errorText(view));
              haptic("light");
            }}
          />
        </View>
      ) : null}
    </View>
  );
}

/** What the turn end row reads of its turn. */
interface EndRowView {
  readonly end: TurnEnd | undefined;
  readonly startedAt: number | undefined;
  readonly interruptedBy: Origin | undefined;
  readonly prompt: string | undefined;
}

function selectEndRow(turnId: string): (state: StreamState) => EndRowView {
  return (state) => {
    const turn = state.turns.get(turnId);
    return {
      end: turn?.end,
      startedAt: turn?.startedAt,
      interruptedBy: turn?.interruptedBy,
      prompt: turn?.prompt?.text,
    };
  };
}

export interface TurnEndRowProps {
  readonly turnId: string;
}

/** The end of turn `turnId`, from the stream store. */
export function TurnEndRow(props: TurnEndRowProps) {
  const scope = useTranscriptScope();
  const row = useSelector(scope.store, selectEndRow(props.turnId), { compare: compareShallow });
  if (row.end === undefined) {
    return null;
  }
  const view = turnEndView(row.end, row.startedAt, row.interruptedBy, scope.machineName);
  const turnId = props.turnId;
  const prompt = row.prompt;
  return (
    <TurnEndLineView
      view={view}
      onRetry={prompt === undefined ? undefined : () => scope.actions.retry(turnId, prompt)}
    />
  );
}
