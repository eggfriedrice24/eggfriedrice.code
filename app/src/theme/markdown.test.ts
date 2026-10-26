import { describe, expect, it } from "vitest";

import { MARKDOWN_STYLE, SYNTAX_COLORS } from "./markdown";
import { efr, opencode, semantic, treesitter } from "./palette";

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

const PALETTE = new Set(strings([semantic, opencode, treesitter, efr]));

/** Every string value of `value` whose key names a colour, with its path. */
function colours(value: unknown, path = ""): [string, string][] {
  if (value === null || typeof value !== "object") {
    return [];
  }
  const found: [string, string][] = [];
  for (const [key, child] of Object.entries(value)) {
    const at = path === "" ? key : `${path}.${key}`;
    const isColourKey = /colou?r$/i.test(key) || path.endsWith("syntaxColors");
    if (typeof child === "string" && isColourKey) {
      found.push([at, child]);
    } else {
      found.push(...colours(child, at));
    }
  }
  return found;
}

describe("the Markdown style", () => {
  it("takes every colour from the design system", () => {
    const all = colours(MARKDOWN_STYLE);
    expect(all.length).toBeGreaterThan(20);
    expect(all.filter(([, colour]) => !PALETTE.has(colour))).toEqual([]);
  });

  it("colours 13 of the 14 code tokens and leaves `embedded` in the code colour", () => {
    expect(Object.keys(SYNTAX_COLORS).sort()).toEqual(
      [
        "attribute",
        "comment",
        "constant",
        "function",
        "keyword",
        "number",
        "operator",
        "property",
        "punctuation",
        "string",
        "tag",
        "type",
        "variable",
      ].sort(),
    );
  });
});
