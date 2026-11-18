// The quiet line of an `attachments_withheld` event (board S-Attach 6d), in text_muted
// where it happened: "Not sent: shot.png. The provider refused it." From then on efrd
// sends those attachments to the model as a path, and their tiles say "As a path".

import { useSelector } from "@tanstack/react-store";

import { BodyText } from "@/components/ui/body-text";
import { withheldLine } from "../model/attachments-format";
import { useTranscriptScope } from "./use-transcript-scope";

export interface WithheldRowProps {
  readonly turnId: string;
  readonly seq: number;
}

/** The withheld line `seq` of turn `turnId`. */
export function WithheldRow(props: WithheldRowProps) {
  const scope = useTranscriptScope();
  const view = useSelector(scope.store, (state) =>
    state.turns.get(props.turnId)?.withheld.get(props.seq),
  );
  const names = useSelector(scope.store, (state) => state.attachmentNames);
  if (view === undefined) {
    return null;
  }
  // NOTE: an attachment of a turn before the loaded history has no name here; its id stands.
  const line = withheldLine(
    view.attachments.map((id) => names.get(id) ?? id),
    view.reason,
  );
  return (
    <BodyText testID="withheld-line" size="caption" tone="text_muted">
      {line}
    </BodyText>
  );
}
