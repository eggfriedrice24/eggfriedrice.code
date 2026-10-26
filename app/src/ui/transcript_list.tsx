// The list of the transcript: the one component that knows the list library, so the
// library can change after the device spike (R6 item 4 of the research) without a change
// to the rows. It starts as an inverted FlashList, newest row first, on a
// KeyboardChatScrollView that lifts the content with the keyboard only when the reader
// is at the end (`whenAtEnd`), so typing while a reply streams never pulls a reader out
// of the history. New rows keep the reader's place unless the reader is within 200 points
// of the newest row (Happy's setting).

import { FlashList, type ListRenderItemInfo } from "@shopify/flash-list";
import { memo, useCallback } from "react";
import type { ScrollViewProps } from "react-native";
import { KeyboardChatScrollView } from "react-native-keyboard-controller";

import type { Row } from "./rows";
import { GUTTER } from "./styles";
import { type RowContext, TranscriptRow } from "./transcript_rows";

const CONTENT = { paddingHorizontal: GUTTER, paddingVertical: 8 } as const;

const POSITION = { autoscrollToTopThreshold: 200 } as const;

// The list passes its ref in the props (React 19), and the view passes it on.
function ChatScroll(props: ScrollViewProps) {
  return <KeyboardChatScrollView {...props} inverted keyboardLiftBehavior="whenAtEnd" />;
}

const keyOf = (row: Row) => row.key;

const typeOf = (row: Row) => row.kind;

/** The rows of one transcript, newest first. */
export const TranscriptList = memo(function TranscriptList(props: {
  readonly rows: readonly Row[];
  readonly context: RowContext;
}) {
  const { context } = props;
  const renderItem = useCallback(
    ({ item }: ListRenderItemInfo<Row>) => (
      // The key remounts a recycled cell, so no row keeps the state of another one.
      <TranscriptRow key={item.key} row={item} context={context} />
    ),
    [context],
  );
  return (
    <FlashList
      data={props.rows}
      renderItem={renderItem}
      keyExtractor={keyOf}
      getItemType={typeOf}
      inverted
      maintainVisibleContentPosition={POSITION}
      renderScrollComponent={ChatScroll}
      contentContainerStyle={CONTENT}
      keyboardDismissMode="interactive"
      keyboardShouldPersistTaps="handled"
    />
  );
});
