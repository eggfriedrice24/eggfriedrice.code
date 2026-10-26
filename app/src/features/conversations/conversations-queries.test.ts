import { readFileSync } from "node:fs";
import { join } from "node:path";

import { QueryObserver, environmentManager, focusManager } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  ConversationStatus,
  ConversationsRecentResult,
  ConversationsSearchResult,
  ConversationSummary,
  RecentConversation,
} from "@/lib/protocol/generated";
import { ConnectionError } from "@/lib/net/errors";
import { machineKey } from "@/lib/query/keys";
import { PERSIST_MAX_AGE_MS } from "@/lib/query/persister";
import { createQueryClient } from "@/lib/query/query-client";
import { FakeCaller } from "@/lib/query/testing";
import {
  APPROVAL_POLL_MS,
  RECENT_LIMIT,
  SEARCH_LIMIT,
  applyWatchItem,
  combineConversationLists,
  conversationsKeys,
  recentConversationsQueryOptions,
  recentItems,
  searchConversationsQueryOptions,
  searchItems,
  sheetQueriesOptions,
  type ListResult,
} from "./conversations-queries";
import type { ConversationItem } from "./model/conversation-item";

const FIXTURES = join(import.meta.dirname, "../../../../crates/efr-protocol/fixtures/v1");

function fixture(name: string): string {
  return readFileSync(join(FIXTURES, name), "utf8");
}

function summary(id: string, title: string): ConversationSummary {
  return {
    id,
    title,
    status: "idle",
    created_at: "2026-10-26T10:00:00Z",
    updated_at: "2026-10-26T12:00:00Z",
    last_seq: 1,
  };
}

function recent(...titles: string[]): ConversationsRecentResult {
  return {
    conversations: titles.map((title, index) => ({
      summary: summary(`c${index}`, title),
      route: { kind: "outside" },
      turns: 1,
    })),
  };
}

function search(...titles: string[]): ConversationsSearchResult {
  return {
    rows: titles.map((title, index) => ({
      conversation: { summary: summary(`s${index}`, title), route: { kind: "outside" }, turns: 1 },
      short_id: `s${index}`,
    })),
  };
}

describe("conversationsKeys", () => {
  it("puts every list under the key of its machine", () => {
    const root = machineKey("m1");
    expect(conversationsKeys.lists("m1").slice(0, 2)).toEqual([...root]);
    expect(conversationsKeys.recent("m1").slice(0, 3)).toEqual([...conversationsKeys.lists("m1")]);
    expect(conversationsKeys.search("m1", "  disk  ")).toEqual([
      "machine",
      "m1",
      "conversations",
      "search",
      "disk",
    ]);
  });
});

describe("the list results of efrd", () => {
  it("reads the fixtures of efr-protocol", () => {
    const recentResult: ConversationsRecentResult = JSON.parse(
      fixture("conversations_recent_result.json"),
    );
    const searchResult: ConversationsSearchResult = JSON.parse(
      fixture("conversations_search_result.json"),
    );
    const expected: ConversationItem = {
      machine: "m1",
      id: "019a9b1c-3d00-7a10-8b20-000000000001",
      title: "why is the disk full",
      status: "running",
      updatedAt: Date.parse("2026-10-03T12:00:05.5Z"),
      cwd: "/var/log",
    };
    expect(recentItems("m1", recentResult)[0]).toEqual(expected);
    expect(searchItems("m1", searchResult)[0]).toEqual(expected);
  });
});

