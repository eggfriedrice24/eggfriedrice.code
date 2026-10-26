// The list of the transcript (plan section 2.7): the one component that knows the list
// library, so the library can change after the device spike (design item 8.15) without
// a change to the rows.
//
// A FlashList 2.3.2 in chat mode, oldest row first, as t3code has it. It is not
// inverted: on iOS 26 the scroll edge effect of the bar is drawn in the coordinates of
// the scroll view, and on a flipped list it covered the whole list. The list starts at
// the bottom (`startRenderingFromBottom`). New rows keep the reader's place, unless the
// reader is within 200 pt of the end: then the list scrolls with them. The scroll view is
// a KeyboardChatScrollView that lifts the content with the keyboard only when the reader
// is at the end (`whenAtEnd`), so typing while a reply streams never pulls a reader out
// of the history.
//
// - `onFollowChange` says whether the reader is within one screen of the end (design
//   3.12: "Jump to latest" shows beyond it). It is called only on a change.
// - `onReachTop` is called when the reader gets near the oldest row, so the screen can
//   fetch older history. The older rows come in at the start, and the list keeps the
//   reader's place.
// - The handle (the React 19 `ref` prop) scrolls to the end, and to a row such as an
//   approval.
// - The transcript sits under the transparent header and above the docked composer:
//   `insets` pads the content by their heights. The items are 12 pt apart (design item
//   8.3) and 20 pt from the screen edges. The answer is 16 pt under your prompt, and a
//   new turn is 20 pt under the turn before it (`rowGap` of model/rows.ts).

import { FlashList, type FlashListRef } from "@shopify/flash-list";
import { type ReactNode, type Ref, useImperativeHandle, useRef } from "react";
import {
  type NativeScrollEvent,
  type NativeSyntheticEvent,
  type ScrollViewProps,
  View,
} from "react-native";
import { KeyboardChatScrollView } from "react-native-keyboard-controller";

import { SPACING } from "@/theme/metrics";

import { isNearEnd } from "../model/list-position";
import { rowGap, type RowGap } from "../model/rows";

/** The key of a row (model/rows.ts), unique in its transcript. */
export type RowKey = string;

/** What the screen can do with the list. */
export interface TranscriptListHandle {
  /** Scrolls to the newest row, and the list follows again. */
  scrollToEnd(animated: boolean): void;
  /** Scrolls to the row with `key`, such as the approval that waits. */
  scrollToKey(key: RowKey, animated: boolean): void;
}

/** The heights of what covers the list: the header above it and the composer below it. */
export interface TranscriptInsets {
  readonly top: number;
  readonly bottom: number;
}

export interface TranscriptListProps {
  /** The rows, oldest first. */
  readonly rows: readonly RowKey[];
  readonly renderRow: (key: RowKey) => ReactNode;
  /** The kind of a row, so the list reuses a view only for a row of the same kind. */
  readonly rowType: (key: RowKey) => string;
  readonly onFollowChange: (following: boolean) => void;
  /** The reader got near the oldest row. */
  readonly onReachTop?: () => void;
  readonly insets: TranscriptInsets;
  readonly ref?: Ref<TranscriptListHandle>;
}

/** The top is near when the oldest row is less than one screen away. */
const TOP_THRESHOLD = 1;

/**
 * Chat mode: start at the bottom, and follow new rows while the reader is this close to
 * the end (Happy's value). The scroll to them is animated, as in a chat.
 */
const POSITION = { startRenderingFromBottom: true, autoscrollToBottomThreshold: 200 } as const;

// The list passes its ref in the props (React 19), and the view passes it on.
function ChatScroll(props: ScrollViewProps) {
  return <KeyboardChatScrollView {...props} keyboardLiftBehavior="whenAtEnd" />;
}

const GAP_CLASS = {
  item: "h-item",
  afterPrompt: "h-after-prompt",
  turn: "h-turn",
} as const satisfies Record<RowGap, string>;

interface GapProps {
  /** The row over the gap. */
  readonly leadingItem: RowKey;
  /** The row under the gap. */
  readonly trailingItem: RowKey;
}

/** The space between two rows. */
export function Gap(props: GapProps) {
  return (
    <View testID="row-gap" className={GAP_CLASS[rowGap(props.leadingItem, props.trailingItem)]} />
  );
}

/** The rows of one transcript, newest at the bottom. */
export function TranscriptList({ ref, ...props }: TranscriptListProps) {
  const list = useRef<FlashListRef<RowKey>>(null);
  const following = useRef(true);
  const data = props.rows;
  useImperativeHandle(ref, () => ({
    scrollToEnd: (animated) => {
      list.current?.scrollToEnd({ animated });
    },
    scrollToKey: (key, animated) => {
      const index = data.indexOf(key);
      if (index >= 0) {
        void list.current?.scrollToIndex({ index, animated, viewPosition: 0.5 });
      }
    },
  }));
  const onFollowChange = props.onFollowChange;
  const onScroll = (event: NativeSyntheticEvent<NativeScrollEvent>): void => {
    const next = isNearEnd(event.nativeEvent);
    if (next !== following.current) {
      following.current = next;
      onFollowChange(next);
    }
  };
  const renderRow = props.renderRow;
  return (
    <FlashList
      ref={list}
      data={data}
      renderItem={({ item }) => <>{renderRow(item)}</>}
      keyExtractor={(key) => key}
      getItemType={(key) => props.rowType(key)}
      maintainVisibleContentPosition={POSITION}
      renderScrollComponent={ChatScroll}
      ItemSeparatorComponent={Gap}
      contentContainerStyle={{
        paddingHorizontal: SPACING.gutter,
        paddingTop: props.insets.top + SPACING.item,
        paddingBottom: props.insets.bottom + SPACING.item,
      }}
      onScroll={onScroll}
      onStartReached={props.onReachTop}
      onStartReachedThreshold={TOP_THRESHOLD}
      scrollEventThrottle={16}
      keyboardDismissMode="interactive"
      keyboardShouldPersistTaps="handled"
    />
  );
}
