import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { dropUtf8Prefix, isCodePointBoundary, utf16IndexAtByte, utf8Length } from "./utf8";

// Two oracles that work another way than utf8Length: encodeURIComponent writes each
// UTF-8 byte of a character that is not ASCII as one %XX triplet (it throws on a lone
// surrogate), and the width of each code point by its value.
function bytesByUri(text: string): number {
  const encoded = encodeURIComponent(text);
  const triplets = encoded.match(/%[0-9A-F]{2}/g)?.length ?? 0;
  return encoded.length - triplets * 2;
}

function bytesByCodePoint(text: string): number {
  let bytes = 0;
  for (const point of Array.from(text)) {
    const value = point.codePointAt(0) ?? 0;
    bytes += value < 0x80 ? 1 : value < 0x800 ? 2 : value < 0x10000 ? 3 : 4;
  }
  return bytes;
}

/** Any string of UTF-16 code units, lone surrogates too. */
const anyUnits = fc.string({
  unit: fc.integer({ min: 0, max: 0xffff }).map((unit) => String.fromCharCode(unit)),
});

/** Any valid Unicode text. */
const anyText = fc.string({ unit: "binary" });

describe("utf8Length", () => {
  it("counts the bytes of each width", () => {
    expect(utf8Length("")).toBe(0);
    expect(utf8Length("a")).toBe(1);
    expect(utf8Length("é")).toBe(2);
    expect(utf8Length("€")).toBe(3);
    expect(utf8Length("中文")).toBe(6);
    expect(utf8Length("😀")).toBe(4);
    expect(utf8Length("a😀b")).toBe(6);
  });

  it("counts a lone surrogate as 3 bytes, as TextEncoder does", () => {
    expect(utf8Length("\ud800")).toBe(3);
    expect(utf8Length("\udc00x")).toBe(4);
  });

  it("agrees with encodeURIComponent for any valid text", () => {
    fc.assert(
      fc.property(anyText, (text) => {
        expect(utf8Length(text)).toBe(bytesByUri(text));
      }),
    );
  });

  it("agrees with the width of each code point for any string", () => {
    fc.assert(
      fc.property(anyUnits, (text) => {
        expect(utf8Length(text)).toBe(bytesByCodePoint(text));
      }),
    );
  });
});

describe("utf16IndexAtByte", () => {
  it("finds the end of each code point prefix", () => {
    fc.assert(
      fc.property(anyText, fc.nat(), (text, cut) => {
        const points = Array.from(text);
        const prefix = points.slice(0, cut % (points.length + 1)).join("");
        expect(utf16IndexAtByte(text, utf8Length(prefix))).toBe(prefix.length);
        expect(dropUtf8Prefix(text, utf8Length(prefix))).toBe(text.slice(prefix.length));
      }),
    );
  });

  it("refuses a byte count inside a character or past the end", () => {
    expect(utf16IndexAtByte("é", 1)).toBeUndefined();
    expect(utf16IndexAtByte("😀", 2)).toBeUndefined();
    expect(utf16IndexAtByte("ab", 3)).toBeUndefined();
    expect(utf16IndexAtByte("ab", -1)).toBeUndefined();
    expect(dropUtf8Prefix("中", 1)).toBeUndefined();
  });
});

describe("isCodePointBoundary", () => {
  it("is false only inside a surrogate pair", () => {
    expect(isCodePointBoundary("a😀b", 0)).toBe(true);
    expect(isCodePointBoundary("a😀b", 1)).toBe(true);
    expect(isCodePointBoundary("a😀b", 2)).toBe(false);
    expect(isCodePointBoundary("a😀b", 3)).toBe(true);
    expect(isCodePointBoundary("a😀b", 4)).toBe(true);
  });
});
