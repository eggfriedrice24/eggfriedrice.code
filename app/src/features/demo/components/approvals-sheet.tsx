// The S-Approvals board: the real approval strip and the real approval sheet in every
// state (design-spec section 1, "Approval", round 8 "minimal"). Each state is a stream of
// its own: its events go through the decoder and the reducer of the app, and the models
// of model/approval-strip.ts give what the strip and the sheet show. The buttons answer
// nothing.

import { useState } from "react";
import { View } from "react-native";

import { ConnectionError } from "@/lib/net/errors";
import { pendingApprovals } from "@/lib/stream/reducer";
import { ApprovalSheetView } from "@/features/conversation/components/approval-sheet";
import { ApprovalStripView } from "@/features/conversation/components/approval-strip";
import {
  NO_ANSWER,
  type AnswerState,
  type ApprovalBlockContext,
} from "@/features/conversation/model/approval-block";
import {
  approvalSheet,
  approvalStrip,
  type ApprovalSheetModel,
  type ApprovalStripModel,
} from "@/features/conversation/model/approval-strip";
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

/** The strip and the sheet of one case. */
interface CaseModels {
  readonly one: ApprovalCase;
  readonly strip: ApprovalStripModel | undefined;
  readonly sheet: ApprovalSheetModel;
}

/** The models of `one`, from its stream at time `end`. */
function modelsOf(one: ApprovalCase, end: number): CaseModels | undefined {
  const state = staticState(one.steps, end, one.name);
  const approval = selectApproval(one.turnId, one.callId)(state);
  if (approval === undefined) {
    return undefined;
  }
  const context: ApprovalBlockContext = {
    machine: "desktop",
    cwd: state.turns.get(one.turnId)?.cwd,
    connected: true,
    expiry: undefined,
    answer: answerOf(one),
  };
  const waiting = pendingApprovals(state);
  return {
    one,
    strip: approvalStrip(approval, context, { index: 0, count: Math.max(1, waiting.length) }),
    sheet: approvalSheet(approval, context, waiting),
  };
}

/** Every case with its models. */
function sheetModels(): CaseModels[] {
  let n = 0;
  const end = Date.now();
  return approvalCases(() => {
    n += 1;
    return `0199d3e0-de36-7000-8000-${n.toString(16).padStart(12, "0")}`;
  }).flatMap((one) => {
    const models = modelsOf(one, end);
    return models === undefined ? [] : [models];
  });
}

/** The S-Approvals board. */
export function ApprovalsSheet() {
  const [cases] = useState(sheetModels);
  return (
    <SheetScroll>
      {cases.map(({ one, strip, sheet }) => (
        <SheetCase key={one.name} name={one.name}>
          <View className="gap-12">
            {strip === undefined ? null : (
              <View className="overflow-hidden rounded-box border border-border-subtle">
                <ApprovalStripView
                  model={strip}
                  onOpen={() => undefined}
                  onAnswer={() => undefined}
                />
              </View>
            )}
            <View className="overflow-hidden rounded-box border border-border-subtle">
              <ApprovalSheetView
                model={sheet}
                onAnswer={() => undefined}
                onClose={() => undefined}
                onNext={() => undefined}
              />
            </View>
          </View>
        </SheetCase>
      ))}
    </SheetScroll>
  );
}
