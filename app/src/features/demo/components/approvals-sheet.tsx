// The S-Approvals board: the real approval block in every state (design-spec section 1,
// "Approval"). Each state is a stream of its own: its events go through the decoder and
// the reducer of the app, and the model of the block (model/approval-block.ts) gives what
// the block shows. The buttons answer nothing.

import { useState } from "react";

import { ConnectionError } from "@/lib/net/errors";
import { ApprovalBlockView } from "@/features/conversation/components/approval-block";
import {
  approvalBlock,
  NO_ANSWER,
  type AnswerState,
  type ApprovalBlockModel,
} from "@/features/conversation/model/approval-block";
import { selectApproval } from "@/features/conversation/model/selectors";
import { approvalCases, type ApprovalCase } from "../model/demo-conversations";
import { staticState } from "../model/static-stream";
import { SheetCase, SheetScroll } from "./sheet-layout";

/** The answer of this phone in `one`. */
function answerOf(one: ApprovalCase): AnswerState {
  if (one.failed !== undefined) {
    return {
      ...NO_ANSWER,
      failed: {
        error: new ConnectionError("offline", "the machine has no ready connection"),
        decision: one.failed,
        commandId: "0199d3e0-de36-7000-8000-000000000001",
      },
    };
  }
  return { ...NO_ANSWER, sending: one.sending, answeredHere: one.answeredHere === true };
}

/** The model of the block of `one`, from its stream at time `end`. */
function modelOf(one: ApprovalCase, end: number): ApprovalBlockModel | undefined {
  const state = staticState(one.steps, end, one.name);
  const approval = selectApproval(one.turnId, one.callId)(state);
  if (approval === undefined) {
    return undefined;
  }
  return approvalBlock(approval, {
    machine: "desktop",
    cwd: state.turns.get(one.turnId)?.cwd,
    connected: true,
    expiry: undefined,
    answer: answerOf(one),
  });
}

/** Every case with its model. */
function sheetModels(): { readonly one: ApprovalCase; readonly model: ApprovalBlockModel }[] {
  let n = 0;
  const end = Date.now();
  return approvalCases(() => {
    n += 1;
    return `0199d3e0-de36-7000-8000-${n.toString(16).padStart(12, "0")}`;
  }).flatMap((one) => {
    const model = modelOf(one, end);
    return model === undefined ? [] : [{ one, model }];
  });
}

/** The S-Approvals board. */
export function ApprovalsSheet() {
  const [cases] = useState(sheetModels);
  return (
    <SheetScroll>
      {cases.map(({ one, model }) => (
        <SheetCase key={one.name} name={one.name}>
          <ApprovalBlockView model={model} onAnswer={() => undefined} />
        </SheetCase>
      ))}
    </SheetScroll>
  );
}
