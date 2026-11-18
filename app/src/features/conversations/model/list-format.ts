// The short texts of the conversation lists (board S-Rows): the time of the last
// activity, the status word, the folder and the title. Pure. The times use the local
// calendar of the phone; the names of months and days are English, as is all text of
// the app.

import type { RowState } from "@/lib/protocol/status";

const WEEKDAYS = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
] as const;
const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
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
 * The day of a time, seen from now: the day sections of the sheet, and the form of its
 * time. `week` is the 5 days before yesterday.
 */
export type ListDay = "today" | "yesterday" | "week" | "older";

/** The day of `then`, seen at `now` (both ms since the epoch). A later time is today. */
export function listDay(then: number, now: number): ListDay {
  if (then >= startOfLocalDay(now)) {
    return "today";
  }
  if (then >= startOfLocalDayBefore(now, 1)) {
    return "yesterday";
  }
  return then >= startOfLocalDayBefore(now, 6) ? "week" : "older";
}

/** The minutes from `then` to `now`; a time after `now` (a clock that is off) gives 0. */
function minutesBetween(then: number, now: number): number {
  return Math.floor(Math.max(0, now - then) / 60_000);
}

function clockTime(date: Date): string {
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

function short(name: string): string {
  return name.slice(0, 3);
}

/**
 * When `then` was, seen at `now`, in the short form of the lists (S-Rows "When"): `now`
 * under a minute, `12m` under an hour, `3h` on the same day, the clock time `18:40`
 * yesterday, the weekday `Tue` in the 5 days before, `Oct 12` in the same year and
 * `Oct 12, 2025` before it.
 */
export function listTime(then: number, now: number): string {
  const date = new Date(then);
  switch (listDay(then, now)) {
    case "today": {
      const minutes = minutesBetween(then, now);
      if (minutes < 1) {
        return "now";
      }
      return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h`;
    }
    case "yesterday":
      return clockTime(date);
    case "week":
      return short(WEEKDAYS[date.getDay()] ?? "");
    default:
      return shortDate(date, now);
  }
}

/** `Oct 12` in the year of `now`, else `Oct 12, 2025`. */
function shortDate(date: Date, now: number): string {
  const day = `${short(MONTHS[date.getMonth()] ?? "")} ${date.getDate()}`;
  return sameYear(date, now) ? day : `${day}, ${date.getFullYear()}`;
}

/** `October 12` in the year of `now`, else `October 12, 2025`. */
function longDate(date: Date, now: number): string {
  const day = `${MONTHS[date.getMonth()] ?? ""} ${date.getDate()}`;
  return sameYear(date, now) ? day : `${day}, ${date.getFullYear()}`;
}

/**
 * The time of `listTime` in full words, for VoiceOver: `Now`, `12 minutes ago`,
 * `3 hours ago`, `Yesterday at 18:40`, `Tuesday`, `October 12` or `October 12, 2025`.
 */
export function spokenTime(then: number, now: number): string {
  const date = new Date(then);
  switch (listDay(then, now)) {
    case "today": {
      const minutes = minutesBetween(then, now);
      if (minutes < 1) {
        return "Now";
      }
      return minutes < 60 ? ago(minutes, "minute") : ago(Math.floor(minutes / 60), "hour");
    }
    case "yesterday":
      return `Yesterday at ${clockTime(date)}`;
    case "week":
      return WEEKDAYS[date.getDay()] ?? "";
    default:
      return longDate(date, now);
  }
}

function ago(count: number, unit: string): string {
  return count === 1 ? `1 ${unit} ago` : `${count} ${unit}s ago`;
}

function sameYear(date: Date, now: number): boolean {
  return date.getFullYear() === new Date(now).getFullYear();
}

/** An age as the screen shows it, and in full words for VoiceOver. */
export interface ListAge {
  readonly text: string;
  readonly label: string;
}

/**
 * The age of the rows that the phone keeps of an offline machine (SH-Offline), seen at
 * `now`: `12m ago` (at least `1m ago`), `3h ago` on the same day, `yesterday`, then the
 * date as the lists give it, `Nov 12` or `Nov 12, 2025`. The label says each in full
 * words: `2 hours ago`, `November 12`.
 */
export function listAge(then: number, now: number): ListAge {
  switch (listDay(then, now)) {
    case "today": {
      const minutes = Math.max(1, minutesBetween(then, now));
      const hours = Math.floor(minutes / 60);
      return minutes < 60
        ? { text: `${minutes}m ago`, label: ago(minutes, "minute") }
        : { text: `${hours}h ago`, label: ago(hours, "hour") };
    }
    case "yesterday":
      return { text: "yesterday", label: "yesterday" };
    default: {
      const date = new Date(then);
      return { text: shortDate(date, now), label: longDate(date, now) };
    }
  }
}

const STATUS_WORDS: Readonly<Record<RowState, string | undefined>> = {
  approval: "needs approval",
  answer: "needs an answer",
  terminal: "answer in the terminal",
  running: "running",
  failed: "failed",
  idle: undefined,
};

/**
 * The status word of a list row (spec 0.0.12, section 6.1): `needs approval`,
 * `needs an answer`, `answer in the terminal`, `running`, `failed`, or none for idle.
 */
export function statusWord(state: RowState): string | undefined {
  return STATUS_WORDS[state];
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

/**
 * The folder of a row (S-Rows "Folder"): the last part of the working directory, `~`
 * for the home directory and `/` for the root.
 */
export function folderName(path: string): string {
  const trimmed = shortPath(path).replace(/\/+$/, "");
  if (trimmed === "") {
    return "/";
  }
  return trimmed.slice(trimmed.lastIndexOf("/") + 1);
}
