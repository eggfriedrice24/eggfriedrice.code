import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { MAX_STACKED, applyPlan, planOpen, type StackRoute } from "./history";

const M1 = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const M2 = "019a9b1c-3d00-7a10-8b20-0000000000d2";

function c(machine: string, id: string): StackRoute {
  return { name: "c", machine, id };
}

const ROOT: StackRoute = { name: "index" };
const OTHER: StackRoute = { name: "other" };

function stackOf(count: number): StackRoute[] {
  return [ROOT, ...Array.from({ length: count }, (_, index) => c(M1, `a${index}`))];
}

describe("planOpen", () => {
  it("pushes a conversation that is not in the stack", () => {
    expect(planOpen([ROOT], { machine: M1, id: "a" }, { firstSend: false })).toEqual({
      kind: "push",
      instant: false,
      dismiss: 0,
      trim: 0,
    });
  });

  it("pushes the first send without an animation", () => {
    const plan = planOpen([ROOT], { machine: M1, id: "a" }, { firstSend: true });
    expect(plan).toMatchObject({ kind: "push", instant: true });
  });

  it("pops back to a conversation that is in the stack", () => {
    const stack = [ROOT, c(M1, "a"), c(M1, "b"), c(M1, "c")];
    expect(planOpen(stack, { machine: M1, id: "a" }, { firstSend: false })).toEqual({
      kind: "pop-to",
      index: 1,
    });
  });

  it("tells a conversation of another machine with the same id apart", () => {
    const stack = [ROOT, c(M1, "a")];
    expect(planOpen(stack, { machine: M2, id: "a" }, { firstSend: false }).kind).toBe("push");
  });

  it("keeps the conversation on top where it is", () => {
    const stack = [ROOT, c(M1, "a"), c(M1, "b")];
    expect(planOpen(stack, { machine: M1, id: "b" }, { firstSend: false })).toEqual({
      kind: "pop-to",
      index: 2,
    });
  });

  it("dismisses the sheet on top before the push", () => {
    const stack = [ROOT, c(M1, "a"), OTHER];
    expect(planOpen(stack, { machine: M1, id: "b" }, { firstSend: false })).toEqual({
      kind: "push",
      instant: false,
      dismiss: 1,
      trim: 0,
    });
  });

  it("drops the oldest conversation on the ninth push", () => {
    const plan = planOpen(stackOf(MAX_STACKED), { machine: M1, id: "new" }, { firstSend: false });
    expect(plan).toEqual({ kind: "push", instant: false, dismiss: 0, trim: 1 });
    expect(applyPlan(stackOf(MAX_STACKED), { machine: M1, id: "new" }, plan)).toEqual([
      ROOT,
      ...stackOf(MAX_STACKED).slice(2),
      c(M1, "new"),
    ]);
  });

  it("does not trim while there is room", () => {
    const plan = planOpen(stackOf(MAX_STACKED - 1), { machine: M1, id: "x" }, { firstSend: false });
    expect(plan).toMatchObject({ kind: "push", trim: 0 });
  });

  it("drops a screen that sits between the oldest conversations with them", () => {
    const stack = [ROOT, c(M1, "a0"), OTHER, ...stackOf(MAX_STACKED).slice(2)];
    const plan = planOpen(stack, { machine: M1, id: "new" }, { firstSend: false });
    expect(plan).toMatchObject({ kind: "push", trim: 1 });
    const next = applyPlan(stack, { machine: M1, id: "new" }, plan);
    expect(next.filter((route) => route.name === "c")).toHaveLength(MAX_STACKED);
  });
});

/** One step of a random walk over the stack. */
type Step =
  | { readonly kind: "open"; readonly target: ConversationRef; readonly firstSend: boolean }
  | { readonly kind: "back" }
  | { readonly kind: "sheet" }
  | { readonly kind: "new" };

const targetArbitrary = fc.record({
  machine: fc.constantFrom(M1, M2),
  id: fc.constantFrom("a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"),
});

const stepArbitrary: fc.Arbitrary<Step> = fc.oneof(
  {
    weight: 6,
    arbitrary: fc.record({
      kind: fc.constant("open" as const),
      target: targetArbitrary,
      firstSend: fc.boolean(),
    }),
  },
  { weight: 1, arbitrary: fc.constant({ kind: "back" as const }) },
  { weight: 1, arbitrary: fc.constant({ kind: "sheet" as const }) },
  { weight: 1, arbitrary: fc.constant({ kind: "new" as const }) },
);

function key(route: StackRoute): string | undefined {
  return route.name === "c" ? `${route.machine}:${route.id}` : undefined;
}

function walk(steps: readonly Step[], check: (stack: readonly StackRoute[], step: Step) => void) {
  let stack: StackRoute[] = [ROOT];
  for (const step of steps) {
    switch (step.kind) {
      case "open": {
        const plan = planOpen(stack, step.target, { firstSend: step.firstSend });
        stack = applyPlan(stack, step.target, plan);
        break;
      }
      case "back":
        stack = stack.length > 1 ? stack.slice(0, -1) : stack;
        break;
      case "sheet":
        stack = stack.at(-1)?.name === "other" ? stack : [...stack, OTHER];
        break;
      default:
        stack = [ROOT];
    }
    check(stack, step);
  }
}

/** A fixed seed, so a run is the same on every machine. */
const RUNS = { seed: 20261026, numRuns: 300 } as const;

describe("the invariants of the history rules", () => {
  it("keeps the root at index 0", () => {
    fc.assert(
      fc.property(fc.array(stepArbitrary, { maxLength: 60 }), (steps) => {
        walk(steps, (stack) => {
          expect(stack[0]).toEqual(ROOT);
        });
      }),
      RUNS,
    );
  });

  it("keeps at most 8 conversations above the root", () => {
    fc.assert(
      fc.property(fc.array(stepArbitrary, { maxLength: 60 }), (steps) => {
        walk(steps, (stack) => {
          expect(stack.filter((route) => route.name === "c").length).toBeLessThanOrEqual(
            MAX_STACKED,
          );
        });
      }),
      RUNS,
    );
  });

  it("ends with the target on top", () => {
    fc.assert(
      fc.property(fc.array(stepArbitrary, { maxLength: 60 }), (steps) => {
        const opened: { readonly top: StackRoute | undefined; readonly target: StackRoute }[] = [];
        walk(steps, (stack, step) => {
          if (step.kind === "open") {
            opened.push({ top: stack.at(-1), target: c(step.target.machine, step.target.id) });
          }
        });
        expect(opened.map((one) => one.top)).toEqual(opened.map((one) => one.target));
      }),
      RUNS,
    );
  });

  it("never holds a conversation twice", () => {
    fc.assert(
      fc.property(fc.array(stepArbitrary, { maxLength: 60 }), (steps) => {
        walk(steps, (stack) => {
          const keys = stack.map(key).filter((value) => value !== undefined);
          expect(new Set(keys).size).toBe(keys.length);
        });
      }),
      RUNS,
    );
  });

  it("keeps the order of the conversations that stay", () => {
    fc.assert(
      fc.property(fc.array(stepArbitrary, { maxLength: 60 }), (steps) => {
        let before: string[] = [];
        const orders: { readonly kept: string[]; readonly after: string[] }[] = [];
        walk(steps, (stack) => {
          const after = stack.map(key).filter((value) => value !== undefined);
          const kept = before.filter((value) => after.includes(value));
          orders.push({ kept, after: after.slice(0, kept.length) });
          before = after;
        });
        expect(orders.map((order) => order.after)).toEqual(orders.map((order) => order.kept));
      }),
      RUNS,
    );
  });
});
