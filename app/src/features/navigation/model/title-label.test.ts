import { describe, expect, it } from "vitest";

import { titleLabel } from "./title-label";

describe("titleLabel", () => {
  it("says that the title opens all conversations, with the words of the waits elsewhere", () => {
    expect(titleLabel("desktop", undefined)).toBe("desktop, all conversations");
    expect(titleLabel("Fix steer race", "approval waiting")).toBe(
      "Fix steer race, all conversations, approval waiting",
    );
    expect(titleLabel("Pick a test database", "3 conversations need you")).toBe(
      "Pick a test database, all conversations, 3 conversations need you",
    );
  });
});
