// The answers of the phone to the questions of the model (S-Ask): the native action
// sheet of a question, and the mutation `question.respond` in the scope of the
// conversation, with a new command id per tap. The strip, the row of the call and the
// composer read which question it sends now and which answer did not arrive.

import { useMutation } from "@tanstack/react-query";
import type { HostInstance } from "react-native";

import { showActionSheet } from "@/components/ui/native/show-action-sheet";
import { newCommandId } from "@/lib/native/database";
import type { QuestionReply } from "@/lib/protocol/generated";
import type { MachineCaller } from "@/lib/query/machine-caller";
import type { UserQuestionView } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { answerQuestionMutationOptions } from "./conversation-queries";
import { questionChoice, questionSheet } from "./model/question";

/** The answers of one conversation. */
export interface AnswerQuestion {
  /**
   * Answers question `questionId` with `reply`. `onSuccess` runs once efrd took it, so a
   * typed answer stays in the field until then.
   */
  readonly answer: (questionId: string, reply: QuestionReply, onSuccess?: () => void) => void;
  /**
   * Opens the action sheet of `question` out of `anchor`: a pick of an option or Skip
   * answers at once, and "Type an answer" calls `onType` of the hook. Cancel does nothing.
   */
  readonly open: (question: UserQuestionView, anchor: HostInstance | null) => void;
  /** The question whose answer efrd has not taken yet. */
  readonly sending: string | undefined;
  /**
   * The question whose last answer did not arrive: efrd refused it, the question ended,
   * or the connection failed. The strip says "not sent", as for an approval.
   */
  readonly failed: string | undefined;
}

/**
 * The answers to the questions of the model in `conversation`. `onType` puts the composer
 * in the answer state of a question.
 */
export function useAnswerQuestion(
  caller: MachineCaller,
  conversation: ConversationRef,
  onType: (question: UserQuestionView) => void,
): AnswerQuestion {
  // The result of useMutation is new on each render, and only `mutate` is stable: a
  // closure over the whole result would make the answers new on each render too.
  const { mutate, isPending, isError, variables } = useMutation(
    answerQuestionMutationOptions(caller, conversation),
  );
  const asked = variables?.questionId;
  const answer = (questionId: string, reply: QuestionReply, onSuccess?: () => void): void => {
    const input = { questionId, answer: reply, commandId: newCommandId() };
    if (onSuccess === undefined) {
      mutate(input);
    } else {
      mutate(input, { onSuccess });
    }
  };
  const ask = async (question: UserQuestionView, anchor: HostInstance | null): Promise<void> => {
    const choice = questionChoice(await showActionSheet(questionSheet(question), anchor));
    if (choice?.kind === "reply") {
      answer(question.questionId, choice.reply);
    } else if (choice?.kind === "type") {
      onType(question);
    }
  };
  return {
    answer,
    open: (question, anchor) => {
      void ask(question, anchor).catch(() => undefined);
    },
    sending: isPending ? asked : undefined,
    failed: isError ? asked : undefined,
  };
}
