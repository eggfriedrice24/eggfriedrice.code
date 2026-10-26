import { describe, expect, it } from "vitest";

import { connectionNotice, repairText } from "./connection-notice";

const ENDPOINT = "100.101.102.103:7419";

describe("connectionNotice", () => {
  it("says nothing while online", () => {
    expect(connectionNotice({ kind: "online", endpoint: ENDPOINT }, 0)).toBeUndefined();
  });

  it("names Tailscale first when the computer does not answer", () => {
    const notice = connectionNotice({ kind: "unreachable", endpoint: ENDPOINT, retryAt: 4_100 }, 0);
    expect(notice).toMatchObject({
      title: "Tailscale is not connected",
      retryIn: 5,
      canRetry: true,
      tone: "warning",
    });
    expect(notice?.body).toContain(ENDPOINT);
  });

  it("tells a starting daemon and a closed connection apart, with the time to the next try", () => {
    expect(
      connectionNotice({ kind: "starting", endpoint: ENDPOINT, retryAt: 2_000 }, 1_000),
    ).toMatchObject({
      title: "efrd is starting",
      retryIn: 1,
    });
    expect(connectionNotice({ kind: "retrying", endpoint: ENDPOINT }, 0)).toMatchObject({
      title: "The connection stopped",
      canRetry: true,
    });
    expect(connectionNotice({ kind: "retrying", endpoint: ENDPOINT }, 0)?.retryIn).toBeUndefined();
  });

  it("asks to pair again, and says why", () => {
    expect(connectionNotice({ kind: "repair_needed", reason: "node_changed" }, 0)).toMatchObject({
      title: "Pair this phone again",
      canRetry: false,
      tone: "error",
    });
    expect(repairText("node_changed")).toContain("new device");
    expect(repairText("unknown_device")).toContain("removed the phone");
    expect(repairText("other")).toContain("efr device add");
  });

  it("names the computer while the app reads its pairing, and asks for an update", () => {
    expect(connectionNotice({ kind: "connecting", endpoint: "" }, 0)?.body).toBe(
      "efr connects to your computer.",
    );
    expect(connectionNotice({ kind: "update_needed" }, 0)).toMatchObject({
      title: "Update efr",
      canRetry: false,
    });
  });
});
