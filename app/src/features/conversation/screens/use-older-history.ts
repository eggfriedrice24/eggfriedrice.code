// The older history of the conversation on a screen (plan section 2.2). A snapshot holds
// a window of the events, and its `history_cursor` tells where the older events are.
// When the reader gets to the top of the transcript, the screen asks for the pages of
// `conversation.history` before the window, one at a time, and the stream puts each
// page under its state and in the cache (model/conversation-stream.ts).
//
// - The query never fetches by itself (`enabled: false`): only the top of the list asks.
// - A stream that resumed from the cache saw no snapshot, so it has no cursor. Then the
//   pages start at the newest event and go back: a page that the stream holds already
//   adds nothing, and the next one follows at once, until a page adds a turn.
// - A cursor of null tells that the window starts at the first event: there is nothing
//   to fetch.

import {
  type InfiniteData,
  type InfiniteQueryObserverResult,
  useInfiniteQuery,
} from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";

import type { ConversationHistoryResult } from "@/lib/protocol/generated";
import type { StreamState } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { useRuntime } from "@/features/navigation/use-app-services";
import { historyQueryOptions } from "../conversation-queries";
import type { ConversationStream } from "../model/conversation-stream";

/** The fetch of the next page of older history, as Query gives it. */
type FetchOlder = InfiniteQueryObserverResult<
  InfiniteData<ConversationHistoryResult>
>["fetchNextPage"];

/**
 * What the top of the transcript calls: it fetches older history for `stream`, whose
 * state is in `store`. It does nothing while there is no stream, while a fetch runs, and
 * when no older page is left.
 */
export function useOlderHistory(
  stream: ConversationStream | undefined,
  store: Readable<StreamState>,
  ref: ConversationRef,
): () => void {
  const runtime = useRuntime();
  const start = useSelector(store, (state) => state.historyCursor);
  const history = useInfiniteQuery({
    ...historyQueryOptions(runtime, ref, start ?? null),
    enabled: false,
  });
  const done = start === null || (history.data !== undefined && !history.hasNextPage);
  const fetchOlder = history.fetchNextPage;
  return () => {
    if (stream === undefined || done || history.isFetching) {
      return;
    }
    void loadOlder(stream, fetchOlder).catch(() => undefined);
  };
}

/** Fetches pages until one adds a turn to `stream`, or no page is left. */
async function loadOlder(stream: ConversationStream, fetchOlder: FetchOlder): Promise<void> {
  // A second call while a page is on its way waits for that page; it does not cancel it.
  const result = await fetchOlder({ cancelRefetch: false });
  const pages = result.data?.pages ?? [];
  const page = pages[pages.length - 1];
  if (result.isError || page === undefined) {
    return;
  }
  const before = stream.latest().order.length;
  stream.older(page.events);
  if (stream.latest().order.length > before || !result.hasNextPage) {
    return;
  }
  await loadOlder(stream, result.fetchNextPage);
}
