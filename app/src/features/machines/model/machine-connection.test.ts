import { describe, expect, it } from "vitest";

import { ConnectionError } from "@/lib/net/errors";
import { remoteHello } from "@/lib/net/testing";
import {
  initialConnection,
  keylessConnection,
  nextConnection,
  statusOf,
} from "./machine-connection";

const ENDPOINT = "100.64.0.1:7419";
const READY = {
  state: "ready",
  hello: remoteHello({ version: "0.0.5" }),
  scopes: ["read", "operate"],
} as const;

describe("the connection of a machine", () => {
  it("keeps the time of the last connection", () => {
    const start = initialConnection({ state: "off" }, ENDPOINT);
    expect(start).toEqual({
      client: { state: "off" },
      view: { kind: "connecting", endpoint: ENDPOINT },
      seenAt: undefined,
    });
    const connecting = nextConnection(start, { state: "connecting" }, ENDPOINT, 10);
    expect(connecting.seenAt).toBeUndefined();
    const up = nextConnection(connecting, READY, ENDPOINT, 20);
    expect(up.seenAt).toBe(20);
    const waiting = {
      state: "waiting",
      retryAt: 1_030,
      error: new ConnectionError("socket", "down"),
    } as const;
    const down = nextConnection(up, waiting, ENDPOINT, 30);
    expect(down).toMatchObject({ view: { kind: "unreachable", retryAt: 1_030 }, seenAt: 30 });
    expect(nextConnection(down, { state: "connecting" }, ENDPOINT, 40).seenAt).toBe(30);
  });

  it("gives the status of efrd only while ready", () => {
    const up = nextConnection(initialConnection({ state: "off" }, ENDPOINT), READY, ENDPOINT, 20);
    expect(statusOf(up)).toEqual({
      version: "0.0.5",
      protocol: 1,
      scopes: ["read", "operate"],
      seenAt: 20,
    });
    expect(statusOf(initialConnection({ state: "connecting" }, ENDPOINT))).toBeUndefined();
    expect(statusOf(keylessConnection())).toBeUndefined();
    expect(keylessConnection().view).toEqual({ kind: "repair_needed", reason: "other" });
  });
});
