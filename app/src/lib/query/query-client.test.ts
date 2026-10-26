import { QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConnectionError, ServerError } from "@/lib/net/errors";
import {
  MAX_RETRY_DELAY_MS,
  STALE_TIME_MS,
  createQueryClient,
  queryRetryDelay,
  shouldRetryQuery,
} from "./query-client";
import { FakeCaller } from "./testing";

const busy = new ServerError({ code: "busy", message: "busy", data: { retry_after_ms: 2500 } });
const notFound = new ServerError({ code: "not_found", message: "no such conversation" });

describe("shouldRetryQuery", () => {
  it("retries an error that can pass, at most twice", () => {
    const lost = new ConnectionError("socket", "the socket closed");
    expect(shouldRetryQuery(0, lost)).toBe(true);
    expect(shouldRetryQuery(1, lost)).toBe(true);
    expect(shouldRetryQuery(2, lost)).toBe(false);
    expect(shouldRetryQuery(0, busy)).toBe(true);
    expect(shouldRetryQuery(0, new ServerError({ code: "internal", message: "x" }))).toBe(true);
  });

  it("never retries a machine that is not connected", () => {
    expect(shouldRetryQuery(0, new ConnectionError("offline", "not connected"))).toBe(false);
  });

  it("never retries an error that needs the user or a refusal of efrd", () => {
    expect(shouldRetryQuery(0, new ConnectionError("proof_refused", "refused"))).toBe(false);
    expect(shouldRetryQuery(0, new ConnectionError("protocol_mismatch", "old"))).toBe(false);
    expect(shouldRetryQuery(0, new ConnectionError("closed", "closed"))).toBe(false);
    expect(shouldRetryQuery(0, notFound)).toBe(false);
    expect(shouldRetryQuery(0, new ServerError({ code: "forbidden", message: "x" }))).toBe(false);
  });

  it("never retries an error that is not of the connection, such as a bug", () => {
    expect(shouldRetryQuery(0, new TypeError("x is undefined"))).toBe(false);
    expect(shouldRetryQuery(0, "a string")).toBe(false);
  });
});

describe("queryRetryDelay", () => {
  it("waits as long as efrd asked, at most the cap", () => {
    expect(queryRetryDelay(0, busy)).toBe(2500);
    const long = new ServerError({ code: "busy", message: "x", data: { retry_after_secs: 600 } });
    expect(queryRetryDelay(0, long)).toBe(MAX_RETRY_DELAY_MS);
    const starting = new ConnectionError("starting", "starting", { retryAfterMs: 400 });
    expect(queryRetryDelay(3, starting)).toBe(400);
  });

  it("doubles from 1 s up to the cap otherwise", () => {
    const lost = new ConnectionError("socket", "closed");
    expect([0, 1, 2, 3, 4, 5, 6].map((count) => queryRetryDelay(count, lost))).toEqual([
      1000, 2000, 4000, 8000, 16_000, 30_000, 30_000,
    ]);
  });
});

describe("createQueryClient", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("sets the defaults of the app", () => {
    const client = createQueryClient();
    const defaults = client.getDefaultOptions();
    expect(defaults.queries?.networkMode).toBe("always");
    expect(defaults.queries?.staleTime).toBe(STALE_TIME_MS);
    expect(defaults.mutations?.retry).toBe(false);
    expect(defaults.mutations?.networkMode).toBe("always");
  });

  it("fails at once, without a retry, when the machine is not connected", async () => {
    const client = createQueryClient();
    const caller = new FakeCaller();
    await expect(
      client.fetchQuery({
        queryKey: ["machine", "m1", "probe"],
        queryFn: () => caller.call("m1", "conversations.recent", {}),
      }),
    ).rejects.toMatchObject({ kind: "offline" });
    expect(caller.calls).toHaveLength(1);
  });

  it("tries a busy efrd again after the wait it asked for", async () => {
    const client = createQueryClient();
    let answers = 0;
    const caller = new FakeCaller({
      "conversations.recent": () => {
        answers += 1;
        if (answers === 1) {
          throw busy;
        }
        return { conversations: [] };
      },
    });
    const observer = new QueryObserver(client, {
      queryKey: ["machine", "m1", "probe"],
      queryFn: () => caller.call("m1", "conversations.recent", {}),
    });
    const stop = observer.subscribe(() => undefined);
    await vi.advanceTimersByTimeAsync(0);
    expect(caller.calls).toHaveLength(1);
    expect(observer.getCurrentResult().failureCount).toBe(1);
    await vi.advanceTimersByTimeAsync(2499);
    expect(caller.calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(caller.calls).toHaveLength(2);
    expect(observer.getCurrentResult().data).toEqual({ conversations: [] });
    stop();
  });

  it("never runs a mutation twice on its own", async () => {
    const client = createQueryClient();
    const caller = new FakeCaller({
      "turn.steer": () => {
        throw new ConnectionError("socket", "closed");
      },
    });
    const mutation = client.getMutationCache().build(client, {
      mutationFn: () =>
        caller.call("m1", "turn.steer", { command_id: "c", conversation_id: "x", text: "t" }),
    });
    await expect(mutation.execute(undefined)).rejects.toMatchObject({ kind: "socket" });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(caller.calls).toHaveLength(1);
  });
});
