// The effects budget (plan section 2.4): a file of src/ calls useEffect or
// useLayoutEffect only as often as scripts/effect-budget.json allows, and a file that
// is not in the budget calls neither. Lifecycle (sockets, AppState, NetInfo, the outbox,
// persistence) lives in the runtime outside React, so most files need none. The budget
// only goes down: a file below its budget fails too, so that the budget follows it.

import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { APP_DIR, isTest, sourceFiles, withoutComments } from "./source-files";

const BUDGET_FILE = path.join(APP_DIR, "scripts", "effect-budget.json");

const EFFECT = /(?:^|[^\w$])use(?:Layout)?Effect\s*\(/g;

/** The number of effect calls in `code` (text without comments). */
function effectCount(code: string): number {
  return [...code.matchAll(EFFECT)].length;
}

/** The budget of each file, from `json`. */
function parseBudget(json: unknown): ReadonlyMap<string, number> {
  if (typeof json !== "object" || json === null || !("files" in json)) {
    throw new Error("the effect budget has no `files` object");
  }
  const files: unknown = json.files;
  if (typeof files !== "object" || files === null) {
    throw new Error("the `files` of the effect budget is not an object");
  }
  const budget = new Map<string, number>();
  for (const [file, value] of Object.entries(files)) {
    if (typeof value !== "number" || !Number.isInteger(value) || value < 1) {
      throw new Error(`the budget of ${file} is not an integer of 1 or more`);
    }
    budget.set(file, value);
  }
  return budget;
}

/** What is wrong with `counts` against `budget`. */
function budgetViolations(
  counts: ReadonlyMap<string, number>,
  budget: ReadonlyMap<string, number>,
): string[] {
  const found: string[] = [];
  for (const [file, count] of counts) {
    const allowed = budget.get(file) ?? 0;
    if (count > allowed) {
      found.push(`${file}: ${count} effects, budget ${allowed}`);
    } else if (count < allowed) {
      found.push(`${file}: ${count} effects, budget ${allowed}: lower the budget`);
    }
  }
  for (const file of budget.keys()) {
    if (!counts.has(file)) {
      found.push(`${file}: in the budget, but not a source file`);
    }
  }
  return found;
}

describe("the effects budget", () => {
  it("holds for every file of src/", () => {
    const counts = new Map<string, number>();
    for (const file of sourceFiles()) {
      if (!isTest(file.path)) {
        counts.set(file.path, effectCount(file.code));
      }
    }
    const budget = parseBudget(JSON.parse(readFileSync(BUDGET_FILE, "utf8")));
    expect(budgetViolations(counts, budget)).toEqual([]);
  });

  it("counts both effect hooks, but not a comment or another name", () => {
    const code = withoutComments(
      [
        "useEffect(() => {}, []);",
        "React.useLayoutEffect(() => {});",
        "// useEffect(() => {});",
        "useEffectEvent(() => {});",
        "myuseEffect();",
      ].join("\n"),
      false,
    );
    expect(effectCount(code)).toBe(2);
  });

  it("fails a file over its budget, under its budget, and a stale entry", () => {
    const counts = new Map([
      ["src/a.tsx", 1],
      ["src/b.tsx", 1],
      ["src/c.tsx", 2],
      ["src/d.tsx", 1],
    ]);
    const budget = new Map([
      ["src/b.tsx", 1],
      ["src/c.tsx", 3],
      ["src/d.tsx", 1],
      ["src/gone.tsx", 1],
    ]);
    expect(budgetViolations(counts, budget)).toEqual([
      "src/a.tsx: 1 effects, budget 0",
      "src/c.tsx: 2 effects, budget 3: lower the budget",
      "src/gone.tsx: in the budget, but not a source file",
    ]);
  });

  it("refuses a budget that is not a map of counts", () => {
    expect(() => parseBudget({})).toThrow("no `files` object");
    expect(() => parseBudget({ files: { "src/a.tsx": 0 } })).toThrow("src/a.tsx");
    expect(parseBudget({ files: { "src/a.tsx": 2 } })).toEqual(new Map([["src/a.tsx", 2]]));
  });
});
