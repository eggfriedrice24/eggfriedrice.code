// What every row of one transcript reads: the stream store of the conversation, its row
// state, the name of its machine, the actions of the rows and the view of an approval.
// The screen gives it once, through context, so a row takes only its own key as a prop
// and renders only when its own part of the store changes.

import type { Readable } from "@tanstack/store";
import { createContext, type ReactNode, use } from "react";

import type { StreamState } from "@/lib/stream/reducer";
import type { RowUiStore } from "../model/row-ui-state";

/** The actions that the rows of a transcript start. The screen wires them. */
export interface TranscriptActions {
  /** Takes back the queued prompt of turn `turnId` (`prompt.withdraw`). */
  readonly withdraw: (turnId: string) => void;
  /**
   * Sends the prompt of the failed turn `turnId` again, as a new prompt with a new
   * command id (decision D8: efrd has no retry of a turn). `text` is that prompt.
   */
  readonly retry: (turnId: string, text: string) => void;
  /** Puts `text`, a message that the user sent, in the composer to change and send again. */
  readonly editAndResend: (text: string) => void;
  /** Sends the prompt of the outbox entry `key` again, after it failed to send. */
  readonly resend: (key: string) => void;
}

/** The approval row with `key`, for the view of the approval block. */
export interface ApprovalRowRef {
  readonly key: string;
  readonly turnId: string;
  readonly callId: string;
}

/** What the rows of one transcript share. */
export interface TranscriptScope {
  /** The stream store of the conversation (`ConversationStream.store`). */
  readonly store: Readable<StreamState>;
  /** The rows that the user opened (`ConversationStream.rowUi`). */
  readonly rowUi: RowUiStore;
  /** The name of the machine, for "in a terminal on desktop". */
  readonly machineName: string;
  readonly actions: TranscriptActions;
  /** The view of an approval row: the approval block (another package builds it). */
  readonly renderApproval: (row: ApprovalRowRef) => ReactNode;
}

/** The context of the rows. Transcript gives it. */
export const TranscriptScopeContext = createContext<TranscriptScope | undefined>(undefined);

/** The scope of the transcript that this row is in. */
export function useTranscriptScope(): TranscriptScope {
  const scope = use(TranscriptScopeContext);
  if (scope === undefined) {
    throw new Error("a transcript row needs the TranscriptScopeContext of its transcript");
  }
  return scope;
}
