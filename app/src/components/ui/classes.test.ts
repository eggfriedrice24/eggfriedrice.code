// The class maps of the kit against the theme: each class must be one that
// scripts/gen-theme.mjs makes, or uniwind gives the view no style and nothing fails.

import { describe, expect, it } from "vitest";

import { BRAILLE, DOT, RADII, SPACING } from "@/theme/metrics";
import { roles } from "@/theme/roles";
import { kebab } from "@/theme/theme-css";
import { TYPE } from "@/theme/type";

import { BUTTON_FRAME, BUTTON_LABEL } from "./button-classes";
import { cn } from "./cn";
import { TEXT_TONE_CLASS, TYPE_CLASS } from "./text-classes";

describe("cn", () => {
  it("joins the classes that are set with one space", () => {
    expect(cn("a", false, "b", undefined, null, "", "c")).toBe("a b c");
    expect(cn()).toBe("");
    const hidden = false;
    expect(cn(hidden && "x", "y")).toBe("y");
  });
});

describe("the type classes", () => {
  it("have one class for each style of the type scale", () => {
    expect(Object.keys(TYPE_CLASS).sort()).toEqual(Object.keys(TYPE).sort());
  });

  it("name the utility that the theme makes from each style", () => {
    for (const [name, className] of Object.entries(TYPE_CLASS)) {
      expect(className).toBe(`text-${kebab(name)}`);
    }
  });
});

describe("the tone classes", () => {
  it("name only colour roles of roles.ts", () => {
    for (const [role, className] of Object.entries(TEXT_TONE_CLASS)) {
      expect(role in roles).toBe(true);
      expect(className).toBe(`text-${kebab(role)}`);
    }
  });
});

describe("the button classes", () => {
  it("use the heights and radii of the metrics", () => {
    expect(SPACING.button).toBe(50);
    expect(RADII.control).toBe(12);
    expect(BUTTON_FRAME.large).toBe("h-button rounded-control");
    expect(SPACING.hit).toBe(44);
    expect(RADII.box).toBe(10);
    expect(BUTTON_FRAME.medium).toBe("h-hit rounded-box");
    expect(BUTTON_LABEL).toEqual({ large: TYPE_CLASS.buttonLarge, medium: TYPE_CLASS.button });
  });
});

describe("the sizes that the views spell as classes", () => {
  // A spacing step is 1 pt, so `w-7` is 7 pt. braille-cell.tsx and dot.tsx spell these.
  it("match the metrics", () => {
    expect(BRAILLE).toEqual({
      inline: { width: 7, height: 13 },
      row: { width: 8, height: 15 },
      button: { width: 9, height: 17 },
    });
    expect(DOT).toEqual({ small: 7, large: 8 });
  });
});
