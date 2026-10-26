// Every board of the design canvas that the pairing feature builds, and the undrawn states
// of plan 5.2 item 8.19, each as a phase of the pairing session with sample data. The dev
// gallery (W4) opens each one in the real pairing modal (screens/pairing-board-screen.tsx);
// a test checks that each canvas board has its state. Pure.

import { machineOf } from "@/features/machines/model/machine";
import { PROTOCOL_VERSION } from "@/lib/protocol/generated";
import type { PairingPhase } from "./pairing-session";
import { pairingTitle } from "./pairing-view";

/** One board: its name on the canvas (or a name for an undrawn state) and its state. */
export interface PairingBoard {
  /** Such as "P-Errors · code expired". */
  readonly name: string;
  /** True for a board that the canvas draws; false for an undrawn state (item 8.19). */
  readonly drawn: boolean;
  /** The title of the nav row. */
  readonly title: string;
  readonly phase: PairingPhase;
  /** The efrd line of the success board. */
  readonly daemon?: { readonly version: string; readonly protocol: number };
}

const DESKTOP = machineOf(
  {
    deviceId: "019a9b1c-3d00-7a10-8b20-0000000000e1",
    daemonId: "019a9b1c-3d00-7a10-8b20-0000000000d1",
    host: "100.64.0.1",
    port: 7419,
    fqdn: "desktop.tail3f2a.ts.net",
    name: "iPhone 17 Pro",
    keyAlias: "device-desktop",
    scopes: ["read", "operate", "approve"],
    pairedAt: "2026-10-26T09:00:00.000Z",
  },
  false,
);

const LAPTOP = machineOf(
  {
    deviceId: "019a9b1c-3d00-7a10-8b20-0000000000e2",
    daemonId: "019a9b1c-3d00-7a10-8b20-0000000000d2",
    host: "100.64.0.2",
    port: 7419,
    fqdn: "laptop.tail3f2a.ts.net",
    name: "iPhone 17 Pro",
    keyAlias: "device-laptop",
    scopes: ["read", "operate", "approve"],
    pairedAt: "2026-10-26T09:00:00.000Z",
  },
  false,
);

const NEW = pairingTitle(undefined);

/** The boards of pairing, in the order of the canvas. */
export const PAIRING_BOARDS: readonly PairingBoard[] = [
  { name: "P-Intro", drawn: true, title: NEW, phase: { kind: "intro" } },
  { name: "P-Scan", drawn: true, title: NEW, phase: { kind: "scanning" } },
  {
    name: "P-Wait",
    drawn: true,
    title: NEW,
    phase: {
      kind: "confirm",
      machine: "desktop",
      device: "iPhone 17 Pro",
      fingerprint: "SHA256:7f3a91c24d0eb6a1",
      saving: false,
    },
  },
  {
    name: "P-Success",
    drawn: true,
    title: NEW,
    phase: { kind: "paired", machine: DESKTOP, repaired: false },
    daemon: { version: "0.0.5", protocol: PROTOCOL_VERSION },
  },
  {
    name: "P-Errors · code expired",
    drawn: true,
    title: NEW,
    phase: { kind: "failed", problem: { kind: "expired" } },
  },
  {
    name: "P-Errors · wrong machine",
    drawn: true,
    title: pairingTitle("laptop"),
    phase: {
      kind: "failed",
      problem: { kind: "other_machine", code: "homelab", repairing: "laptop" },
    },
  },
  {
    name: "P-Errors · tailscale not connected",
    drawn: true,
    title: NEW,
    phase: { kind: "failed", problem: { kind: "tailnet", machine: "desktop" } },
  },
  {
    name: "P-Errors · refused",
    drawn: true,
    title: NEW,
    phase: { kind: "failed", problem: { kind: "refused", machine: "desktop" } },
  },
  {
    name: "P-Errors · efrd too old",
    drawn: true,
    title: NEW,
    phase: {
      kind: "failed",
      problem: {
        kind: "protocol",
        direction: "efrd_old",
        machine: "nas",
        protocol: PROTOCOL_VERSION - 1,
        version: "0.0.3",
      },
    },
  },
  {
    name: "Pairing · connecting",
    drawn: false,
    title: NEW,
    phase: { kind: "connecting", machine: "desktop" },
  },
  {
    name: "Pairing · saving after the yes",
    drawn: false,
    title: NEW,
    phase: {
      kind: "confirm",
      machine: "desktop",
      device: "iPhone 17 Pro",
      fingerprint: "SHA256:7f3a91c24d0eb6a1",
      saving: true,
    },
  },
  {
    name: "Pairing · re-paired",
    drawn: false,
    title: pairingTitle("laptop"),
    phase: { kind: "paired", machine: LAPTOP, repaired: true },
    daemon: { version: "0.0.5", protocol: PROTOCOL_VERSION },
  },
  {
    name: "Pairing · camera denied",
    drawn: false,
    title: NEW,
    phase: { kind: "failed", problem: { kind: "camera" } },
  },
  {
    name: "Pairing · no scanner",
    drawn: false,
    title: NEW,
    phase: { kind: "failed", problem: { kind: "no_scanner" } },
  },
  {
    name: "Pairing · app too old",
    drawn: false,
    title: NEW,
    phase: {
      kind: "failed",
      problem: {
        kind: "protocol",
        direction: "app_old",
        machine: "nas",
        protocol: PROTOCOL_VERSION + 1,
        version: "0.1.0",
      },
    },
  },
  {
    name: "Pairing · bad code",
    drawn: false,
    title: NEW,
    phase: { kind: "failed", problem: { kind: "bad_code" } },
  },
  {
    name: "Pairing · another machine answered",
    drawn: false,
    title: NEW,
    phase: { kind: "failed", problem: { kind: "wrong_daemon", machine: "desktop" } },
  },
  {
    name: "Pairing · no device key",
    drawn: false,
    title: NEW,
    phase: {
      kind: "failed",
      problem: {
        kind: "key",
        message: "This build of the app has no device key, so it cannot pair.",
      },
    },
  },
];

/** The board named `name`, or undefined. */
export function pairingBoard(name: string): PairingBoard | undefined {
  return PAIRING_BOARDS.find((board) => board.name === name);
}
