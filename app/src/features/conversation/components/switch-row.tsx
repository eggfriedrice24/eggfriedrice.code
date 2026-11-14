// The ⇄ line of a switch of model or account (MS-Switched): mono 11 text_subtle above
// the prompt of the turn that runs on the new model. Not tappable; VoiceOver reads the
// whole sentence. model/switch-line.ts makes the words.

import { useSelector } from "@tanstack/react-store";

import { MonoText } from "@/components/ui/mono-text";
import { compareShallow } from "../model/selectors";
import { switchLine } from "../model/switch-line";
import { useTranscriptScope } from "./use-transcript-scope";

export interface SwitchRowProps {
  readonly turnId: string;
}

/** The switch line of turn `turnId`. */
export function SwitchRow(props: SwitchRowProps) {
  const scope = useTranscriptScope();
  const row = useSelector(
    scope.store,
    (state) => {
      const turn = state.turns.get(props.turnId);
      return { switched: turn?.switched, effort: turn?.effort };
    },
    { compare: compareShallow },
  );
  if (row.switched === undefined) {
    return null;
  }
  const line = switchLine(row.switched, row.effort, scope.catalog);
  return (
    <MonoText
      testID="switch-line"
      tone="text_subtle"
      accessibilityLabel={line.label}
      accessibilityRole="text"
    >
      {line.text}
    </MonoText>
  );
}
