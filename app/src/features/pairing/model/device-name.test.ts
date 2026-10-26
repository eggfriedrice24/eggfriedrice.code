import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { DEVICE_NAME_MAX_CHARS, deviceName, FALLBACK_DEVICE_NAME } from "./device-name";

/** The ranges of code points that efr-store refuses in a name. */
const REFUSED: readonly (readonly [number, number])[] = [
  [0x00, 0x1f],
  [0x7f, 0x9f],
  [0x061c, 0x061c],
  [0x200b, 0x200f],
  [0x2028, 0x202e],
  [0x2060, 0x2069],
  [0xfeff, 0xfeff],
  [0xd800, 0xdfff],
];

function isRefused(char: string): boolean {
  const point = char.codePointAt(0) ?? 0;
  return REFUSED.some(([low, high]) => point >= low && point <= high);
}

describe("deviceName", () => {
  it("keeps a plain name", () => {
    expect(deviceName("Anna's iPhone")).toBe("Anna's iPhone");
    expect(deviceName("Gr\u00fc\u00dfe \u{1f642}")).toBe("Gr\u00fc\u00dfe \u{1f642}");
  });

  it("removes control and invisible format characters", () => {
    expect(deviceName("Anna\u0000's\u200b iPhone\u202e")).toBe("Anna's iPhone");
    expect(deviceName("a\u0085b\u2066c\u061cd")).toBe("abcd");
    expect(deviceName("a\ud800b")).toBe("ab");
  });

  it("makes white space one space and trims it", () => {
    expect(deviceName("  Anna's\n\tiPhone  ")).toBe("Anna's iPhone");
  });

  it("gives the fallback name when nothing usable is left", () => {
    expect(deviceName(undefined)).toBe(FALLBACK_DEVICE_NAME);
    expect(deviceName(null)).toBe(FALLBACK_DEVICE_NAME);
    expect(deviceName(" \u200b\u0007 ")).toBe(FALLBACK_DEVICE_NAME);
  });

  it("cuts a long name at 64 characters, not at 64 UTF-16 units", () => {
    expect(Array.from(deviceName("\u{1f642}".repeat(100))).length).toBe(DEVICE_NAME_MAX_CHARS);
  });

  it("always gives a name that efr-store takes", () => {
    fc.assert(
      fc.property(fc.string({ unit: "binary", maxLength: 200 }), (raw) => {
        const name = deviceName(raw);
        const chars = Array.from(name);
        expect(chars.length).toBeGreaterThanOrEqual(1);
        expect(chars.length).toBeLessThanOrEqual(DEVICE_NAME_MAX_CHARS);
        expect(name.trim()).toBe(name);
        expect(chars.filter(isRefused)).toEqual([]);
      }),
    );
  });
});
