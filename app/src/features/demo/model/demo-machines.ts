// The demo machines of the dev builds (plan sections 0 and 4, W4-DEMO-MEASURE): three
// machines that the demo server (demo-server.ts) plays, so every screen and state of the
// design can show on the iPhone without a computer.
//
// Each demo machine has a pairing code like the one that `efr device add` shows. The
// gallery pairs the phone with it through the real pairing flow and the real runtime;
// only the socket goes to the demo server instead of the network. The address of each
// machine is a Tailscale address with port 9 (the discard port), where no efrd listens: a
// build without the dev tools that still has a demo record cannot reach anything there.
// Pure.

import type { MachineId } from "@/features/machines/model/machine-ref";

/** The key of a demo machine. */
export type DemoMachineKey = "desktop" | "laptop" | "homelab";

/** One demo machine. */
export interface DemoMachine {
  readonly key: DemoMachineKey;
  readonly daemonId: MachineId;
  /** The short name, as the app shows it (the first label of the MagicDNS name). */
  readonly name: string;
  readonly host: string;
  readonly port: number;
  readonly fqdn: string;
  /** The pairing token of its code. */
  readonly token: string;
}

/** The port of every demo machine. */
export const DEMO_PORT = 9;

/** The demo machines, in the order that the gallery pairs them. */
export const DEMO_MACHINES: readonly DemoMachine[] = [
  {
    key: "desktop",
    daemonId: "0199d3e0-de30-7000-8000-00000000d001",
    name: "desktop",
    host: "100.127.250.1",
    port: DEMO_PORT,
    fqdn: "desktop.demo.invalid",
    token: "ZGVtby1kZXNrdG9wLXRva2VuLTAx",
  },
  {
    key: "laptop",
    daemonId: "0199d3e0-de30-7000-8000-00000000d002",
    name: "laptop",
    host: "100.127.250.2",
    port: DEMO_PORT,
    fqdn: "laptop.demo.invalid",
    token: "ZGVtby1sYXB0b3AtdG9rZW4tMDI",
  },
  {
    key: "homelab",
    daemonId: "0199d3e0-de30-7000-8000-00000000d003",
    name: "homelab",
    host: "100.127.250.3",
    port: DEMO_PORT,
    fqdn: "homelab.demo.invalid",
    token: "ZGVtby1ob21lbGFiLXRva2VuLTAz",
  },
];

/** The demo machine `key`. */
export function demoMachine(key: DemoMachineKey): DemoMachine {
  const machine = DEMO_MACHINES.find((one) => one.key === key);
  if (machine === undefined) {
    throw new Error(`no demo machine ${key}`);
  }
  return machine;
}

/** The URL of the listener of `machine`, as the app makes it from the record. */
export function demoUrl(machine: DemoMachine): string {
  return `ws://${machine.host}:${machine.port}/`;
}

/** The demo machine at `url`, or undefined for any other address. */
export function demoMachineAt(url: string): DemoMachine | undefined {
  return DEMO_MACHINES.find((machine) => demoUrl(machine) === url);
}

/** True when `id` is the daemon id of a demo machine. */
export function isDemoMachine(id: string): boolean {
  return DEMO_MACHINES.some((machine) => machine.daemonId === id);
}

/** The pairing code of `machine`, as its QR code would carry it. */
export function demoPairingCode(machine: DemoMachine): string {
  return `efr://pair?v=1&d=${machine.daemonId}&h=${machine.host}&p=${machine.port}&f=${machine.fqdn}&t=${machine.token}`;
}
