import { describe, expect, it } from "vitest";

import type { ConnectionView } from "../model/connection-view";
import { machineOf } from "../model/machine";
import type { MachineConnection } from "../model/machine-connection";
import { DESKTOP, LAPTOP, machineRecord } from "../model/testing";
import { machineButtonLabel, machineMenuItems, machineSymbol } from "./machine-menu-items";

const NOW = Date.parse("2026-10-26T14:00:00Z");
const ENDPOINT = "100.64.0.1:7419";

function connection(view: ConnectionView, seenAt?: number): MachineConnection {
  return { client: { state: "off" }, view, seenAt };
}

const MACHINES = [
  machineOf(machineRecord(DESKTOP), false),
  machineOf(machineRecord(LAPTOP), false),
];

describe("machineMenuItems", () => {
  it("lists each machine with its state and checks the machine of the root", () => {
    const items = machineMenuItems({
      machines: MACHINES,
      connections: new Map([
        [DESKTOP, connection({ kind: "online", endpoint: ENDPOINT })],
        [LAPTOP, connection({ kind: "connecting", endpoint: ENDPOINT })],
      ]),
      current: DESKTOP,
      now: NOW,
      zone: 0,
    });
    expect(items).toEqual([
      {
        id: DESKTOP,
        title: "desktop",
        subtitle: "Connected",
        symbol: "desktopcomputer",
        current: true,
      },
      {
        id: LAPTOP,
        title: "laptop",
        subtitle: "Connecting…",
        symbol: "laptopcomputer",
        current: false,
      },
    ]);
  });

  it("says when an offline machine was seen", () => {
    const [desktop] = machineMenuItems({
      machines: MACHINES,
      connections: new Map([
        [DESKTOP, connection({ kind: "retrying", endpoint: ENDPOINT }, NOW - 2 * 86_400_000)],
        [LAPTOP, connection({ kind: "online", endpoint: ENDPOINT })],
      ]),
      current: LAPTOP,
      now: NOW,
      zone: 0,
    });
    expect(desktop?.subtitle).toBe("Offline · seen 2 days ago");
  });

  it("says that Tailscale is off when no machine answers", () => {
    const items = machineMenuItems({
      machines: MACHINES,
      connections: new Map([
        [DESKTOP, connection({ kind: "unreachable", endpoint: ENDPOINT })],
        [LAPTOP, connection({ kind: "unreachable", endpoint: ENDPOINT })],
      ]),
      current: DESKTOP,
      now: NOW,
      zone: 0,
    });
    expect(items.map((item) => item.subtitle)).toEqual([
      "Tailscale is not connected",
      "Tailscale is not connected",
    ]);
  });
});

describe("the machine symbols and label", () => {
  it("picks a symbol from the name", () => {
    expect(machineSymbol("desktop")).toBe("desktopcomputer");
    expect(machineSymbol("work-macbook")).toBe("laptopcomputer");
    expect(machineSymbol("homelab")).toBe("server.rack");
    expect(machineSymbol("nas")).toBe("server.rack");
  });

  it("labels the button with the machine", () => {
    expect(machineButtonLabel("desktop")).toBe("Machine: desktop");
  });
});
