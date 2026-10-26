import { describe, expect, it } from "vitest";

import type { MachineConnection } from "./machine-connection";
import { isTailnetDown, machineStateWords, seenText, zoneAt } from "./machine-status";

const ENDPOINT = "100.64.0.1:7419";
/** 2026-10-26 13:58:00 UTC. */
const SEEN = Date.UTC(2026, 9, 26, 13, 58);

function connection(view: MachineConnection["view"], seenAt?: number): MachineConnection {
  return { client: { state: "off" }, view, seenAt };
}

describe("seenText", () => {
  it("gives the time on the same day, in the phone's time zone", () => {
    expect(seenText(SEEN, SEEN + 60_000, 0)).toBe("seen 13:58");
    expect(seenText(SEEN, SEEN + 60_000, 120)).toBe("seen 15:58");
    expect(seenText(Date.UTC(2026, 9, 26, 9, 5), SEEN, 0)).toBe("seen 09:05");
  });

  it("gives yesterday and the days after that", () => {
    expect(seenText(SEEN, SEEN + 86_400_000, 0)).toBe("seen yesterday");
    expect(seenText(SEEN, SEEN + 2 * 86_400_000, 0)).toBe("seen 2 days ago");
    // 23:30 UTC is the next day at UTC+1.
    expect(seenText(Date.UTC(2026, 9, 25, 23, 30), SEEN, 60)).toBe("seen 00:30");
    expect(seenText(Date.UTC(2026, 9, 25, 23, 30), SEEN, 0)).toBe("seen yesterday");
  });
});

describe("machineStateWords", () => {
  const now = SEEN + 2 * 86_400_000;

  it("names each state as the menu and Settings show it", () => {
    const words = (view: MachineConnection["view"], seenAt?: number) =>
      machineStateWords(connection(view, seenAt), now, 0, false);
    expect(words({ kind: "online", endpoint: ENDPOINT })).toEqual({
      tone: "connected",
      subtitle: "Connected",
      line: "connected",
    });
    expect(words({ kind: "connecting", endpoint: ENDPOINT }).subtitle).toBe("Connecting…");
    expect(words({ kind: "starting", endpoint: ENDPOINT }).subtitle).toBe("efrd is starting");
    expect(words({ kind: "repair_needed", reason: "other" }).subtitle).toBe("Re-pair needed");
    expect(words({ kind: "update_needed" }).tone).toBe("update");
    expect(words({ kind: "unreachable", endpoint: ENDPOINT }, SEEN)).toEqual({
      tone: "offline",
      subtitle: "Offline · seen 2 days ago",
      line: "offline · seen 2 days ago",
    });
    expect(words({ kind: "retrying", endpoint: ENDPOINT }, now - 60_000).line).toBe(
      "offline · seen 13:57",
    );
    expect(words({ kind: "unreachable", endpoint: ENDPOINT }).line).toBe("offline");
  });

  it("names Tailscale when no machine answers", () => {
    const view = { kind: "unreachable", endpoint: ENDPOINT } as const;
    expect(machineStateWords(connection(view, SEEN), now, 0, true)).toEqual({
      tone: "offline",
      subtitle: "Tailscale is not connected",
      line: "tailscale is not connected",
    });
  });
});

describe("isTailnetDown", () => {
  const unreachable = connection({ kind: "unreachable", endpoint: ENDPOINT });
  const online = connection({ kind: "online", endpoint: ENDPOINT });
  const repair = connection({ kind: "repair_needed", reason: "other" });

  it("is true only when every machine with a client is unreachable", () => {
    expect(isTailnetDown([])).toBe(false);
    expect(isTailnetDown([unreachable])).toBe(true);
    expect(isTailnetDown([unreachable, repair])).toBe(true);
    expect(isTailnetDown([unreachable, online])).toBe(false);
    expect(isTailnetDown([repair])).toBe(false);
  });
});

describe("zoneAt", () => {
  it("gives minutes east of UTC, the sign that seenText uses", () => {
    const seen = Date.UTC(2026, 9, 26, 13, 58);
    expect(zoneAt(seen)).toBe(-new Date(seen).getTimezoneOffset());
  });
});
