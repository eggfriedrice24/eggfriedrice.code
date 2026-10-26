import { readFileSync } from "node:fs";
import { join } from "node:path";

import { QueryObserver, environmentManager, focusManager } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  ConversationsRecentResult,
  ConversationsSearchResult,
  ConversationSummary,
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
