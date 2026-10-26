import { describe, expect, it } from "vitest";

import type { ConnectionView } from "@/features/machines/model/connection-view";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import {
  OFFLINE_GRACE_MS,
  offlineFrom,
  offlineWords,
  slotContent,
  slotLabel,
  type SlotInput,
} from "./header-slot";

const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const LAPTOP = "019a9b1c-3d00-7a10-8b20-0000000000d2";
/** 2026-10-26 14:00 UTC. */
const NOW = Date.parse("2026-10-26T14:00:00Z");
const SEEN = Date.parse("2026-10-26T13:58:00Z");

function connection(view: ConnectionView, seenAt: number | undefined = SEEN): MachineConnection {
  return { client: { state: "off" }, view, seenAt };
}

const ENDPOINT = "100.64.0.2:7419";

function input(overrides: Partial<SlotInput> = {}): SlotInput {
  return {
    elsewhere: { kind: "none" },
    machineName: (machine) => (machine === DESKTOP ? "desktop" : "laptop"),
    machine: {
      name: "laptop",
      connection: connection({ kind: "online", endpoint: ENDPOINT }),
      graceOver: true,
    },
    now: NOW,
    zone: 0,
    tailnetDown: false,
    ...overrides,
  };
}

describe("slotContent", () => {
  it("is empty while the machine is connected and nothing waits", () => {
    expect(slotContent(input())).toEqual({ kind: "none" });
  });

  it("shows one approval elsewhere with its machine and title", () => {
    const content = slotContent(
      input({
        elsewhere: {
          kind: "one",
          conversation: { machine: DESKTOP, id: "a", title: "Fix steer race" },
        },
      }),
    );
    expect(content).toEqual({
      kind: "approval",
      conversation: { machine: DESKTOP, id: "a" },
      machineName: "desktop",
      title: "Fix steer race",
    });
    expect(slotLabel(content)).toBe("Approval waiting, desktop, Fix steer race");
  });

  it("names a conversation without a title Untitled", () => {
    const content = slotContent(
      input({ elsewhere: { kind: "one", conversation: { machine: DESKTOP, id: "a" } } }),
    );
    expect(content).toMatchObject({ kind: "approval", title: "Untitled" });
  });

  it("counts two or more approvals", () => {
    const content = slotContent(
      input({
        elsewhere: {
          kind: "many",
          count: 2,
          conversations: [
            { machine: DESKTOP, id: "a" },
            { machine: LAPTOP, id: "b" },
          ],
        },
      }),
    );
    expect(content).toEqual({ kind: "approvals", count: 2, text: "2 approvals waiting" });
  });

  it("shows the offline line when the machine does not answer", () => {
    const content = slotContent(
      input({
        machine: {
          name: "laptop",
          connection: connection({ kind: "unreachable", endpoint: ENDPOINT, retryAt: NOW + 5000 }),
          graceOver: true,
        },
      }),
    );
    expect(content).toEqual({
      kind: "offline",
      machineName: "laptop",
      words: "offline · seen 13:58 · retrying",
    });
    expect(slotLabel(content)).toBe("laptop offline · seen 13:58 · retrying");
  });

  it("gives the slot to an approval over the offline line", () => {
    const content = slotContent(
      input({
        elsewhere: { kind: "one", conversation: { machine: DESKTOP, id: "a", title: "x" } },
        machine: {
          name: "laptop",
          connection: connection({ kind: "unreachable", endpoint: ENDPOINT }),
          graceOver: true,
        },
      }),
    );
    expect(content.kind).toBe("approval");
  });

  it("waits for the grace after the connection went down", () => {
    const content = slotContent(
      input({
        machine: {
          name: "laptop",
          connection: connection({ kind: "unreachable", endpoint: ENDPOINT }),
          graceOver: false,
        },
      }),
    );
    expect(content).toEqual({ kind: "none" });
  });

  it("is empty for a machine that is not paired any more", () => {
    expect(slotContent(input({ machine: undefined }))).toEqual({ kind: "none" });
  });
});

describe("offlineWords", () => {
  it("says nothing while the client connects or connects again", () => {
    expect(
      offlineWords(connection({ kind: "connecting", endpoint: ENDPOINT }), NOW, 0, false),
    ).toBeUndefined();
    expect(
      offlineWords(connection({ kind: "retrying", endpoint: ENDPOINT }), NOW, 0, false),
    ).toBeUndefined();
  });

  it("says that Tailscale is off when no machine answers", () => {
    expect(
      offlineWords(connection({ kind: "unreachable", endpoint: ENDPOINT }), NOW, 0, true),
    ).toBe("tailscale is not connected");
  });

  it("says that efrd starts, and that the machine needs a re-pair", () => {
    expect(
      offlineWords(
        connection({ kind: "starting", endpoint: ENDPOINT, retryAt: NOW + 1000 }),
        NOW,
        0,
        false,
      ),
    ).toBe("efrd is starting · retrying");
    expect(
      offlineWords(connection({ kind: "repair_needed", reason: "unknown_device" }), NOW, 0, false),
    ).toBe("re-pair needed");
  });

  it("uses the time zone of the phone for the seen time", () => {
    expect(
      offlineWords(connection({ kind: "unreachable", endpoint: ENDPOINT }), NOW, 60, false),
    ).toBe("offline · seen 14:58");
  });
});

describe("offlineFrom", () => {
  it("waits the grace after the connection went down", () => {
    expect(offlineFrom(connection({ kind: "unreachable", endpoint: ENDPOINT }))).toBe(
      SEEN + OFFLINE_GRACE_MS,
    );
  });

  it("shows the line at once for a machine that was not connected in this run", () => {
    const never: MachineConnection = {
      client: { state: "off" },
      view: { kind: "unreachable", endpoint: ENDPOINT },
      seenAt: undefined,
    };
    expect(offlineFrom(never)).toBeUndefined();
    expect(offlineFrom(undefined)).toBeUndefined();
  });
});
