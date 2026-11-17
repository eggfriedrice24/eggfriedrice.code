import { describe, expect, it } from "vitest";

import {
  folderName,
  listDay,
  listTime,
  rowTitle,
  shortPath,
  spokenTime,
  startOfLocalDay,
  startOfLocalDayBefore,
  statusWord,
} from "./list-format";
import { october } from "./testing";

// Monday, 26 October 2026, 14:00 local time. In Europe, the clocks went back an hour in
// the night of Sunday 25 October, so that day has 25 hours there.
const NOW = october(26, 14);
const MINUTE = 60_000;

/** Local time in `year`, `month` (1 to 12), `day`, at `hour:minute`. */
function at(year: number, month: number, day: number, hour = 12, minute = 0): number {
  return new Date(year, month - 1, day, hour, minute).getTime();
}

describe("listTime (S-Rows, When)", () => {
  it("gives now, minutes and hours on the same day", () => {
    expect(listTime(NOW, NOW)).toBe("now");
    expect(listTime(NOW - 59_000, NOW)).toBe("now");
    expect(listTime(NOW - 12 * MINUTE, NOW)).toBe("12m");
    expect(listTime(NOW - 59 * MINUTE, NOW)).toBe("59m");
    expect(listTime(NOW - 60 * MINUTE, NOW)).toBe("1h");
    expect(listTime(NOW - 3 * 60 * MINUTE, NOW)).toBe("3h");
  });

  it("reads a time after now as now", () => {
    expect(listTime(NOW + 5 * MINUTE, NOW)).toBe("now");
  });

  it("gives the clock time in 24 h yesterday", () => {
    expect(listTime(october(25, 18, 40), NOW)).toBe("18:40");
    expect(listTime(october(25, 23, 59), NOW)).toBe("23:59");
    expect(listTime(october(25, 0, 0), NOW)).toBe("00:00");
    expect(listTime(october(25, 9, 5), NOW)).toBe("09:05");
  });

  it("gives the weekday for the 5 days before yesterday", () => {
    expect(listTime(october(24, 9), NOW)).toBe("Sat");
    expect(listTime(october(20, 0, 0), NOW)).toBe("Tue");
  });

  it("gives the month and the day, with a comma and the year for another year", () => {
    expect(listTime(october(19, 23, 59), NOW)).toBe("Oct 19");
    expect(listTime(october(12), NOW)).toBe("Oct 12");
    expect(listTime(at(2026, 1, 1, 0, 0), NOW)).toBe("Jan 1");
    expect(listTime(at(2025, 10, 12), NOW)).toBe("Oct 12, 2025");
  });

  it("counts calendar days over the end of a year", () => {
    const newYear = at(2027, 1, 2, 10);
    expect(listTime(at(2027, 1, 1, 15), newYear)).toBe("15:00");
    expect(listTime(at(2026, 12, 31, 9), newYear)).toBe("Thu");
    expect(listTime(at(2026, 12, 27, 0, 0), newYear)).toBe("Sun");
    expect(listTime(at(2026, 12, 26, 23, 59), newYear)).toBe("Dec 26, 2026");
    expect(listTime(at(2026, 12, 31, 23, 50), at(2027, 1, 1, 0, 30))).toBe("23:50");
    expect(listTime(at(2026, 12, 31, 23, 50), at(2026, 12, 31, 23, 59))).toBe("9m");
  });

  // NOTE: these cases hold in every time zone; in a zone with daylight saving time (such
  // as Europe/Berlin) they cross a day of 23 or 25 hours.
  it("counts calendar days over a change of daylight saving time", () => {
    const spring = at(2026, 3, 30, 1, 30);
    expect(listTime(at(2026, 3, 29, 0, 30), spring)).toBe("00:30");
    expect(listTime(at(2026, 3, 28, 23, 30), spring)).toBe("Sat");
    const autumn = october(26, 0, 30);
    expect(listTime(october(25, 0, 10), autumn)).toBe("00:10");
    expect(listTime(october(24, 23, 50), autumn)).toBe("Sat");
    expect(listTime(october(19, 23, 59), autumn)).toBe("Oct 19");
  });
});

