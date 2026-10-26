// The row of one tool call: an edit block for a call with a diff (a file tool's call),
// else a tool block. It reads only its call, whether its turn ended, and its own open flags.

import { useSelector } from "@tanstack/react-store";

import type { StreamState } from "@/lib/stream/reducer";
import { rowFlag } from "../model/row-ui-state";
import { selectTool } from "../model/selectors";
import { EditBlockView } from "./edit-block";
import { ToolBlockView } from "./tool-block";
import { useTranscriptScope } from "./use-transcript-scope";

function selectTurnEnded(turnId: string): (state: StreamState) => boolean {
  return (state) => state.turns.get(turnId)?.end !== undefined;
}

export interface ToolRowProps {
  readonly rowKey: string;
  readonly turnId: string;
  readonly callId: string;
}

/** The block of tool call `callId` of turn `turnId`. */
export function ToolRow(props: ToolRowProps) {
  const scope = useTranscriptScope();
  const call = useSelector(scope.store, selectTool(props.turnId, props.callId));
  const turnEnded = useSelector(scope.store, selectTurnEnded(props.turnId));
  const expanded = useSelector(scope.rowUi.store, rowFlag(props.rowKey, "expanded"));
  const allHunks = useSelector(scope.rowUi.store, rowFlag(props.rowKey, "all_hunks"));
  if (call === undefined) {
    return null;
  }
  const rowKey = props.rowKey;
  const toggle = (): void => scope.rowUi.toggle(rowKey, "expanded");
  if (call.diff !== undefined && call.diff !== "") {
    return (
      <EditBlockView
        call={call}
        diff={call.diff}
        expanded={expanded}
        allHunks={allHunks}
        onToggle={toggle}
        onShowAllHunks={() => scope.rowUi.set(rowKey, "all_hunks", true)}
      />
    );
  }
  return <ToolBlockView call={call} turnEnded={turnEnded} expanded={expanded} onToggle={toggle} />;
}
