import { describe, expect, it } from "vitest";

import { conversationKey } from "./machine-ref";
import { endpointLabel, endpointOf, machineOf } from "./machine";
import {
  findMachine,
  loadedRegistry,
  LOADING_REGISTRY,
  withMachine,
  withoutMachine,
} from "./machine-registry";
import { pathChanged } from "./network-path";
import { DESKTOP, LAPTOP, machineRecord } from "./testing";

const desktop = machineOf(machineRecord(DESKTOP), false);
const laptop = machineOf(machineRecord(LAPTOP), false);

describe("the registry", () => {
  it("adds a machine at the end and replaces one in its place", () => {
    const state = withMachine(withMachine(loadedRegistry([]), desktop), laptop);
    expect(state.machines.map((machine) => machine.name)).toEqual(["desktop", "laptop"]);
    const repaired = machineOf(machineRecord(DESKTOP, { keyAlias: "device-2" }), true);
    const next = withMachine(state, repaired);
    expect(next.machines).toEqual([repaired, laptop]);
    expect(findMachine(next, DESKTOP)?.keyMissing).toBe(true);
    expect(withoutMachine(next, DESKTOP).machines).toEqual([laptop]);
    expect(withMachine(LOADING_REGISTRY, desktop).loaded).toBe(false);
  });
});

describe("a machine", () => {
  it("has a short name, an endpoint for its client and a label of its address", () => {
    expect(desktop).toMatchObject({ id: DESKTOP, name: "desktop", keyMissing: false });
    expect(endpointOf(machineRecord(DESKTOP))).toEqual({
      daemonId: DESKTOP,
      host: "100.64.0.1",
      port: 7419,
      fqdn: "desktop.tail3f2a.ts.net",
      deviceId: machineRecord(DESKTOP).deviceId,
    });
    const { fqdn: _, ...bare } = machineRecord(LAPTOP);
    expect(endpointOf(bare)).not.toHaveProperty("fqdn");
    expect(machineOf(bare, false).name).toBe("100.64.0.2");
    expect(endpointLabel(bare)).toBe("100.64.0.2:7419");
    expect(conversationKey({ machine: DESKTOP, id: "c1" })).toBe(`${DESKTOP}:c1`);
  });
});

describe("pathChanged", () => {
  it("is true only for a new path with a network", () => {
    expect(pathChanged(undefined, { type: "wifi", connected: true })).toBe(false);
    expect(pathChanged({ type: "wifi", connected: true }, { type: "wifi", connected: true })).toBe(
      false,
    );
    expect(
      pathChanged({ type: "wifi", connected: true }, { type: "cellular", connected: true }),
    ).toBe(true);
    expect(pathChanged({ type: "wifi", connected: true }, { type: "none", connected: false })).toBe(
      false,
    );
    expect(pathChanged({ type: "wifi", connected: false }, { type: "wifi", connected: true })).toBe(
      true,
    );
  });
});