describe("spokenTime", () => {
  it("says the time of the row in full words", () => {
    expect(spokenTime(NOW, NOW)).toBe("Now");
    expect(spokenTime(NOW - MINUTE, NOW)).toBe("1 minute ago");
    expect(spokenTime(NOW - 12 * MINUTE, NOW)).toBe("12 minutes ago");
    expect(spokenTime(NOW - 60 * MINUTE, NOW)).toBe("1 hour ago");
    expect(spokenTime(NOW - 3 * 60 * MINUTE, NOW)).toBe("3 hours ago");
    expect(spokenTime(october(25, 18, 40), NOW)).toBe("Yesterday at 18:40");
    expect(spokenTime(october(20), NOW)).toBe("Tuesday");
    expect(spokenTime(october(12), NOW)).toBe("October 12");
    expect(spokenTime(at(2025, 12, 31), NOW)).toBe("December 31, 2025");
  });
});

describe("listDay", () => {
  it("gives today, yesterday, the week and older by calendar days", () => {
    expect(listDay(NOW + MINUTE, NOW)).toBe("today");
    expect(listDay(october(26, 0, 0), NOW)).toBe("today");
    expect(listDay(october(26, 0, 0) - 1, NOW)).toBe("yesterday");
    expect(listDay(october(25, 0, 0), NOW)).toBe("yesterday");
    expect(listDay(october(25, 0, 0) - 1, NOW)).toBe("week");
    expect(listDay(october(20, 0, 0), NOW)).toBe("week");
    expect(listDay(october(20, 0, 0) - 1, NOW)).toBe("older");
  });
});

describe("startOfLocalDayBefore", () => {
  it("counts calendar days", () => {
    expect(startOfLocalDay(NOW)).toBe(october(26, 0, 0));
    expect(startOfLocalDayBefore(NOW, 0)).toBe(october(26, 0, 0));
    expect(startOfLocalDayBefore(NOW, 6)).toBe(october(20, 0, 0));
    expect(startOfLocalDayBefore(october(1), 1)).toBe(at(2026, 9, 30, 0, 0));
  });
});

describe("statusWord", () => {
  it("names a waiting and a running conversation, and no idle one", () => {
    expect(statusWord("awaiting_approval")).toBe("needs approval");
    expect(statusWord("running")).toBe("running");
    expect(statusWord("idle")).toBeUndefined();
  });
});

describe("shortPath", () => {
  it("writes the home directory as ~", () => {
    expect(shortPath("/home/me/code/efr")).toBe("~/code/efr");
    expect(shortPath("/home/me")).toBe("~");
    expect(shortPath("/Users/me/.dotfiles")).toBe("~/.dotfiles");
    expect(shortPath("/root/x")).toBe("~/x");
  });

  it("keeps other paths", () => {
    expect(shortPath("/var/log")).toBe("/var/log");
    expect(shortPath("/home")).toBe("/home");
    expect(shortPath("/rootfs/x")).toBe("/rootfs/x");
    expect(shortPath("/srv/home/me")).toBe("/srv/home/me");
  });
});

describe("folderName (S-Rows, Folder)", () => {
  it("gives the last part of the path, and ~ for home", () => {
    expect(folderName("/home/me/code/efr")).toBe("efr");
    expect(folderName("/home/me/code/nimbus-v2/")).toBe("nimbus-v2");
    expect(folderName("/home/me")).toBe("~");
    expect(folderName("/Users/me/")).toBe("~");
    expect(folderName("/root")).toBe("~");
    expect(folderName("/var/log")).toBe("log");
    expect(folderName("/")).toBe("/");
    expect(folderName("relative")).toBe("relative");
  });
});

describe("rowTitle", () => {
  it("names a conversation without a title", () => {
    expect(rowTitle(undefined)).toBe("Untitled");
    expect(rowTitle("Fix steer race")).toBe("Fix steer race");
  });
});
