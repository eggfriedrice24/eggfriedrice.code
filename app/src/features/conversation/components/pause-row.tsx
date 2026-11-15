// The pause of the reviewer in auto (board AU-Auto 2): one quiet mono line in its turn.
// It asks nothing: no strip, no sheet, no Allow. The model finishes what stays in the
// sandbox, and the next action that needs the reviewer stops the turn (errata 6).

import { useSelector } from "@tanstack/react-store";

import { MonoText } from "@/components/ui/mono-text";
import { pausedLine } from "../model/exit-words";
import { useTranscriptScope } from "./use-transcript-scope";

export interface PauseRowProps {
  readonly turnId: string;
}

/** The pause line of turn `turnId`. */
export function PauseRow(props: PauseRowProps) {
  const scope = useTranscriptScope();
  const pause = useSelector(scope.store, (state) => state.turns.get(props.turnId)?.pause);
  if (pause === undefined) {
    return null;
  }
  return (
    <MonoText testID="pause-line" tone="text_subtle">
      {pausedLine(pause)}
    </MonoText>
  );
}
