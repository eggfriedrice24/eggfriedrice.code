// The transcript boards of the gallery (S-Messages, S-Markdown, S-Tools, S-Diff): the real
// transcript of the app over a stream state that a demo script gives at once
// (model/static-stream.ts). The rows are the rows of a conversation screen; the actions
// of the rows do nothing.

import { createStore } from "@tanstack/store";
import { useHeaderHeight } from "expo-router/react-navigation";
import { useState } from "react";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { StreamState } from "@/lib/stream/reducer";
import { ApprovalBlockView } from "@/features/conversation/components/approval-block";
import { Transcript } from "@/features/conversation/components/transcript";
import type {
  ApprovalRowRef,
  TranscriptActions,
  TranscriptScope,
} from "@/features/conversation/components/use-transcript-scope";
import { approvalBlock, NO_ANSWER } from "@/features/conversation/model/approval-block";
import { createRowUiStore } from "@/features/conversation/model/row-ui-state";
import { selectApproval } from "@/features/conversation/model/selectors";
import { demoConversation, type DemoConversationKind } from "../model/demo-conversations";
import { staticState } from "../model/static-stream";

/** The actions of the rows of a sheet: none. */
const NO_ACTIONS: TranscriptActions = {
  withdraw: () => undefined,
  retry: () => undefined,
  editAndResend: () => undefined,
  resend: () => undefined,
};

/** The approval block of a row of `state`. */
function approvalRow(state: StreamState, row: ApprovalRowRef) {
  const approval = selectApproval(row.turnId, row.callId)(state);
  if (approval === undefined) {
    return null;
  }
  const model = approvalBlock(approval, {
    machine: "desktop",
    cwd: state.turns.get(row.turnId)?.cwd,
    connected: true,
    expiry: undefined,
    answer: NO_ANSWER,
  });
  return <ApprovalBlockView model={model} onAnswer={() => undefined} />;
}

/** The scope of a sheet of `kind`; with `expanded`, every tool block shows in full. */
export function sheetScope(kind: DemoConversationKind, expanded: boolean): TranscriptScope {
  let n = 0;
  const script = demoConversation(kind, () => {
    n += 1;
    return `0199d3e0-de37-7000-8000-${n.toString(16).padStart(12, "0")}`;
  });
  const state = staticState(script.history, Date.now(), script.title);
  const rowUi = createRowUiStore();
  if (expanded) {
    for (const [turnId, turn] of state.turns) {
      for (const callId of turn.tools.keys()) {
        rowUi.set(`t:${turnId}:${callId}`, "expanded", true);
      }
    }
  }
  return {
    store: createStore(state),
    rowUi,
    machineName: "desktop",
    actions: NO_ACTIONS,
    renderApproval: (row) => approvalRow(state, row),
  };
}

export interface TranscriptSheetProps {
  readonly kind: DemoConversationKind;
  /** True to show every tool block in full (S-Diff: the edit block expanded). */
  readonly expanded?: boolean;
}

/** A transcript board. */
export function TranscriptSheet(props: TranscriptSheetProps) {
  const [scope] = useState(() => sheetScope(props.kind, props.expanded === true));
  const top = useHeaderHeight();
  const bottom = useSafeAreaInsets().bottom;
  return <Transcript scope={scope} insets={{ top: top + 12, bottom: bottom + 12 }} />;
}
