// The list of the transcript (plan section 2.7): the one component that knows the list
// library, so the library can change after the device spike (design item 8.15) without
// a change to the rows.
//
// Option A: an inverted FlashList 2.3.2, newest row first, on a KeyboardChatScrollView
// that lifts the content with the keyboard only when the reader is at the end
// (`whenAtEnd`), so typing while a reply streams never pulls a reader out of the
// history. New rows keep the reader's place unless the reader is within 200 pt of the
// newest row; while you read above, streaming text never moves what you look at.
//
// - `rows` come oldest first; the list reverses them, because it is inverted. In an
//   inverted list the end is offset 0.
// - `onFollowChange` says whether the reader is within one screen of the end (design
//   3.12: "Jump to latest" shows beyond it). It is called only on a change.
// - The handle (the React 19 `ref` prop) scrolls to the end, and to a row such as an
//   approval.
// - The transcript sits under the transparent header and above the docked composer:
//   `insets` pads the content by their heights. The items are 12 pt apart (design item
//   8.3) and 20 pt from the screen edges.

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
  readonly insets: TranscriptInsets;
  readonly ref?: Ref<TranscriptListHandle>;
}

/** New rows keep the reader's place unless the reader is this close to the end (Happy's value). */
const POSITION = { autoscrollToTopThreshold: 200 } as const;

// The list passes its ref in the props (React 19), and the view passes it on.
function ChatScroll(props: ScrollViewProps) {
  return <KeyboardChatScrollView {...props} inverted keyboardLiftBehavior="whenAtEnd" />;
}

function Gap() {
  return <View className="h-item" />;
}

/** The rows of one transcript, newest at the bottom. */
export function TranscriptList({ ref, ...props }: TranscriptListProps) {
  const list = useRef<FlashListRef<RowKey>>(null);
  const following = useRef(true);
  const data = reversed(props.rows);
  useImperativeHandle(ref, () => ({
    scrollToEnd: (animated) => {
      list.current?.scrollToOffset({ offset: 0, animated });
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
    const { contentOffset, layoutMeasurement } = event.nativeEvent;
    const next = contentOffset.y <= layoutMeasurement.height;
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
      inverted
      maintainVisibleContentPosition={POSITION}
      renderScrollComponent={ChatScroll}
      ItemSeparatorComponent={Gap}
      contentContainerStyle={{
        paddingHorizontal: SPACING.gutter,
        // Inverted: the top of the content is the bottom of the screen.
        paddingTop: props.insets.bottom + SPACING.item,
        paddingBottom: props.insets.top + SPACING.item,
      }}
      onScroll={onScroll}
      scrollEventThrottle={16}
      keyboardDismissMode="interactive"
      keyboardShouldPersistTaps="handled"
    />
  );
}

/** A new array with the items of `items` in the other order. */
function reversed<T>(items: readonly T[]): T[] {
  const out: T[] = [];
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i];
    if (item !== undefined) {
      out.push(item);
    }
  }
  return out;
}
