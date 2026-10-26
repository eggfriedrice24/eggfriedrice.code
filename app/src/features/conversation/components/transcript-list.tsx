// The list of the transcript (plan section 2.7): the one component that knows the list
// library, so the library can change after the device spike (design item 8.15) without
// a change to the rows.
//
// A FlashList 2.3.2 in chat mode, oldest row first, as t3code has it. It is not
// inverted: on iOS 26 the scroll edge effect of the bar is drawn in the coordinates of
// the scroll view, and on a flipped list it covered the whole list. The list starts at
// the bottom (`startRenderingFromBottom`). New content keeps the reader's place, unless
// the reader is within 200 pt of the end: then the list scrolls with it
// (model/list-position.ts, `createEndFollow`). A row that the user opens or closes never
// scrolls the list, so the row stays where it is. The scroll view is
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
//   `insets` pads the content by their heights. The space under the last row is a row
//   of its own (END_KEY), not a padding of the content: FlashList fills a short list
//   from the top with a margin of the height of the list less its rows and the top
//   padding, so a bottom padding made a short list taller than the screen, and the user
//   could drag it. A list that fits does not bounce either. With the keyboard up, the
//   inset of the keyboard makes it scroll, so a drag still closes the keyboard, and a
//   tap on the empty space closes it too (`keyboardShouldPersistTaps="handled"`). The items are 12 pt apart (design item
//   8.3) and 20 pt from the screen edges. The answer is 16 pt under your prompt, and a
//   new turn is 20 pt under the turn before it (`rowGap` of model/rows.ts).

import { FlashList, type FlashListRef } from "@shopify/flash-list";
import { type ReactNode, type Ref, useImperativeHandle, useRef, useState } from "react";
import {
  type NativeScrollEvent,
  type NativeSyntheticEvent,
  type ScrollViewProps,
  View,
} from "react-native";
import { KeyboardChatScrollView } from "react-native-keyboard-controller";

import { reduceMotion } from "@/lib/native/reduce-motion";
import { SPACING } from "@/theme/metrics";

import { createEndFollow, isNearEnd, type ContentMark } from "../model/list-position";
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
  /**
   * What the content holds now. The list reads it when the size of its content changes,
   * to tell new content from a row that the user opened or closed.
   */
  readonly contentMark: () => ContentMark;
  /** The reader got near the oldest row. */
  readonly onReachTop?: () => void;
  readonly insets: TranscriptInsets;
  readonly ref?: Ref<TranscriptListHandle>;
}

/** The key of the space under the last row, which no row of model/rows.ts has. */
export const END_KEY = "end";

/** The top is near when the oldest row is less than one screen away. */
const TOP_THRESHOLD = 1;

/**
 * Chat mode: start at the bottom. The list follows the end with its own rule
 * (`createEndFollow`), not with `autoscrollToBottomThreshold` of FlashList, which also
 * follows a row that gets shorter or longer.
 */
const POSITION = { startRenderingFromBottom: true } as const;

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

/** The space between two rows. The space under the last row has its own height. */
export function Gap(props: GapProps) {
  if (props.trailingItem === END_KEY) {
    return null;
  }
  return (
    <View testID="row-gap" className={GAP_CLASS[rowGap(props.leadingItem, props.trailingItem)]} />
  );
}

/** The rows of one transcript, newest at the bottom. */
export function TranscriptList({ ref, ...props }: TranscriptListProps) {
  const list = useRef<FlashListRef<RowKey>>(null);
  const following = useRef(true);
  const [follow] = useState(createEndFollow);
  const data = [...props.rows, END_KEY];
  const endSpace = props.insets.bottom + SPACING.item;
  useImperativeHandle(ref, () => ({
    scrollToEnd: (animated) => {
      follow.jumped();
      list.current?.scrollToEnd({ animated });
    },
    scrollToKey: (key, animated) => {
      const index = data.indexOf(key);
      if (index >= 0) {
        follow.movedAway();
        void list.current?.scrollToIndex({ index, animated, viewPosition: 0.5 });
      }
    },
  }));
  const onFollowChange = props.onFollowChange;
  const contentMark = props.contentMark;
  const onScroll = (event: NativeSyntheticEvent<NativeScrollEvent>): void => {
    follow.scrolled(event.nativeEvent);
    const next = isNearEnd(event.nativeEvent);
    if (next !== following.current) {
      following.current = next;
      onFollowChange(next);
    }
  };
  const onContentSizeChange = (): void => {
    if (follow.contentChanged(contentMark())) {
      list.current?.scrollToEnd({ animated: !reduceMotion() });
    }
  };
  const renderRow = props.renderRow;
  return (
    <FlashList
      ref={list}
      data={data}
      renderItem={({ item }) =>
        item === END_KEY ? (
          <View testID="list-end" style={{ height: endSpace }} />
        ) : (
          <>{renderRow(item)}</>
        )
      }
      keyExtractor={(key) => key}
      getItemType={(key) => (key === END_KEY ? END_KEY : props.rowType(key))}
      maintainVisibleContentPosition={POSITION}
      renderScrollComponent={ChatScroll}
      ItemSeparatorComponent={Gap}
      contentContainerStyle={{
        paddingHorizontal: SPACING.gutter,
        paddingTop: props.insets.top + SPACING.item,
      }}
      alwaysBounceVertical={false}
      onScroll={onScroll}
      onScrollBeginDrag={() => follow.dragStarted()}
      onScrollEndDrag={(event) => follow.dragEnded(event.nativeEvent)}
      onMomentumScrollBegin={() => follow.momentumStarted()}
      onMomentumScrollEnd={(event) => follow.momentumEnded(event.nativeEvent)}
      onContentSizeChange={onContentSizeChange}
      onStartReached={props.onReachTop}
      onStartReachedThreshold={TOP_THRESHOLD}
      scrollEventThrottle={16}
      keyboardDismissMode="interactive"
      keyboardShouldPersistTaps="handled"
    />
  );
}
