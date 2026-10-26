// The transcript of one conversation: the list of its rows, the loading line, and
// "Jump to latest". The conversation screen renders it with the scope of its stream
// (the store, the row state, the machine name, the actions and the approval block).
//
// - The rows come from the structure selector, which changes only when a row comes or
//   goes; a row reads its own part. Prompts that the outbox holds and efrd has not
//   echoed yet follow the rows of the stream.
// - Until the cache or efrd gave events, the loading line shows.
// - "Jump to latest" shows when the reader is more than one screen from the end and
//   new content came after the reader left it.
//
// Give each conversation its own Transcript (a `key` of the conversation), because its
// row selector keeps a memo of one stream.

import { useSelector } from "@tanstack/react-store";
import { type Ref, useImperativeHandle, useRef, useState } from "react";
import { View } from "react-native";

import type { StreamState } from "@/lib/stream/reducer";
import type { Row } from "../model/rows";
import { compareRows, createRowsSelector } from "../model/selectors";
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
  /** The handle of the list, to scroll to an approval (the React 19 `ref` prop). */
  readonly ref?: Ref<TranscriptListHandle>;
}

const NO_PENDING: readonly PendingPrompt[] = [];

/** The new content of `state` since `left`: what makes "Jump to latest" show. */
function changedSince(left: StreamState | undefined): (state: StreamState) => boolean {
  return (state) =>
    left !== undefined && (state.lastSeq !== left.lastSeq || state.turns !== left.turns);
}

/** The transcript of one conversation. */
export function Transcript({ ref, ...props }: TranscriptProps) {
  const scope = props.scope;
  const [selectRows] = useState(createRowsSelector);
  const rows = useSelector(scope.store, selectRows, { compare: compareRows });
  const loaded = useSelector(scope.store, (state) => state.loaded);
  // The state when the reader left the end; undefined while the list follows.
  const [leftAt, setLeftAt] = useState<StreamState | undefined>(undefined);
  const changed = useSelector(scope.store, changedSince(leftAt));
  const list = useRef<TranscriptListHandle>(null);
  useImperativeHandle(ref, () => ({
    scrollToEnd: (animated) => list.current?.scrollToEnd(animated),
    scrollToKey: (key, animated) => list.current?.scrollToKey(key, animated),
  }));
  const items = transcriptItems(rows, props.pending ?? NO_PENDING);
  const byKey = new Map(items.map((item) => [item.key, item]));
  const jump = (animated: boolean): void => {
    list.current?.scrollToEnd(animated);
    setLeftAt(undefined);
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
            setLeftAt(following ? undefined : scope.store.get());
          }}
          insets={props.insets}
        />
        {!loaded && items.length === 0 ? (
          <View
            className="absolute right-0 left-0 px-gutter"
            style={{ top: props.insets.top + 12 }}
          >
            <LoadingLine machineName={scope.machineName} />
          </View>
        ) : null}
        <JumpToLatest
          visible={leftAt !== undefined && changed}
          bottom={props.insets.bottom}
          onJump={jump}
        />
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
