// The transcript of one conversation: the list of its rows, the loading line, and
// "Jump to latest". The conversation screen renders it with the scope of its stream
// (the store, the row state, the machine name, the actions and the approval block).
//
// - The rows come from the structure selector, which changes only when a row comes or
//   goes; a row reads its own part. Prompts that the outbox holds and efrd has not
//   echoed yet follow the rows of the stream.
// - Until the cache or efrd gave events, the loading line shows.
// - "Jump to latest" shows while the reader is more than one screen from the end, as in
//   most chat apps. The user asked for it there, not only after new content came.
//
// The screen keeps one Transcript while it shows, also when its stream store changes
// (the cache first, then the held stream). Each store has its own row selector
// (`rowsSelectorOf`), so the rows keep their memo. A new Transcript would make a new
// scroll view, and react-native-screens sets the scroll edge effects of iOS 26 only on
// the scroll view that is there when the screen appears.

import { useSelector } from "@tanstack/react-store";
import { type Ref, useImperativeHandle, useRef, useState } from "react";
import { View } from "react-native";

import { runningTurn } from "@/lib/stream/reducer";
import type { Row } from "../model/rows";
import { compareRows, rowsSelectorOf } from "../model/selectors";
import { JumpToLatest } from "./jump-to-latest";
import { LoadingLine } from "./loading-line";
import {
  type TranscriptInsets,
  TranscriptList,
  type TranscriptListHandle,
} from "./transcript-list";
import { type TranscriptItem, TranscriptRow } from "./transcript-row";
import { type TranscriptScope, TranscriptScopeContext } from "./use-transcript-scope";
import type { PendingPrompt } from "./your-message";

export interface TranscriptProps {
  readonly scope: TranscriptScope;
  /** The prompts of the outbox for this conversation that efrd has not echoed yet. */
  readonly pending?: readonly PendingPrompt[];
  readonly insets: TranscriptInsets;
  /** The reader got near the oldest row: fetch older history. */
  readonly onReachTop?: () => void;
  /** The handle of the list, to scroll to an approval (the React 19 `ref` prop). */
  readonly ref?: Ref<TranscriptListHandle>;
}

const NO_PENDING: readonly PendingPrompt[] = [];

/** The transcript of one conversation. */
export function Transcript({ ref, ...props }: TranscriptProps) {
  const scope = props.scope;
  const selectRows = rowsSelectorOf(scope.store);
  const rows = useSelector(scope.store, selectRows, { compare: compareRows });
  const loaded = useSelector(scope.store, (state) => state.loaded);
  // True while the reader is more than one screen from the end.
  const [away, setAway] = useState(false);
  const list = useRef<TranscriptListHandle>(null);
  useImperativeHandle(ref, () => ({
    scrollToEnd: (animated) => list.current?.scrollToEnd(animated),
    scrollToKey: (key, animated) => list.current?.scrollToKey(key, animated),
  }));
  const items = transcriptItems(rows, props.pending ?? NO_PENDING);
  const byKey = new Map(items.map((item) => [item.key, item]));
  const reachTop: { onReachTop?: () => void } = {};
  if (props.onReachTop !== undefined) {
    reachTop.onReachTop = props.onReachTop;
  }
  const jump = (animated: boolean): void => {
    list.current?.scrollToEnd(animated);
    setAway(false);
  };
  return (
    <TranscriptScopeContext value={scope}>
      <View className="flex-1 bg-surface-1">
        <TranscriptList
          ref={list}
          rows={items.map((item) => item.key)}
          renderRow={(key) => {
            const item = byKey.get(key);
            return item === undefined ? null : <TranscriptRow item={item} />;
          }}
          rowType={(key) => byKey.get(key)?.kind ?? "pending"}
          onFollowChange={(following) => {
            setAway(!following);
          }}
          contentMark={() => {
            const state = scope.store.get();
            return {
              seq: state.lastSeq,
              running: runningTurn(state),
              rows: items.length,
              rowUi: scope.rowUi.store.get(),
            };
          }}
          insets={props.insets}
          {...reachTop}
        />
        {!loaded && items.length === 0 ? (
          <View
            className="absolute right-0 left-0 px-gutter"
            style={{ top: props.insets.top + 12 }}
          >
            <LoadingLine machineName={scope.machineName} />
          </View>
        ) : null}
        <JumpToLatest visible={away} bottom={props.insets.bottom} onJump={jump} />
      </View>
    </TranscriptScopeContext>
  );
}

/** The rows of the stream, then the prompts of the outbox, oldest first. */
export function transcriptItems(
  rows: readonly Row[],
  pending: readonly PendingPrompt[],
): TranscriptItem[] {
  const items: TranscriptItem[] = [...rows];
  for (const prompt of pending) {
    items.push({ kind: "pending", key: `o:${prompt.key}`, prompt });
  }
  return items;
}
