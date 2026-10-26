import { describe, expect, it } from "vitest";

import {
  joinParts,
  listTime,
  rowTitle,
  shortPath,
  startOfLocalDay,
  startOfLocalDayBefore,
  statusWord,
} from "./list-format";
import { october } from "./testing";

// Monday, 26 October 2026, 14:00 local time.
const NOW = october(26, 14);

describe("listTime", () => {
  it("gives minutes and hours on the same day", () => {
    expect(listTime(NOW, NOW)).toBe("now");
    expect(listTime(NOW - 59_000, NOW)).toBe("now");
    expect(listTime(NOW - 2 * 60_000, NOW)).toBe("2m");
    expect(listTime(NOW - 59 * 60_000, NOW)).toBe("59m");
    expect(listTime(NOW - 60 * 60_000, NOW)).toBe("1h");
    expect(listTime(october(26, 0, 0), NOW)).toBe("14h");
  });

  it("reads a time after now as now", () => {
    expect(listTime(NOW + 5 * 60_000, NOW)).toBe("now");
  });

  it("gives Yesterday, then the weekday for the last week", () => {
    expect(listTime(october(25, 23, 59), NOW)).toBe("Yesterday");
    expect(listTime(october(25, 0, 0), NOW)).toBe("Yesterday");
    expect(listTime(october(24, 9), NOW)).toBe("Sat");
    expect(listTime(october(20, 0, 0), NOW)).toBe("Tue");
  });

  it("gives the month and the day before that, with the year when it is another", () => {
    expect(listTime(october(19, 23, 59), NOW)).toBe("Oct 19");
    expect(listTime(october(12), NOW)).toBe("Oct 12");
    expect(listTime(new Date(2025, 11, 31, 12).getTime(), NOW)).toBe("Dec 31 2025");
  });

  it("writes Yesterday lowercase in the sheet, and keeps the weekday", () => {
    expect(listTime(october(25), NOW, "sheet")).toBe("yesterday");
    expect(listTime(october(24), NOW, "sheet")).toBe("Sat");
  });
});

describe("startOfLocalDayBefore", () => {
  it("counts calendar days", () => {
    expect(startOfLocalDay(NOW)).toBe(october(26, 0, 0));
    expect(startOfLocalDayBefore(NOW, 0)).toBe(october(26, 0, 0));
    expect(startOfLocalDayBefore(NOW, 6)).toBe(october(20, 0, 0));
    expect(startOfLocalDayBefore(october(1), 1)).toBe(new Date(2026, 8, 30).getTime());
  });
});

describe("statusWord", () => {
  it("names a waiting and a running conversation, and no idle one", () => {
    expect(statusWord("awaiting_approval")).toBe("Needs approval");
    expect(statusWord("running")).toBe("Running");
    expect(statusWord("idle")).toBeUndefined();
    expect(statusWord("awaiting_approval", "sheet")).toBe("needs approval");
    expect(statusWord("running", "sheet")).toBe("running");
    expect(statusWord("idle", "sheet")).toBeUndefined();
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

describe("joinParts and rowTitle", () => {
  it("joins the parts that have text", () => {
    expect(joinParts(["desktop", undefined, "", "~", "2m"])).toBe("desktop · ~ · 2m");
    expect(joinParts([])).toBe("");
  });

  it("names a conversation without a title", () => {
    expect(rowTitle(undefined)).toBe("Untitled");
    expect(rowTitle("Fix steer race")).toBe("Fix steer race");
  });
});
