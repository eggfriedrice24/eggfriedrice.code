import { QueryClient } from "@tanstack/react-query";
import {
  persistQueryClientRestore,
  persistQueryClientSave,
  type PersistedClient,
} from "@tanstack/react-query-persist-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  PERSIST_KEY,
  PERSIST_MAX_AGE_MS,
  PERSIST_SCHEMA_VERSION,
  PERSIST_THROTTLE_MS,
  persistBuster,
  queryPersistOptions,
} from "./persister";
import { MemoryStorage } from "./testing";

const START = Date.UTC(2026, 9, 26, 12, 0, 0);

function filledClient(): QueryClient {
  const client = new QueryClient();
  const recent = client.getQueryCache().build(client, {
    queryKey: ["machine", "m1", "conversations", "recent"],
    meta: { persist: true },
  });
  recent.setData({ conversations: [] });
  client.setQueryData(["machine", "m1", "conversation", "c1", "history", null], { pages: [] });
  return client;
}

function persistedKeys(storage: MemoryStorage): unknown[] {
  const text = storage.values.get(PERSIST_KEY);
  if (text === undefined) {
    return [];
  }
  const parsed: PersistedClient = JSON.parse(text);
  return parsed.clientState.queries.map((query) => query.queryKey);
}

async function save(client: QueryClient, storage: MemoryStorage, version: string): Promise<void> {
  const options = queryPersistOptions(storage, version);
  await persistQueryClientSave({ queryClient: client, ...options });
  await vi.advanceTimersByTimeAsync(PERSIST_THROTTLE_MS);
}

describe("queryPersistOptions", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(START);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("writes only the queries with meta.persist and a result", async () => {
    const storage = new MemoryStorage();
    const client = filledClient();
    const failed = client.getQueryCache().build(client, {
      queryKey: ["machine", "m1", "conversations", "search", ""],
      meta: { persist: true },
    });
    failed.setState({ status: "error", error: new Error("offline") });
    await save(client, storage, "1.0.0");
    expect(persistedKeys(storage)).toEqual([["machine", "m1", "conversations", "recent"]]);
  });

  it("writes no mutation", async () => {
    const storage = new MemoryStorage();
    const client = filledClient();
    client.getMutationCache().build(
      client,
      { mutationKey: ["m"] },
      {
        context: undefined,
        data: undefined,
        error: null,
        failureCount: 0,
        failureReason: null,
        isPaused: true,
        status: "pending",
        variables: { text: "x" },
        submittedAt: START,
      },
    );
    await save(client, storage, "1.0.0");
    const text = storage.values.get(PERSIST_KEY) ?? "";
    const parsed: PersistedClient = JSON.parse(text);
    expect(parsed.clientState.mutations).toEqual([]);
  });

  it("gives the restored lists back to a new client", async () => {
    const storage = new MemoryStorage();
    await save(filledClient(), storage, "1.0.0");
    const fresh = new QueryClient();
    await persistQueryClientRestore({
      queryClient: fresh,
      ...queryPersistOptions(storage, "1.0.0"),
    });
    expect(fresh.getQueryData(["machine", "m1", "conversations", "recent"])).toEqual({
      conversations: [],
    });
    expect(fresh.getQueryData(["machine", "m1", "conversation", "c1", "history", null])).toBe(
      undefined,
    );
  });

  it("drops the copy of another app version", async () => {
    const storage = new MemoryStorage();
    await save(filledClient(), storage, "1.0.0");
    const fresh = new QueryClient();
    await persistQueryClientRestore({
      queryClient: fresh,
      ...queryPersistOptions(storage, "1.0.1"),
    });
    expect(fresh.getQueryData(["machine", "m1", "conversations", "recent"])).toBeUndefined();
    expect(storage.values.has(PERSIST_KEY)).toBe(false);
  });

  it("drops a copy older than 7 days", async () => {
    const storage = new MemoryStorage();
    await save(filledClient(), storage, "1.0.0");
    vi.setSystemTime(START + PERSIST_MAX_AGE_MS + 1);
    const fresh = new QueryClient();
    await persistQueryClientRestore({
      queryClient: fresh,
      ...queryPersistOptions(storage, "1.0.0"),
    });
    expect(fresh.getQueryData(["machine", "m1", "conversations", "recent"])).toBeUndefined();
  });

  it("removes the lists of a forgotten machine from the disk on the next write", async () => {
    const storage = new MemoryStorage();
    const client = filledClient();
    await save(client, storage, "1.0.0");
    expect(persistedKeys(storage)).toHaveLength(1);
    client.removeQueries({ queryKey: ["machine", "m1"] });
    await save(client, storage, "1.0.0");
    expect(persistedKeys(storage)).toEqual([]);
  });

  it("puts the schema version into the buster", () => {
    expect(persistBuster("0.0.6")).toBe(`0.0.6+${PERSIST_SCHEMA_VERSION}`);
    expect(queryPersistOptions(new MemoryStorage(), "0.0.6").buster).toBe(persistBuster("0.0.6"));
  });
});
