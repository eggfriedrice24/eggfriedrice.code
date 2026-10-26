// The items of the machine menu of the blank root (design-spec section 3.13, board
// R-MachineMenu): one item per paired machine with its state as the subtitle, a check on
// the machine of the root, then "Pair a new machine" in its own section. An offline
// machine stays enabled and is not dimmed (design item 8.12); its subtitle says so. Pure.

import type { Machine } from "../model/machine";
import type { MachineConnection } from "../model/machine-connection";
import type { MachineId } from "../model/machine-ref";
import { isTailnetDown, machineStateWords, type ZoneOffset } from "../model/machine-status";

/** The SF Symbol of a machine. */
export type MachineSymbol = "desktopcomputer" | "laptopcomputer" | "server.rack";

/** One machine of the menu. */
export interface MachineMenuItem {
  readonly id: MachineId;
  readonly title: string;
  /** "Connected", "Connecting…", "Offline · seen 2 days ago" and the like. */
  readonly subtitle: string;
  readonly symbol: MachineSymbol;
  /** True for the machine of the blank root: the item has the check. */
  readonly current: boolean;
}

/** The title of the menu. */
export const MACHINE_MENU_TITLE = "Machine";

/** The item that opens Pairing. */
export const PAIR_ITEM = { title: "Pair a new machine", symbol: "qrcode.viewfinder" } as const;

const LAPTOP = /laptop|book/i;
const SERVER = /server|nas|lab|rack|vps/i;

/**
 * The symbol of a machine, from its name: the phone knows no more about the computer.
 * "laptop" or "macbook" is a laptop, "homelab" or "nas" a server, else a desktop.
 */
export function machineSymbol(name: string): MachineSymbol {
  if (LAPTOP.test(name)) {
    return "laptopcomputer";
  }
  if (SERVER.test(name)) {
    return "server.rack";
  }
  return "desktopcomputer";
}

/** What the menu reads. */
export interface MachineMenuInput {
  readonly machines: readonly Machine[];
  readonly connections: ReadonlyMap<MachineId, MachineConnection>;
  /** The machine of the blank root. */
  readonly current: MachineId | undefined;
  /** The time now, ms since the epoch, for "seen 13:58". */
  readonly now: number;
  readonly zone: ZoneOffset;
}

/** The machine items of the menu, in the order of the registry. */
export function machineMenuItems(input: MachineMenuInput): MachineMenuItem[] {
  const tailnetDown = isTailnetDown([...input.connections.values()]);
  return input.machines.map((machine) => {
    const connection = input.connections.get(machine.id);
    const subtitle =
      connection === undefined
        ? "Connecting…"
        : machineStateWords(connection, input.now, input.zone, tailnetDown).subtitle;
    return {
      id: machine.id,
      title: machine.name,
      subtitle,
      symbol: machineSymbol(machine.name),
      current: machine.id === input.current,
    };
  });
}

/** The VoiceOver label of the machine button: "Machine: desktop". */
export function machineButtonLabel(name: string): string {
  return `${MACHINE_MENU_TITLE}: ${name}`;
}
