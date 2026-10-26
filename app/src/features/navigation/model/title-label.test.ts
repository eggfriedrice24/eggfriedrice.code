import { describe, expect, it } from "vitest";

import { titleLabel } from "./title-label";

describe("titleLabel", () => {
  it("says that the title opens all conversations, with the approvals that wait", () => {
    expect(titleLabel("desktop", 0)).toBe("desktop, all conversations");
    expect(titleLabel("Fix steer race", 1)).toBe(
      "Fix steer race, all conversations, 1 approval waiting",
    );
    expect(titleLabel("Fix steer race", 3)).toBe(
      "Fix steer race, all conversations, 3 approvals waiting",
    );
  });
});