describe("recentConversationsQueryOptions", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // Node has no window, so Query takes it for a server, where it never polls. The
    // app runs on a phone.
    environmentManager.setIsServer(() => false);
  });

  afterEach(() => {
    environmentManager.setIsServer(() => true);
    vi.useRealTimers();
  });

  it("asks the machine for its recent list and gives the items", async () => {
    const caller = new FakeCaller({ "conversations.recent": () => recent("a", "b") });
    const client = createQueryClient();
    const observer = new QueryObserver(client, recentConversationsQueryOptions(caller, "m1"));
    const stop = observer.subscribe(() => undefined);
    await vi.advanceTimersByTimeAsync(0);
    expect(caller.calls).toEqual([
      {
        machine: "m1",
        method: "conversations.recent",
        params: { limit: RECENT_LIMIT },
        signal: expect.any(AbortSignal),
      },
    ]);
    expect(observer.getCurrentResult().data?.map((row) => [row.machine, row.title])).toEqual([
      ["m1", "a"],
      ["m1", "b"],
    ]);
    stop();
  });

  it("asks again every 10 s while it has an observer and the app is active", async () => {
    const caller = new FakeCaller({ "conversations.recent": () => recent("a") });
    const client = createQueryClient();
    const observer = new QueryObserver(client, recentConversationsQueryOptions(caller, "m1"));
    const stop = observer.subscribe(() => undefined);
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(APPROVAL_POLL_MS);
    expect(caller.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(APPROVAL_POLL_MS);
    expect(caller.calls).toHaveLength(3);
    focusManager.setFocused(false);
    await vi.advanceTimersByTimeAsync(3 * APPROVAL_POLL_MS);
    expect(caller.calls).toHaveLength(3);
    focusManager.setFocused(undefined);
    stop();
    await vi.advanceTimersByTimeAsync(5 * APPROVAL_POLL_MS);
    expect(caller.calls).toHaveLength(3);
  });

  it("does not poll while the watch of its machine is live, and polls again after it", async () => {
    const caller = new FakeCaller({ "conversations.recent": () => recent("a") });
    const client = createQueryClient();
    const observer = new QueryObserver(client, recentConversationsQueryOptions(caller, "m1"));
    const stop = observer.subscribe(() => undefined);
    await vi.advanceTimersByTimeAsync(0);
    expect(caller.calls).toHaveLength(1);

    // The first item of the watch writes the list: the poll stops.
    caller.watched.add("m1");
    applyWatchItem(client, "m1", {
      kind: "snapshot",
      conversations: recent("a", "b").conversations,
    });
    await vi.advanceTimersByTimeAsync(5 * APPROVAL_POLL_MS);
    expect(caller.calls).toHaveLength(1);
    expect(observer.getCurrentResult().data?.map((item) => item.title)).toEqual(["a", "b"]);

    // A watch that stopped makes the list ask again, and the poll starts again.
    caller.watched.delete("m1");
    await client.invalidateQueries({ queryKey: conversationsKeys.recent("m1") });
    expect(caller.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(APPROVAL_POLL_MS);
    expect(caller.calls).toHaveLength(3);
    stop();
  });

  it("goes to the disk and stays in memory as long as the disk copy", () => {
    const options = recentConversationsQueryOptions(new FakeCaller(), "m1");
    expect(options.meta).toEqual({ persist: true });
    expect(options.gcTime).toBe(PERSIST_MAX_AGE_MS);
  });
});

