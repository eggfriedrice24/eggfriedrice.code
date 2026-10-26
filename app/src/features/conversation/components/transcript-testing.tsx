// Helpers for the component tests of the transcript: a scope over a real stream store
// and a real row state, with actions that record their calls. Only test files import
// this module.

import { createStore, type Store } from "@tanstack/store";
import type { ReactNode } from "react";
import { Text } from "react-native";

import type { StreamState } from "@/lib/stream/reducer";
import { createRowUiStore } from "../model/row-ui-state";
import {
  type ApprovalRowRef,
  type TranscriptActions,
  type TranscriptScope,
  TranscriptScopeContext,
} from "./use-transcript-scope";

/** The calls of the actions of a test scope, in order. */
export type ActionCall =
  | { readonly action: "withdraw"; readonly turnId: string }
  | { readonly action: "retry"; readonly turnId: string; readonly text: string }
  | { readonly action: "editAndResend"; readonly text: string }
  | { readonly action: "resend"; readonly key: string };

/** A scope for a test, and what it records. */
export interface TestScope {
  readonly scope: TranscriptScope;
  readonly store: Store<StreamState>;
  readonly calls: ActionCall[];
}

/** A scope over `state` on the machine "desktop". The approval block is a plain text. */
export function testScope(state: StreamState): TestScope {
  const store = createStore(state);
  const calls: ActionCall[] = [];
  const actions: TranscriptActions = {
    withdraw: (turnId) => calls.push({ action: "withdraw", turnId }),
    retry: (turnId, text) => calls.push({ action: "retry", turnId, text }),
    editAndResend: (text) => calls.push({ action: "editAndResend", text }),
    resend: (key) => calls.push({ action: "resend", key }),
  };
  return {
    store,
    calls,
    scope: {
      store,
      rowUi: createRowUiStore(),
      machineName: "desktop",
      actions,
      renderApproval: (row: ApprovalRowRef) => <Text>{`approval ${row.callId}`}</Text>,
    },
  };
}

/** `children` inside the context of `scope`. */
export function InScope(props: { readonly scope: TranscriptScope; readonly children: ReactNode }) {
  return <TranscriptScopeContext value={props.scope}>{props.children}</TranscriptScopeContext>;
}
