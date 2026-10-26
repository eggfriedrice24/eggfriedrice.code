// The words of a machine's state in the machine menu, in Settings and in the offline line
// (design-spec sections 2, 3.13 and 3.16): "Connected", "Connecting…", "Offline · seen 2
// days ago", "Tailscale is not connected", "Re-pair needed", "efrd is starting". Pure, for
// the tests.

import type { MachineConnection } from "./machine-connection";

/** The kind of state, for the colour of the word. */
export type MachineTone = "connected" | "connecting" | "offline" | "starting" | "repair" | "update";

/** The words of one machine's state. */
export interface MachineStateWords {
  readonly tone: MachineTone;
  /** The subtitle of the machine in the machine menu, such as "Offline · seen 13:58". */
  readonly subtitle: string;
  /**
   * The state word of the Settings row and the offline line, in lower case, such as
   * "connected" or "offline · seen 2 days ago".
   */
  readonly line: string;
}

/** The time zone of the phone, for "seen 13:58": minutes east of UTC. */
export type ZoneOffset = number;

const DAY_MS = 86_400_000;

/**
 * The words of `connection` at `now`. `tailnetDown` is true when no machine of the phone
 * answers: the phone is then most likely off its tailnet, so an unreachable machine reads
 * "Tailscale is not connected" instead of "Offline".
 */
export function machineStateWords(
  connection: MachineConnection,
  now: number,
  zone: ZoneOffset,
  tailnetDown: boolean,
): MachineStateWords {
  const { view, seenAt } = connection;
  switch (view.kind) {
    case "online":
      return { tone: "connected", subtitle: "Connected", line: "connected" };
    case "connecting":
      return { tone: "connecting", subtitle: "Connecting…", line: "connecting" };
    case "starting":
      return { tone: "starting", subtitle: "efrd is starting", line: "efrd is starting" };
    case "repair_needed":
    case "unpaired":
      return { tone: "repair", subtitle: "Re-pair needed", line: "re-pair needed" };
    case "update_needed":
      return { tone: "update", subtitle: "Update needed", line: "update needed" };
    case "unreachable":
      if (tailnetDown) {
        const words = "Tailscale is not connected";
        return { tone: "offline", subtitle: words, line: words.toLowerCase() };
      }
      return offline(seenAt, now, zone);
    default:
      // `retrying`: the connection closed, and the client tries again soon.
      return offline(seenAt, now, zone);
  }
}

/**
 * True when every machine with a client is unreachable, and there is at least one. A
 * machine without a key, or one that needs the user, does not count.
 */
export function isTailnetDown(connections: readonly MachineConnection[]): boolean {
  let unreachable = 0;
  for (const { view } of connections) {
    if (view.kind === "unreachable") {
      unreachable += 1;
    } else if (view.kind !== "repair_needed" && view.kind !== "update_needed") {
      return false;
    }
  }
  return unreachable > 0;
}

function offline(seenAt: number | undefined, now: number, zone: ZoneOffset): MachineStateWords {
  if (seenAt === undefined) {
    return { tone: "offline", subtitle: "Offline", line: "offline" };
  }
  const seen = seenText(seenAt, now, zone);
  return { tone: "offline", subtitle: `Offline · ${seen}`, line: `offline · ${seen}` };
}

/**
 * When the machine was last seen: "seen 13:58" on the same day, "seen yesterday", else
 * "seen 3 days ago". `zone` is the time zone of the phone, in minutes east of UTC.
 */
export function seenText(seenAt: number, now: number, zone: ZoneOffset): string {
  const shift = zone * 60_000;
  const days = Math.floor((now + shift) / DAY_MS) - Math.floor((seenAt + shift) / DAY_MS);
  if (days <= 0) {
    const local = new Date(seenAt + shift);
    return `seen ${pad(local.getUTCHours())}:${pad(local.getUTCMinutes())}`;
  }
  return days === 1 ? "seen yesterday" : `seen ${days} days ago`;
}

function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}
