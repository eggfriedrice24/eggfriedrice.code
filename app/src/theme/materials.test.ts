import { describe, expect, it } from "vitest";

import { MATERIALS, OPACITY } from "./materials";
import { CURSOR, DIFF_GUTTER, HAIRLINE, RADII, SPACING } from "./metrics";
import { roles } from "./roles";

describe("the materials", () => {
  it("are roles at the opacities of the canvas", () => {
    expect(MATERIALS.chrome.fill).toBe("rgba(13, 17, 26, 0.76)");
    expect(MATERIALS.glassButton.fill).toBe("rgba(18, 23, 34, 0.66)");
    expect(MATERIALS.jump.fill).toBe("rgba(18, 23, 34, 0.7)");
    expect(MATERIALS.dim).toBe("rgba(9, 12, 19, 0.5)");
    expect(MATERIALS.glassButton.borderColor).toBe(roles.border_subtle);
    expect(MATERIALS.jump.shadow.color).toBe("rgba(9, 12, 19, 0.6)");
  });

  it("keep every opacity between 0 and 1", () => {
    expect(Object.values(OPACITY).filter((value) => value <= 0 || value >= 1)).toEqual([]);
  });
});

describe("the metrics", () => {
  it("keep the decisions of the plan", () => {
    expect(SPACING.hit).toBe(44);
    expect(SPACING.item).toBe(12);
    expect(SPACING.gutter).toBe(20);
    expect(HAIRLINE).toBe(0.5);
    expect(DIFF_GUTTER).toEqual({ width: 28, paddingRight: 6, codeInset: 4 });
    expect(CURSOR.shell).toEqual({ width: 9, height: 19 });
    expect(RADII.box).toBe(10);
  });
});
