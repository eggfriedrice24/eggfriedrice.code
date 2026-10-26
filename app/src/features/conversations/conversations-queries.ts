// The queries of the conversation lists of each machine: the recent list (the title of a
// conversation, approvals elsewhere) and the search (the conversations sheet). Plan section 2.2.
//
// - Keys start with `machineKey(machine)`, so a `ready` of the machine invalidates them
//   and Forget removes them.
// - The recent list polls every APPROVAL_POLL_MS while it has an observer and the app
//   is active (D7): the protocol has no feed of list changes.
// - The recent list and the empty search go to the disk (`meta.persist`), so the title
//   menu and the sheet show them before the machine connects. A persisted query keeps
//   its memory copy as long as the disk copy, so that the next write keeps it.
// - The sheet asks every machine with `useQueries({ queries: machines.map(...),
//   combine: combineConversationLists })`.

import {
  keepPreviousData,
  type QueryClient,
  queryOptions,
  type QueryObserverResult,
} from "@tanstack/react-query";

import { machineKey } from "@/lib/query/keys";
import type { MachineCaller } from "@/lib/query/machine-caller";
import { PERSIST_MAX_AGE_MS } from "@/lib/query/persister";
import type {
  ConversationSummary,
  ConversationsRecentResult,
  ConversationsSearchResult,
} from "@/lib/protocol/generated";
import { conversationItem, type ConversationItem } from "./model/conversation-item";

/** How often a recent list asks efrd again while it is on screen (D7). */
export const APPROVAL_POLL_MS = 10_000;

/** The most conversations of a recent list. */
export const RECENT_LIMIT = 20;

/** The most rows of a search, per machine. */
export const SEARCH_LIMIT = 50;

/** How long a search that is not on screen stays in memory (the default of Query). */
export const SEARCH_GC_MS = 5 * 60 * 1000;

/** The query keys of the conversation lists. */
export const conversationsKeys = {
  /** Every list of `machine`. */
  lists: (machine: string) => [...machineKey(machine), "conversations"] as const,
  recent: (machine: string) => [...machineKey(machine), "conversations", "recent"] as const,
  /** The search for `query`; the key holds the trimmed query. */
  search: (machine: string, query: string) =>
    [...machineKey(machine), "conversations", "search", query.trim()] as const,
};

/**
 * Writes `summary`, the summary of a snapshot of a conversation of `machine`, into the
 * lists of `machine` in `client`: the bar and the sheet show its new status at
 * once, not on the next poll. A recent list that does not hold the conversation asks
 * efrd again.
 */
export function applySummary(
  client: QueryClient,
  machine: string,
  summary: ConversationSummary,
): void {
  const recentKey = conversationsKeys.recent(machine);
  const recent = client.getQueryData<ConversationsRecentResult>(recentKey);
  if (recent !== undefined) {
    if (recent.conversations.some((row) => row.summary.id === summary.id)) {
      client.setQueryData<ConversationsRecentResult>(recentKey, (held) =>
        held === undefined
          ? held
          : {
              ...held,
              conversations: held.conversations.map((row) =>
                row.summary.id === summary.id && !sameSummary(row.summary, summary)
                  ? { ...row, summary }
                  : row,
              ),
            },
      );
    } else {
      void client.invalidateQueries({ queryKey: recentKey });
    }
  }
  client.setQueriesData<ConversationsSearchResult>(
    { queryKey: [...conversationsKeys.lists(machine), "search"] },
    (held) =>
      held === undefined
        ? held
        : {
            ...held,
            rows: held.rows.map((row) =>
              row.conversation.summary.id === summary.id &&
              !sameSummary(row.conversation.summary, summary)
                ? { ...row, conversation: { ...row.conversation, summary } }
                : row,
            ),
          },
  );
}

function sameSummary(a: ConversationSummary, b: ConversationSummary): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** The conversations of a `conversations.recent` result of `machine`. */
export function recentItems(
  machine: string,
  result: ConversationsRecentResult,
): ConversationItem[] {
  return result.conversations.map((row) => conversationItem(machine, row.summary));
}

/** The conversations of a `conversations.search` result of `machine`, best first. */
export function searchItems(
  machine: string,
  result: ConversationsSearchResult,
): ConversationItem[] {
  return result.rows.map((row) => conversationItem(machine, row.conversation.summary));
}

/** The recent conversations of `machine`, newest first. */
export function recentConversationsQueryOptions(caller: MachineCaller, machine: string) {
  return queryOptions({
    queryKey: conversationsKeys.recent(machine),
    queryFn: ({ signal }) =>
      caller.call(machine, "conversations.recent", { limit: RECENT_LIMIT }, signal),
    select: (result: ConversationsRecentResult) => recentItems(machine, result),
    meta: { persist: true },
    gcTime: PERSIST_MAX_AGE_MS,
    refetchInterval: APPROVAL_POLL_MS,
  });
}

/**
 * The conversations of `machine` that match `query`, best first. An empty query gives
 * the recent ones, newest first; only that one goes to the disk. While a new query
 * runs, the rows of the last one stay.
 */
export function searchConversationsQueryOptions(
  caller: MachineCaller,
  machine: string,
  query: string,
) {
  const trimmed = query.trim();
  const persisted = trimmed.length === 0;
  return queryOptions({
    queryKey: conversationsKeys.search(machine, trimmed),
    queryFn: ({ signal }) =>
      caller.call(machine, "conversations.search", { query: trimmed, limit: SEARCH_LIMIT }, signal),
    select: (result: ConversationsSearchResult) => searchItems(machine, result),
    meta: { persist: persisted },
    gcTime: persisted ? PERSIST_MAX_AGE_MS : SEARCH_GC_MS,
    placeholderData: keepPreviousData,
  });
}

/** The lists of several machines, as `useQueries` gives them to `combine`. */
export interface CombinedConversationLists {
  /** One list per machine, in the order of the queries; empty while one has no data. */
  readonly lists: readonly (readonly ConversationItem[])[];
  /** True while no machine has data yet and one still fetches. */
  readonly isPending: boolean;
  /** True while any machine fetches. */
  readonly isFetching: boolean;
  /** The number of machines whose last fetch failed. */
  readonly failed: number;
}

/** The members of a query result that `combineConversationLists` reads. */
export type ListResult = Pick<
  QueryObserverResult<ConversationItem[]>,
  "data" | "isPending" | "isFetching" | "isError"
>;

/** The `combine` of the sheet's `useQueries` over the machines. */
export function combineConversationLists(
  results: readonly ListResult[],
): CombinedConversationLists {
  return {
    lists: results.map((result) => result.data ?? []),
    isPending:
      results.length > 0 &&
      results.every((result) => result.data === undefined) &&
      results.some((result) => result.isPending),
    isFetching: results.some((result) => result.isFetching),
    failed: results.filter((result) => result.isError).length,
  };
}

/**
 * The `useQueries` options of the sheet: the search for `query` on each of `machines`
 * (the machine on screen first), joined by `combineConversationLists`.
 */
export function sheetQueriesOptions(
  caller: MachineCaller,
  machines: readonly string[],
  query: string,
) {
  return {
    queries: machines.map((machine) => searchConversationsQueryOptions(caller, machine, query)),
    combine: combineConversationLists,
  };
}
