// The short texts of the conversation lists: the time of the last activity, the status
// word and the working directory (design-spec sections 3.14 and 3.15). Pure. The times
// use the local calendar of the phone; the names of months and days are English, as is
// all text of the app.

import type { ConversationStatus } from "@/lib/protocol/generated";

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const;
const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

/** The start of the local day of `time` (ms since the epoch). */
export function startOfLocalDay(time: number): number {
  const date = new Date(time);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

/**
 * The start of the local day `days` days before the day of `time`. It counts calendar
 * days, so a day of 23 or 25 hours (a change of daylight saving time) counts as one.
 */
export function startOfLocalDayBefore(time: number, days: number): number {
  const date = new Date(startOfLocalDay(time));
  date.setDate(date.getDate() - days);
  return date.getTime();
}

/**
 * The case of the words of a list text. The title menu is a system menu with capital
 * words ("Needs approval", "Yesterday"); the sheet draws them lowercase in mono
 * ("needs approval", "yesterday"). Weekdays and months keep their capital in both.
 */
export type ListCase = "menu" | "sheet";

/**
 * When `then` was, seen at `now` (both ms since the epoch), in the short form of the
 * lists: `now`, `5m` and `3h` on the same day, `Yesterday`, a weekday such as `Mon` in
 * the last week, `Oct 19` in the same year, and `Oct 19 2025` before it. A time after
 * `now` (a clock that is a little off) reads as `now`.
 */
export function listTime(then: number, now: number, wordCase: ListCase = "menu"): string {
  const today = startOfLocalDay(now);
  if (then >= today) {
    const minutes = Math.floor(Math.max(0, now - then) / 60_000);
    if (minutes < 1) {
      return "now";
    }
    if (minutes < 60) {
      return `${minutes}m`;
    }
    return `${Math.floor(minutes / 60)}h`;
  }
  if (then >= startOfLocalDayBefore(now, 1)) {
    return wordCase === "menu" ? "Yesterday" : "yesterday";
  }
  const date = new Date(then);
  if (then >= startOfLocalDayBefore(now, 6)) {
    return WEEKDAYS[date.getDay()] ?? "";
  }
  const day = `${MONTHS[date.getMonth()] ?? ""} ${date.getDate()}`;
  return date.getFullYear() === new Date(now).getFullYear() ? day : `${day} ${date.getFullYear()}`;
}

const STATUS_WORDS: Readonly<Record<ConversationStatus, string | undefined>> = {
  awaiting_approval: "Needs approval",
  running: "Running",
  idle: undefined,
};

/** The status word of a list row: `Needs approval`, `Running`, or none for idle. */
export function statusWord(
  status: ConversationStatus,
  wordCase: ListCase = "menu",
): string | undefined {
  const word = STATUS_WORDS[status];
  return wordCase === "menu" || word === undefined ? word : word.toLowerCase();
}

/** The title of a row; a conversation without a title yet reads as `Untitled`. */
export function rowTitle(title: string | undefined): string {
  return title ?? "Untitled";
}

const HOME = /^(?:\/home\/[^/]+|\/Users\/[^/]+|\/root)(?=\/|$)/;

/**
 * A working directory as the lists show it: the home directory of the machine reads as
 * `~`. The phone does not know that directory, so it takes `/home/<name>`,
 * `/Users/<name>` and `/root` as one.
 */
export function shortPath(path: string): string {
  return path.replace(HOME, "~");
}

/** The parts of a subtitle joined with ` · `, without the empty ones. */
export function joinParts(parts: readonly (string | undefined)[]): string {
  return parts.filter((part) => part !== undefined && part.length > 0).join(" · ");
}
