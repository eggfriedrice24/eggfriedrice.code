// What every row of one transcript reads: the stream store of the conversation, its row
// state, the name of its machine and the actions of the rows.
// The screen gives it once, through context, so a row takes only its own key as a prop
// and renders only when its own part of the store changes.

import type { Readable } from "@tanstack/store";
import { createContext, use } from "react";
import type { HostInstance } from "react-native";

import type { StreamState, UserQuestionView } from "@/lib/stream/reducer";
import type { AppAttach } from "@/features/navigation/model/app-services";
import type { DraftKey } from "../model/drafts";
import type { ModelChoicesState } from "../model/model-choices";
import type { RowUiStore } from "../model/row-ui-state";
import type { SwitchCatalog } from "../model/switch-line";

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
  /** Opens the approval sheet of the question of tool call `callId`. */
  readonly openApproval: (callId: string) => void;
  /**
   * Opens the model sheet with `model` on account `provider` selected: a switch action
   * of a turn whose account is out (AC-Out).
   */
  readonly openModels: (model: string, provider: string) => void;
}

/** The model choice of the composer of the transcript: a switch offer reads it. */
export interface ScopeChoice {
  readonly store: Readable<ModelChoicesState>;
  readonly key: DraftKey;
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
  /**
   * The models and accounts of the machine (`models.list`), when the app has them: the
   * ⇄ line names the account of a model that runs on two.
   */
  readonly catalog?: SwitchCatalog;
  /** The model choice of the composer: after a switch, the offer becomes "retry". */
  readonly choice?: ScopeChoice;
  /** The answers to the questions of the model; none: the rows take no tap (S-Ask 4). */
  readonly questions?: ScopeQuestions;
  /**
   * The files of the prompts: the previews and Quick Look of their photos, and the
   * withdraw of a prompt on its way. Null in a build without them.
   */
  readonly attach: AppAttach | null;
}

/** What the row of a question of the model needs from the screen. */
export interface ScopeQuestions {
  /** Opens the action sheet of `question`, out of `anchor` (the row). */
  readonly open: (question: UserQuestionView, anchor: HostInstance | null) => void;
  /** The question whose answer efrd has not taken yet. */
  readonly sending: string | undefined;
  /**
   * True while the phone can answer question `questionId` (`canAnswer`). Else the row
   * takes no tap, as the strip does: offline, or with its answer in flight.
   */
  readonly enabled: (questionId: string) => boolean;
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
