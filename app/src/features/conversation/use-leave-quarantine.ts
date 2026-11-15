// The Leave of a quarantine question from the phone (Q-Quarantine 8, 9): the mutation
// `sandbox.surface_respond` with `keep: false`, in the scope of the conversation, with a
// new command id per tap. The strip and the sheet show the braille cell while it runs.

import { useMutation } from "@tanstack/react-query";

import { newCommandId } from "@/lib/native/database";
import type { MachineCaller } from "@/lib/query/machine-caller";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { leaveQuarantineMutationOptions } from "./conversation-queries";

/** The Leave of one conversation. */
export interface LeaveQuarantine {
  /** Leaves the changes of question `questionId` in quarantine. */
  leave(questionId: string): void;
  /** True while efrd has not taken the answer yet. */
  readonly leaving: boolean;
}

/** The Leave of the quarantine questions of `conversation`. */
export function useLeaveQuarantine(
  caller: MachineCaller,
  conversation: ConversationRef,
): LeaveQuarantine {
  const mutation = useMutation(leaveQuarantineMutationOptions(caller, conversation));
  return {
    leave(questionId: string): void {
      mutation.mutate({ questionId, commandId: newCommandId() });
    },
    leaving: mutation.isPending,
  };
}
