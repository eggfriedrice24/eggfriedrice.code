import { describe, expect, it } from "vitest";

import { pairRouteMachine } from "./pair-route-schemas";

const LAPTOP = "019a9b1c-3d00-7a10-8b20-0000000000d2";

describe("pairRouteMachine", () => {
  it("gives the machine of a re-pair", () => {
    expect(pairRouteMachine({ machine: LAPTOP })).toBe(LAPTOP);
  });

  it("pairs a new machine without the param, or for a param that is not one daemon id", () => {
    expect(pairRouteMachine({})).toBeNull();
    expect(pairRouteMachine({ machine: "laptop" })).toBeNull();
    expect(pairRouteMachine({ machine: [LAPTOP, LAPTOP] })).toBeNull();
    expect(pairRouteMachine(undefined)).toBeNull();
  });
});
