// The row of one tool call: the question card of an `ask_user` call (S-Ask 4), an edit
// block for a call with a diff (a file tool's call), else a tool block. It reads only its
// call, whether its turn ended, its approval and its own open flags. A tap on a card
// whose approval waits opens the approval sheet.

import { useSelector } from "@tanstack/react-store";

import type { SurfaceChange } from "@/lib/protocol/generated";
import type { ReviewerPause, StreamState, ToolCallView } from "@/lib/stream/reducer";
import { approvalCall, shortTimeLeft } from "../model/approval-strip";
import { exitNote, stoppedByPause, surfaceLines } from "../model/exit-words";
import { rowFlag } from "../model/row-ui-state";
import {
  compareShallow,
  selectApproval,
  selectTool,
  selectToolApproval,
  selectToolExpiry,
} from "../model/selectors";
import { useExpiryLabel } from "../use-answer-approval";
import { questionRow } from "../model/question";
import { EditBlockView } from "./edit-block";
import { QuestionRowView } from "./question-row";
import { ToolBlockView } from "./tool-block";
import { useTranscriptScope } from "./use-transcript-scope";

function selectTurnEnded(turnId: string): (state: StreamState) => boolean {
  return (state) => state.turns.get(turnId)?.end !== undefined;
}

function selectPause(turnId: string): (state: StreamState) => ReviewerPause | undefined {
  return (state) => state.turns.get(turnId)?.pause;
}

function selectCwd(turnId: string): (state: StreamState) => string | undefined {
  return (state) => state.turns.get(turnId)?.cwd;
}

/**
 * The changed files that the card of `call` lists. When efr asks about the quarantined
 * ones (the reviewer off), the sheet of the question lists those, so the card keeps only
 * the ones left in place, as the terminal does.
 */
function cardChanges(call: ToolCallView): readonly SurfaceChange[] {
  const changes = call.surface ?? [];
  return call.quarantine === undefined ? changes : changes.filter((change) => !change.quarantined);
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
  const approval = useSelector(scope.store, selectToolApproval(props.turnId, props.callId), {
    compare: compareShallow,
  });
  const expiresAt = useSelector(scope.store, selectToolExpiry(props.turnId, props.callId));
  const left = shortTimeLeft(useExpiryLabel(expiresAt));
  const pause = useSelector(scope.store, selectPause(props.turnId));
  const cwd = useSelector(scope.store, selectCwd(props.turnId));
  const expanded = useSelector(scope.rowUi.store, rowFlag(props.rowKey, "expanded"));
  const allHunks = useSelector(scope.rowUi.store, rowFlag(props.rowKey, "all_hunks"));
  if (call === undefined) {
    return null;
  }
  const rowKey = props.rowKey;
  const callId = props.callId;
  const toggle = (): void => scope.rowUi.toggle(rowKey, "expanded");
  const stopped = stoppedByPause(call, pause, approval !== undefined, turnEnded);
  const note =
    call.exit === undefined || approval !== undefined
      ? undefined
      : exitNote(call.exit, call.summary, stopped ? pause : undefined);
  const question = call.question;
  if (question !== undefined) {
    const questions = scope.questions;
    const questionId = question.questionId;
    return (
      <QuestionRowView
        row={questionRow(question, scope.machineName, questions?.sending === questionId)}
        onOpen={
          questions?.enabled(questionId) === true
            ? (anchor) => questions.open(question, anchor)
            : undefined
        }
      />
    );
  }
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
  return (
    <ToolBlockView
      call={call}
      turnEnded={turnEnded}
      approval={approval}
      left={left}
      expanded={expanded}
      onToggle={toggle}
      onOpenApproval={() => scope.actions.openApproval(callId)}
      note={note}
      stopped={stopped}
      surface={call.surface === undefined ? undefined : surfaceLines(cardChanges(call), cwd)}
    />
  );
}

export interface ApprovalCardRowProps {
  readonly turnId: string;
  readonly callId: string;
}

/**
 * The card of a question whose tool call the transcript does not hold: the tool card of
 * the call as the question names it, with the same states (S-Tools).
 */
export function ApprovalCardRow(props: ApprovalCardRowProps) {
  const scope = useTranscriptScope();
  const question = useSelector(scope.store, selectApproval(props.turnId, props.callId));
  const turnEnded = useSelector(scope.store, selectTurnEnded(props.turnId));
  const approval = useSelector(scope.store, selectToolApproval(props.turnId, props.callId), {
    compare: compareShallow,
  });
  const expiresAt = useSelector(scope.store, selectToolExpiry(props.turnId, props.callId));
  const left = shortTimeLeft(useExpiryLabel(expiresAt));
  if (question === undefined) {
    return null;
  }
  const callId = props.callId;
  return (
    <ToolBlockView
      call={approvalCall(question)}
      turnEnded={turnEnded}
      approval={approval}
      left={left}
      expanded={false}
      onToggle={() => undefined}
      onOpenApproval={() => scope.actions.openApproval(callId)}
    />
  );
}
