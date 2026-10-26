import { MutationObserver, QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";

import { FakeClock, FakeSockets, completeHandshake, remoteHello } from "@/lib/net/testing";
import { PairingError } from "@/features/pairing/model/pair";
import {
  forgetMachineMutationOptions,
  invalidateOnReady,
  machineKeys,
  machineStatusQueryOptions,
  pairMachineMutationOptions,
} from "./machine-queries";
import { createRuntime } from "./model/runtime";
import { DESKTOP, FakeStreams, LAPTOP, machineRecord, pairedStores } from "./model/testing";

async function setup() {
  const { keys, devices } = await pairedStores([machineRecord(DESKTOP), machineRecord(LAPTOP)]);
  const sockets = new FakeSockets();
  const runtime = createRuntime({
    pairing: {
      sockets: sockets.factory,
      clock: new FakeClock(),
      keys,
      devices,
      sha256: () => Promise.resolve(new Uint8Array(32)),
      newKeyAlias: () => "device-new",
      now: () => new Date("2026-10-26T12:00:00Z"),
      client: "efr-app test",
    },
    random: () => 0,
    streams: new FakeStreams().factory,
    schedule: () => () => {},
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  await runtime.load();
  const desktop = sockets.made.find((socket) => socket.url === "ws://100.64.0.1:7419/");
  if (desktop === undefined) {
    throw new Error("the desktop did not connect");
  }
  return { runtime, client, sockets, desktop, devices };
}

describe("machineKeys", () => {
  it("starts every key of a machine with the machine", () => {
    expect(machineKeys.all).toEqual(["machine"]);
    expect(machineKeys.machine(DESKTOP)).toEqual(["machine", DESKTOP]);
    expect(machineKeys.status(DESKTOP).slice(0, 2)).toEqual(machineKeys.machine(DESKTOP));
  });
});

describe("the status query", () => {
  it("fails while offline and gives efrd's version once connected", async () => {
    const { runtime, client, desktop } = await setup();
    const options = machineStatusQueryOptions(runtime, DESKTOP);
    expect(options.meta).toEqual({ persist: true });
    await expect(client.fetchQuery(options)).rejects.toMatchObject({ kind: "offline" });
    await completeHandshake(desktop, remoteHello({ daemon_id: DESKTOP, version: "0.0.5" }));
    await expect(client.fetchQuery(options)).resolves.toMatchObject({
      version: "0.0.5",
      protocol: 1,
    });
  });
});

describe("invalidateOnReady", () => {
  it("refreshes the queries of a machine when its connection comes up", async () => {
    const { runtime, client, desktop } = await setup();
    const stop = invalidateOnReady(runtime, client);
    client.setQueryData(["machine", DESKTOP, "conversations", "recent"], []);
    client.setQueryData(["machine", LAPTOP, "conversations", "recent"], []);
    await completeHandshake(desktop, remoteHello({ daemon_id: DESKTOP }));
    const state = (machine: string) =>
      client.getQueryState(["machine", machine, "conversations", "recent"])?.isInvalidated;
    expect(state(DESKTOP)).toBe(true);
    expect(state(LAPTOP)).toBe(false);
    stop();
  });
});

describe("the forget mutation", () => {
  it("forgets the machine and removes its queries only", async () => {
    const { runtime, client, devices } = await setup();
    client.setQueryData(machineKeys.status(DESKTOP), { version: "0.0.5" });
    client.setQueryData(machineKeys.status(LAPTOP), { version: "0.0.5" });
    await new MutationObserver(client, forgetMachineMutationOptions(runtime)).mutate(DESKTOP);
    expect(client.getQueryData(machineKeys.status(DESKTOP))).toBeUndefined();
    expect(client.getQueryData(machineKeys.status(LAPTOP))).toEqual({ version: "0.0.5" });
    expect(devices.devices.map((device) => device.daemonId)).toEqual([LAPTOP]);
    expect(runtime.machine(DESKTOP)).toBeUndefined();
  });
});

describe("the pair mutation", () => {
  it("rejects with the failure of the flow", async () => {
    const { runtime, client } = await setup();
    const observer = new MutationObserver(client, pairMachineMutationOptions(runtime));
    const result = observer.mutate({ code: "not a code", name: "iPhone" });
    await expect(result).rejects.toBeInstanceOf(PairingError);
    expect(observer.getCurrentResult().isError).toBe(true);
  });
});
