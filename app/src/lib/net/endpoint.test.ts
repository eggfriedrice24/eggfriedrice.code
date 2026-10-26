import { describe, expect, it } from "vitest";

import { isIpv4, socketUrl } from "./endpoint";
import { asksToRepair, retryAfterMs } from "./errors";

describe("socketUrl", () => {
  it("gives the ws URL of an IPv4 address and port", () => {
    expect(socketUrl({ host: "100.64.0.1", port: 7419 })).toBe("ws://100.64.0.1:7419/");
    expect(socketUrl({ host: "100.127.255.254", port: 1 })).toBe("ws://100.127.255.254:1/");
  });

  it("refuses names, IPv6, bad addresses and bad ports", () => {
    for (const [host, port] of [
      ["desk.tail1234.ts.net", 7419],
      ["fd7a:115c:a1e0::1", 7419],
      ["100.64.0", 7419],
      ["100.64.0.256", 7419],
      ["100.064.0.1", 7419],
      ["100.64.0.1", 0],
      ["100.64.0.1", 65_536],
      ["100.64.0.1", 7419.5],
    ] as const) {
      expect({ host, port, url: socketUrl({ host, port }) }).toEqual({
        host,
        port,
        url: undefined,
      });
    }
    expect(isIpv4("0.0.0.0")).toBe(true);
  });
});

describe("the error data", () => {
  it("reads the retry wait in milliseconds or seconds", () => {
    expect(retryAfterMs({ code: "busy", message: "", data: { retry_after_ms: 1500 } })).toBe(1500);
    expect(retryAfterMs({ code: "busy", message: "", data: { retry_after_secs: 2 } })).toBe(2000);
    expect(retryAfterMs({ code: "busy", message: "", data: { retry_after_ms: -1 } })).toBe(
      undefined,
    );
    expect(retryAfterMs({ code: "busy", message: "" })).toBe(undefined);
  });

  it("reads re_pair as efrd sends it, and only true", () => {
    const device_id = "019a9b1c-3d00-7a10-8b20-000000000006";
    expect(
      asksToRepair({ code: "unauthorized", message: "", data: { re_pair: true, device_id } }),
    ).toBe(true);
    expect(asksToRepair({ code: "unauthorized", message: "", data: { re_pair: "yes" } })).toBe(
      false,
    );
    expect(asksToRepair({ code: "unauthorized", message: "" })).toBe(false);
  });
});
