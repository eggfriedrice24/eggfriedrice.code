import { describe, expect, it } from "vitest";

import { ConnectionError } from "@/lib/net/errors";
import { connectionView, repairReason, type ConnectionView } from "./connection-view";

const HOST = "100.101.102.103";
const PORT = 7419;

describe("connectionView", () => {
  const endpoint = `${HOST}:${PORT}`;
  const waiting = (error: ConnectionError): ConnectionView =>
    connectionView({ state: "waiting", retryAt: 5_000, error }, endpoint);
  const stopped = (error: ConnectionError): ConnectionView =>
    connectionView({ state: "stopped", error }, endpoint);

  it("tells Tailscale, a starting efrd and a closed connection apart", () => {
    expect(waiting(new ConnectionError("socket", "x"))).toEqual({
      kind: "unreachable",
      endpoint,
      retryAt: 5_000,
    });
    expect(waiting(new ConnectionError("timeout", "x")).kind).toBe("unreachable");
    expect(waiting(new ConnectionError("starting", "x")).kind).toBe("starting");
    expect(waiting(new ConnectionError("silent", "x")).kind).toBe("retrying");
    expect(connectionView({ state: "connecting" }, endpoint)).toEqual({
      kind: "connecting",
      endpoint,
    });
  });

  it("asks to pair again or to update when the client stopped", () => {
    expect(stopped(new ConnectionError("protocol_mismatch", "x"))).toEqual({
      kind: "update_needed",
    });
    expect(stopped(new ConnectionError("proof_refused", "x"))).toEqual({
      kind: "repair_needed",
      reason: "unknown_device",
    });
    expect(stopped(new ConnectionError("daemon_changed", "x"))).toEqual({
      kind: "repair_needed",
      reason: "other",
    });
  });

  it("reads re_pair as a changed node", () => {
    expect(repairReason(new ConnectionError("proof_refused", "x", { rePair: true }))).toBe(
      "node_changed",
    );
  });
});
