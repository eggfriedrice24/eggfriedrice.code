import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { forgetPlan, type ForgetPlan, type PlanRoute } from "./forget-plan";

const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const LAPTOP = "019a9b1c-3d00-7a10-8b20-0000000000d2";

const ROOT: PlanRoute = { name: "index" };
const SETTINGS: PlanRoute = { name: "settings" };

function conversation(machine: string, id: string): PlanRoute {
  return { name: "c/[machine]/[id]", params: { machine, id } };
}

/** True when `step` is a conversation of the desktop. */
function ofDesktop(step: PlanRoute | undefined): boolean {
  const params = step?.params;
  return params !== undefined && "machine" in params && params.machine === DESKTOP;
}

/** The rules of a plan: the root stays first, Settings stays on top, the desktop goes. */
function holds(plan: ForgetPlan, routes: readonly PlanRoute[], remaining: number): boolean {
  if (plan.kind === "stay") {
    // Nothing moves only when no conversation of the machine is in the stack.
    return remaining > 0 && !routes.some(ofDesktop);
  }
  const kept = plan.keep.map((index) => routes[index]);
  const settingsStays = remaining === 0 || plan.keep.at(-1) === routes.length - 1;
  return plan.keep[0] === 0 && !kept.some(ofDesktop) && settingsStays;
}

describe("forgetPlan", () => {
  it("resets to the root with a fade after the last machine", () => {
    expect(forgetPlan([ROOT, conversation(DESKTOP, "a"), SETTINGS], DESKTOP, 0)).toEqual({
      kind: "reset",
      keep: [0],
      fade: true,
    });
    expect(forgetPlan([ROOT, SETTINGS], DESKTOP, 0)).toEqual({
      kind: "reset",
      keep: [0],
      fade: true,
    });
  });

  it("goes back to the blank root and keeps Settings when the machine is in the stack", () => {
    const routes = [ROOT, conversation(LAPTOP, "b"), conversation(DESKTOP, "a"), SETTINGS];
    expect(forgetPlan(routes, DESKTOP, 1)).toEqual({ kind: "reset", keep: [0, 3], fade: false });
  });

  it("moves nothing when no conversation of the machine is in the stack", () => {
    const routes = [ROOT, conversation(LAPTOP, "b"), SETTINGS];
    expect(forgetPlan(routes, DESKTOP, 1)).toEqual({ kind: "stay" });
    expect(forgetPlan([ROOT, SETTINGS], DESKTOP, 1)).toEqual({ kind: "stay" });
    expect(forgetPlan([], DESKTOP, 0)).toEqual({ kind: "stay" });
  });

  it("keeps the root first and never keeps a conversation of the forgotten machine", () => {
    const route = fc.oneof(
      fc.constant(SETTINGS),
      fc
        .tuple(fc.constantFrom(DESKTOP, LAPTOP), fc.string())
        .map(([machine, id]) => conversation(machine, id)),
    );
    fc.assert(
      fc.property(fc.array(route, { maxLength: 10 }), fc.nat(3), (above, remaining) => {
        const routes = [ROOT, ...above, SETTINGS];
        const plan = forgetPlan(routes, DESKTOP, remaining);
        expect(holds(plan, routes, remaining)).toBe(true);
      }),
    );
  });
});
