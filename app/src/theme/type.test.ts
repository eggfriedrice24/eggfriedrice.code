import { describe, expect, it } from "vitest";

import { MIN_TEXT_SIZE, MONO_FONT, TYPE } from "./type";

const STYLES = Object.entries(TYPE);

describe("the type scale", () => {
  it("has no text under 11 pt", () => {
    expect(STYLES.filter(([, style]) => style.fontSize < MIN_TEXT_SIZE)).toEqual([]);
  });

  it("gives every line room for its text", () => {
    const tight = STYLES.filter(
      ([, style]) => "lineHeight" in style && style.lineHeight < style.fontSize,
    );
    expect(tight).toEqual([]);
  });

  it("uses SF Mono exactly for the mono styles", () => {
    const wrong = STYLES.filter(
      ([name, style]) => name.startsWith("mono") !== "fontFamily" in style,
    );
    expect(wrong).toEqual([]);
    expect(MONO_FONT).toBe("ui-monospace");
  });

  it("keeps the sizes of the plan", () => {
    expect(TYPE.body).toEqual({ fontSize: 14, lineHeight: 21 });
    expect(TYPE.input).toEqual({ fontSize: 15, lineHeight: 21 });
    expect(TYPE.monoShell).toMatchObject({ fontSize: 13, lineHeight: 19 });
    expect(TYPE.monoXs).toMatchObject({ fontSize: 11, lineHeight: 16 });
    expect(TYPE.monoCode).toMatchObject({ fontSize: 12, lineHeight: 18 });
    expect(TYPE.monoInline).toMatchObject({ fontSize: 12.5 });
    expect(TYPE.heading1).toEqual({ fontSize: 18, lineHeight: 24, fontWeight: "600" });
    expect(TYPE.heading2).toEqual({ fontSize: 16, lineHeight: 22, fontWeight: "600" });
    expect(TYPE.heading3).toEqual({ fontSize: 15, lineHeight: 20, fontWeight: "600" });
    expect(TYPE.monoCommand).toMatchObject({ fontSize: 12.5, lineHeight: 18 });
    expect(TYPE.reason).toEqual({ fontSize: 13, lineHeight: 18 });
    expect(TYPE.button).toEqual({ fontSize: 15, fontWeight: "600" });
    expect(TYPE.title).toEqual({ fontSize: 15, lineHeight: 20, fontWeight: "600" });
  });
});
