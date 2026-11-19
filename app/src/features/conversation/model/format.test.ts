import { describe, expect, it } from "vitest";

import { ago, baseName, byteSize, clock, contextGauge, lastLines, tokens } from "./format";

describe("byteSize", () => {
  // The table of `a_byte_size_has_one_decimal_only_under_ten` in efr-stdx.
  it.each([
    [0, "0 B"],
    [999, "999 B"],
    [2_000, "2.0 KB"],
    [2_399, "2.3 KB"],
    [9_999, "9.9 KB"],
    [14_200, "14 KB"],
    [999_999, "999 KB"],
    [2_180_000, "2.1 MB"],
    [310_000_000, "310 MB"],
    [1_250_000_000, "1.2 GB"],
  ])("writes %d bytes as %s", (bytes, want) => {
    expect(byteSize(bytes)).toBe(want);
  });
});

const at = (used: number, limit: number) => contextGauge({ tokens: used, limit, window: 0 });

describe("contextGauge", () => {
  it("gives the percent of the limit, rounded down, as efr-cli's tests", () => {
    expect(at(0, 206_720)?.text).toBe("ctx 0%");
    expect(at(89_400, 206_720)?.text).toBe("ctx 43%");
    expect(at(206_719, 206_720)?.text).toBe("ctx 99%");
    expect(at(206_720, 206_720)?.text).toBe("ctx 100%");
    expect(at(240_000, 206_720)?.text).toBe("ctx 116%");
    expect(at(89_400, 206_720)?.detail).toBe("89k of 206k tokens");
  });

  it("takes the level from the percent", () => {
    expect(at(49, 100)?.level).toBe("success");
    expect(at(50, 100)?.level).toBe("warning");
    expect(at(89, 100)?.level).toBe("warning");
    expect(at(90, 100)?.level).toBe("error");
  });

  it("gives no gauge without a limit", () => {
    expect(contextGauge({ tokens: 5, limit: 0, window: 0 })).toBeUndefined();
  });
});

describe("tokens", () => {
  it("cuts the digits as the terminal does", () => {
    expect(tokens(999)).toBe("999");
    expect(tokens(3_250)).toBe("3.2k");
    expect(tokens(24_999)).toBe("24k");
    expect(tokens(1_250_000)).toBe("1.2M");
  });
});

describe("lastLines", () => {
  it("keeps the last lines and drops the empty line after a final newline", () => {
    expect(lastLines("a\nb\nc\nd\n", 3)).toEqual(["b", "c", "d"]);
    expect(lastLines("a\r\nb", 3)).toEqual(["a", "b"]);
    expect(lastLines("", 3)).toEqual([]);
  });
});

describe("clock, ago and baseName", () => {
  it("formats short times and names", () => {
    expect(clock(245)).toBe("4:05");
    expect(clock(-3)).toBe("0:00");
    expect(clock(0.2)).toBe("0:01");
    expect(ago(0, 30_000)).toBe("now");
    expect(ago(0, 5 * 60_000)).toBe("5 min");
    expect(ago(0, 3 * 3_600_000)).toBe("3 h");
    expect(ago(0, 49 * 3_600_000)).toBe("2 d");
    expect(baseName("/home/me/p/efr/")).toBe("efr");
    expect(baseName("/")).toBe("/");
  });
});
