import { describe, expect, it } from "vitest";

import { opencode, semantic, efr, treesitter } from "./palette";
import { NO_FILL, roles, withOpacity, type Role } from "./roles";

/** Every string inside `value`, at any depth. */
function strings(value: unknown): string[] {
  if (typeof value === "string") {
    return [value];
  }
  if (value === null || typeof value !== "object") {
    return [];
  }
  return Object.values(value).flatMap(strings);
}

const PALETTE = new Set(strings([semantic, efr, opencode, treesitter]));

/** The relative luminance of a `#rrggbb` colour (WCAG 2.2). */
function luminance(hex: string): number {
  const channel = (start: number) => {
    const value = Number.parseInt(hex.slice(start, start + 2), 16) / 255;
    return value <= 0.040_45 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

/** The WCAG contrast ratio of two `#rrggbb` colours. */
function contrast(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((light ?? 0) + 0.05) / ((dark ?? 0) + 0.05);
}

/**
 * The roles that are not text on a surface: surfaces, fills, borders, rules, the
 * decorative line numbers and disabled glyphs, and the text on a yolk fill (checked with
 * its fill below). Every other role is text on a surface.
 */
const NOT_TEXT: ReadonlySet<string> = new Set<Role>([
  "text_on_accent",
  "surface_0",
  "surface_1",
  "surface_2",
  "selection_bg",
  "text_line_number",
  "text_decorative",
  "border_subtle",
  "border_default",
  "border_accent",
  "markdown_link",
  "markdown_horizontal_rule",
  "diff_added_bg",
  "diff_removed_bg",
  "diff_text_bg",
  "diff_line_number",
]);

const ROLES = Object.entries(roles);

describe("the colour roles", () => {
  it("take every value from the design system", () => {
    expect(ROLES.filter(([, value]) => !PALETTE.has(value))).toEqual([]);
  });

  it("name the roles that the plan maps to apps.opencode", () => {
    expect(roles.markdown_heading).toBe("#e06c75");
    expect(roles.markdown_text).toBe("#d8d3c3");
    expect(roles.border_subtle).toBe("#46516c");
    expect(roles.markdown_link_text).toBe("#78e2d6");
    expect(roles.markdown_link).toBe("#c084fc");
  });

  it("list only real roles as not text", () => {
    expect([...NOT_TEXT].filter((name) => !(name in roles))).toEqual([]);
  });

  it("give every text role at least 4.5:1 on surface_1 and surface_2", () => {
    const weak = ROLES.filter(([name]) => !NOT_TEXT.has(name)).flatMap(([name, value]) =>
      [roles.surface_1, roles.surface_2]
        .filter((surface) => contrast(value, surface) < 4.5)
        .map((surface) => `${name} on ${surface}`),
    );
    expect(weak).toEqual([]);
  });

  it("give the quiet text roles the ratios of the design", () => {
    expect(contrast(roles.text_subtle, roles.surface_1)).toBeCloseTo(6.29, 2);
    expect(contrast(roles.text_muted, roles.surface_1)).toBeCloseTo(7.5, 2);
  });

  it("give text on a fill at least 4.5:1", () => {
    const pairs: [Role, Role][] = [
      ["text_on_accent", "accent_primary"],
      ["text_bright", "selection_bg"],
      ["diff_added", "diff_added_bg"],
      ["diff_removed", "diff_removed_bg"],
      ["diff_highlight_added", "diff_added_bg"],
      ["diff_highlight_removed", "diff_removed_bg"],
    ];
    const weak = pairs.filter(([text, fill]) => contrast(roles[text], roles[fill]) < 4.5);
    expect(weak).toEqual([]);
  });

  it("keep the line numbers decorative: under 4.5:1, so never the only signal", () => {
    expect(contrast(roles.text_line_number, roles.surface_1)).toBeLessThan(4.5);
    expect(contrast(roles.text_decorative, roles.surface_1)).toBeLessThan(4.5);
  });
});

describe("withOpacity", () => {
  it("gives a role at an opacity as rgba", () => {
    expect(withOpacity("surface_1", 0.76)).toBe("rgba(13, 17, 26, 0.76)");
    expect(withOpacity("accent_primary", 0.35)).toBe("rgba(255, 201, 64, 0.35)");
  });
});

describe("NO_FILL", () => {
  it("is no colour", () => {
    expect(NO_FILL).toBe("transparent");
    expect(PALETTE.has(NO_FILL)).toBe(false);
  });
});
