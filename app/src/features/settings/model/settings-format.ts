// The words of the Settings screen (design 3.16): the three lines of a machine row, the
// row menu, the Forget sheet and the version. Pure, for the tests.
//
// A machine row is all mono:
// - the name, in text_bright, or in text_muted while the machine is offline;
// - the state, such as "✓ connected · desktop.tail3f2a.ts.net", "… connecting · laptop…"
//   or "offline · seen 2 days ago" (an offline machine shows no node name);
// - the efrd version and protocol of the last connection, "efrd 0.0.5 · protocol 1". The
//   phone keeps them, so the line stays while the machine is offline.

import type { ActionSheet } from "@/components/ui/action-sheet";
import type { MenuEntry } from "@/components/ui/menu-actions";
import type { TextTone } from "@/components/ui/text-classes";
import type { Machine } from "@/features/machines/model/machine";
import type {
  MachineConnection,
  MachineStatus,
} from "@/features/machines/model/machine-connection";
import {
  machineStateWords,
  type MachineTone,
  type ZoneOffset,
} from "@/features/machines/model/machine-status";

/** The text of a machine row. */
export interface MachineRowText {
  readonly name: string;
  /** True while the machine is offline: the name is text_muted. */
  readonly dim: boolean;
  /** The state word with its glyph, such as "✓ connected". */
  readonly state: string;
  /** The colour of the state word. */
  readonly stateTone: TextTone;
  /** The node name after the state word, such as "desktop.tail3f2a.ts.net". */
  readonly node: string | undefined;
  /** "efrd 0.0.5 · protocol 1", or undefined before the first connection. */
  readonly version: string | undefined;
  /** The VoiceOver label of the ellipsis button, such as "desktop actions". */
  readonly actionsLabel: string;
}

/** The glyph and the colour of each state word. */
const STATE_LOOK: Readonly<
  Record<MachineTone, { readonly glyph: string; readonly tone: TextTone }>
> = {
  connected: { glyph: "✓ ", tone: "state_success" },
  connecting: { glyph: "… ", tone: "text_muted" },
  starting: { glyph: "… ", tone: "text_muted" },
  offline: { glyph: "", tone: "text_subtle" },
  repair: { glyph: "✗ ", tone: "state_error" },
  update: { glyph: "✗ ", tone: "state_error" },
};

/**
 * The text of the row of `machine`. `status` is what efrd said on the last connection,
 * which the phone keeps; `now` and `zone` give "seen 13:58". Settings does not say
 * "Tailscale is not connected": an unreachable machine reads "offline".
 */
export function machineRowText(
  machine: Machine,
  connection: MachineConnection,
  status: MachineStatus | undefined,
  now: number,
  zone: ZoneOffset,
): MachineRowText {
  // The connection knows only this run of the app; the kept status knows the last one.
  const seenAt = connection.seenAt ?? status?.seenAt;
  const words = machineStateWords({ ...connection, seenAt }, now, zone, false);
  const look = STATE_LOOK[words.tone];
  const offline = words.tone === "offline";
  return {
    name: machine.name,
    dim: offline,
    state: `${look.glyph}${words.line}`,
    stateTone: look.tone,
    node: offline ? undefined : nodeName(machine),
    version:
      status === undefined
        ? undefined
        : `efrd ${status.version} · protocol ${String(status.protocol)}`,
    actionsLabel: `${machine.name} actions`,
  };
}

/** The node name of a machine: its MagicDNS name, or its tailnet address without one. */
export function nodeName(machine: Machine): string {
  return machine.fqdn ?? machine.device.host;
}

/** The ids of the row menu. */
export type MachineAction = "copy" | "repair" | "forget";

/** The action of a menu id, or undefined for an id that the menu does not have. */
export function machineAction(id: string): MachineAction | undefined {
  switch (id) {
    case "copy":
    case "repair":
    case "forget":
      return id;
    default:
      return undefined;
  }
}

/**
 * The row menu of a machine. Every item works while the machine is offline. Forget is in
 * a section of its own, in system red.
 */
export const MACHINE_MENU: readonly MenuEntry[] = [
  { id: "copy", title: "Copy node name", image: "doc.on.doc" },
  { id: "repair", title: "Re-pair", image: "arrow.triangle.2.circlepath" },
  {
    section: "forget",
    items: [{ id: "forget", title: "Forget this machine", image: "trash", destructive: true }],
  },
];

/** The id of the destructive action of the Forget sheet. */
export const FORGET_ACTION = "forget";

/**
 * The Forget sheet of the machine `name`. `last` is true when no other machine is
 * paired: the app then goes back to pairing.
 */
export function forgetSheet(name: string, last: boolean): ActionSheet {
  const message = last
    ? `${name} is the last machine on this iPhone. Its pairing key is deleted and the app goes back to pairing. Nothing changes on ${name}.`
    : `This iPhone deletes its pairing key for ${name} and stops showing its conversations. Nothing changes on ${name}. To use it again, pair with efr device add.`;
  return {
    title: `Forget ${name}?`,
    message,
    actions: [{ id: FORGET_ACTION, label: `Forget ${name}`, destructive: true }],
  };
}

/** The alert after a Forget whose key could not be deleted. */
export interface ForgetFailedText {
  readonly title: string;
  readonly message: string;
}

/**
 * The alert when the phone could not delete the pairing key of `name`. The machine is
 * gone from the list, but the key comes back with the next start of the app.
 */
export function forgetFailedText(name: string): ForgetFailedText {
  return {
    title: `Could not forget ${name}`,
    message: `The pairing key of ${name} is still on this iPhone. Forget ${name} again after the next start of the app.`,
  };
}

/** The version of the app: "0.1.0 (14)", or "0.1.0" without a build number. */
export function versionText(version: string | undefined, build: string | undefined): string {
  const name = version === undefined || version === "" ? "unknown" : version;
  return build === undefined || build === "" ? name : `${name} (${build})`;
}

/** The time zone of the phone at `now`, in minutes east of UTC. */
export function zoneAt(now: number): ZoneOffset {
  return -new Date(now).getTimezoneOffset();
}
