// One row of the transcript, by kind (model/rows.ts). Each row reads only its own part
// of the stream store through a selector, so it renders only when that part changes. An
// approval row is the approval block, which the screen gives through the scope.

import type { Row } from "../model/rows";
import { MessageRow } from "./message-row";
import { RunningRow } from "./running-line";
import { ThoughtRow } from "./thought-row";
import { ToolRow } from "./tool-row";
import { TurnEndRow } from "./turn-end-line";
import { useTranscriptScope } from "./use-transcript-scope";
import { type PendingPrompt, PendingPromptRow, PromptRow, SteerRow } from "./your-message";

/** A row of the transcript list: a row of the stream, or a prompt of the outbox. */
export type TranscriptItem =
  | Row
  | { readonly kind: "pending"; readonly key: string; readonly prompt: PendingPrompt };

export interface TranscriptRowProps {
  readonly item: TranscriptItem;
}

/** One row. */
export function TranscriptRow(props: TranscriptRowProps) {
  const item = props.item;
  switch (item.kind) {
    case "prompt":
      return <PromptRow turnId={item.turnId} />;
    case "reasoning":
      return <ThoughtRow rowKey={item.key} turnId={item.turnId} />;
    case "message":
      return <MessageRow turnId={item.turnId} index={item.index} />;
    case "tool":
      return <ToolRow rowKey={item.key} turnId={item.turnId} callId={item.callId} />;
    case "approval":
      return <ApprovalRow rowKey={item.key} turnId={item.turnId} callId={item.callId} />;
    case "steer":
      return <SteerRow turnId={item.turnId} seq={item.seq} />;
    case "status":
      return <RunningRow turnId={item.turnId} />;
    case "end":
      return <TurnEndRow turnId={item.turnId} />;
    case "pending":
      return <PendingPromptRow prompt={item.prompt} />;
    default:
      return null;
  }
}

function ApprovalRow(props: {
  readonly rowKey: string;
  readonly turnId: string;
  readonly callId: string;
}) {
  const scope = useTranscriptScope();
  return scope.renderApproval({ key: props.rowKey, turnId: props.turnId, callId: props.callId });
}