describe("searchConversationsQueryOptions", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("sends the trimmed query and persists only the empty one", () => {
    const caller = new FakeCaller();
    expect(searchConversationsQueryOptions(caller, "m1", "").meta).toEqual({ persist: true });
    expect(searchConversationsQueryOptions(caller, "m1", "  ").meta).toEqual({ persist: true });
    expect(searchConversationsQueryOptions(caller, "m1", "disk").meta).toEqual({ persist: false });
  });

  it("keeps the rows of the last query while the next one runs", async () => {
    let answer: ((result: ConversationsSearchResult) => void) | undefined;
    const caller = new FakeCaller({ "conversations.search": () => search("first") });
    const client = createQueryClient();
    const observer = new QueryObserver(client, searchConversationsQueryOptions(caller, "m1", "fi"));
    const stop = observer.subscribe(() => undefined);
    await vi.advanceTimersByTimeAsync(0);
    expect(observer.getCurrentResult().data?.map((row) => row.title)).toEqual(["first"]);

    caller.answer({
      "conversations.search": () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    observer.setOptions(searchConversationsQueryOptions(caller, "m1", "fir "));
    await vi.advanceTimersByTimeAsync(0);
    expect(caller.calls.at(-1)?.params).toEqual({ query: "fir", limit: SEARCH_LIMIT });
    expect(observer.getCurrentResult().isPlaceholderData).toBe(true);
    expect(observer.getCurrentResult().data?.map((row) => row.title)).toEqual(["first"]);

    answer?.(search("firewall"));
    await vi.advanceTimersByTimeAsync(0);
    expect(observer.getCurrentResult().isPlaceholderData).toBe(false);
    expect(observer.getCurrentResult().data?.map((row) => row.title)).toEqual(["firewall"]);
    stop();
  });
});

function result(
  data: ConversationItem[] | undefined,
  flags: {
    readonly isPending?: boolean;
    readonly isFetching?: boolean;
    readonly isError?: boolean;
  },
): ListResult {
  return {
    data,
    isPending: flags.isPending ?? false,
    isFetching: flags.isFetching ?? false,
    isError: flags.isError ?? false,
  };
}

/** A row of a watch: conversation `id`, `minutes` before 12:00, with `status`. */
function watched(
  id: string,
  minutes: number,
  status: ConversationStatus = "idle",
): RecentConversation {
  const at = new Date(Date.parse("2026-10-26T12:00:00Z") - minutes * 60_000).toISOString();
  return {
    summary: { ...summary(id, `title ${id}`), status, updated_at: at },
    route: { kind: "outside" },
    turns: 1,
  };
}

/** A query client and the ids of the recent list of `m1` in it. */
function lists() {
  const client = createQueryClient();
  const recentIds = (): string[] =>
    (
      client.getQueryData<ConversationsRecentResult>(conversationsKeys.recent("m1"))
        ?.conversations ?? []
    ).map((row) => row.summary.id);
  return { client, recentIds };
}

describe("applyWatchItem", () => {
  it("makes a snapshot the recent list, newest first, and updates the rows of a search", () => {
    const { client, recentIds } = lists();
    const found = search("old title");
    client.setQueryData(conversationsKeys.search("m1", "disk"), found);
    const waiting = watched("s0", 1, "awaiting_approval");
    applyWatchItem(client, "m1", { kind: "snapshot", conversations: [watched("a", 5), waiting] });
    expect(recentIds()).toEqual(["s0", "a"]);
    const searched = client.getQueryData<ConversationsSearchResult>(
      conversationsKeys.search("m1", "disk"),
    );
    expect(searched?.rows[0]?.conversation.summary.status).toBe("awaiting_approval");
    expect(searched?.rows[0]?.short_id).toBe("s0");
  });

  it("puts a changed row in its place, keeps at most RECENT_LIMIT rows, and asks the empty search again when it lacks the row", () => {
    const { client, recentIds } = lists();
    const many = Array.from({ length: RECENT_LIMIT }, (_, index) =>
      watched(`r${index}`, index + 1),
    );
    applyWatchItem(client, "m1", { kind: "snapshot", conversations: many });
    client.setQueryData(conversationsKeys.search("m1", ""), search("x"));
    applyWatchItem(client, "m1", { kind: "changed", conversation: watched("new", 0, "running") });
    expect(recentIds()).toHaveLength(RECENT_LIMIT);
    expect(recentIds()[0]).toBe("new");
    expect(recentIds()).not.toContain(`r${RECENT_LIMIT - 1}`);
    expect(client.getQueryState(conversationsKeys.search("m1", ""))?.isInvalidated).toBe(true);

    // The same conversation again moves; it is never in the list twice.
    applyWatchItem(client, "m1", { kind: "changed", conversation: watched("r5", 0, "idle") });
    expect(recentIds().filter((id) => id === "r5")).toHaveLength(1);
  });

  it("drops a removed conversation from every list, and skips a kind that it does not know", () => {
    const { client, recentIds } = lists();
    applyWatchItem(client, "m1", { kind: "snapshot", conversations: [watched("s0", 1)] });
    client.setQueryData(conversationsKeys.search("m1", "disk"), search("a"));
    applyWatchItem(client, "m1", { kind: "other", wireKind: "renamed" });
    expect(recentIds()).toEqual(["s0"]);
    applyWatchItem(client, "m1", { kind: "removed", conversation_id: "s0" });
    expect(recentIds()).toEqual([]);
    expect(
      client.getQueryData<ConversationsSearchResult>(conversationsKeys.search("m1", "disk"))?.rows,
    ).toEqual([]);
  });
});

describe("combineConversationLists", () => {
  it("joins the lists in order and sums up their state", () => {
    const a: ConversationItem = { machine: "a", id: "1", status: "idle", updatedAt: 1 };
    const combined = combineConversationLists([
      result([a], { isFetching: true }),
      result(undefined, { isError: true }),
    ]);
    expect(combined).toEqual({ lists: [[a], []], isPending: false, isFetching: true, failed: 1 });
  });

  it("is pending only while no machine has data", () => {
    expect(combineConversationLists([result(undefined, { isPending: true })]).isPending).toBe(true);
    expect(combineConversationLists([]).isPending).toBe(false);
  });
});

describe("sheetQueriesOptions", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("asks every machine and gives one list per machine, in order", async () => {
    const caller = new FakeCaller({
      "conversations.search": (_params, machine) => {
        if (machine === "homelab") {
          throw new ConnectionError("offline", "not connected");
        }
        return search(`on ${machine}`);
      },
    });
    const options = sheetQueriesOptions(caller, ["desktop", "laptop", "homelab"], "");
    expect(options.queries.map((query) => query.queryKey)).toEqual([
      conversationsKeys.search("desktop", ""),
      conversationsKeys.search("laptop", ""),
      conversationsKeys.search("homelab", ""),
    ]);
    const client = createQueryClient();
    const observers = options.queries.map((query) => new QueryObserver(client, query));
    const stops = observers.map((observer) => observer.subscribe(() => undefined));
    await vi.advanceTimersByTimeAsync(0);
    const combined = options.combine(observers.map((observer) => observer.getCurrentResult()));
    expect(
      combined.lists.map((list) => list.map((row) => `${row.machine} ${row.title ?? ""}`)),
    ).toEqual([["desktop on desktop"], ["laptop on laptop"], []]);
    expect(combined.failed).toBe(1);
    for (const stop of stops) {
      stop();
    }
  });
});
