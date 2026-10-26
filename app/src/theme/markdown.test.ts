import { describe, expect, it } from "vitest";

import { CODE_BLOCK_FONT, FENCE_MARKDOWN_STYLE, MARKDOWN_STYLE, SYNTAX_COLORS } from "./markdown";
import { efr, opencode, semantic, treesitter } from "./palette";
import { NO_FILL } from "./roles";
import { MIN_TEXT_SIZE, TYPE } from "./type";

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

/** Every leaf value of `value` that `pick` takes by its key and path, with its path. */
function leaves(
  value: unknown,
  pick: (key: string, path: string) => boolean,
  path = "",
): [string, unknown][] {
  if (value === null || typeof value !== "object") {
    return [];
  }
  const found: [string, unknown][] = [];
  for (const [name, child] of Object.entries(value)) {
    const at = path === "" ? name : `${path}.${name}`;
    if (typeof child !== "object" && pick(name, path)) {
      found.push([at, child]);
    } else {
      found.push(...leaves(child, pick, at));
    }
  }
  return found;
}

const colours = (style: unknown) =>
  leaves(
    style,
    (key, path) =>
      /colou?r$/i.test(key) || path.endsWith("syntaxColors") || path.includes("admonitions"),
  );

describe("the Markdown style", () => {
  it("takes every colour from the design system, or sets no fill", () => {
    const all = colours(MARKDOWN_STYLE);
    expect(all.length).toBeGreaterThan(40);
    expect(
      all.filter(
        ([, colour]) => typeof colour !== "string" || (!PALETTE.has(colour) && colour !== NO_FILL),
      ),
    ).toEqual([]);
  });

  it("sets each fill that enriched draws by default, so no light default shows", () => {
    const paths = new Set(colours(MARKDOWN_STYLE).map(([path]) => path));
    for (const path of [
      "link.backgroundColor",
      "underline.color",
      "code.backgroundColor",
      "code.borderColor",
      "codeBlock.backgroundColor",
      "blockquote.backgroundColor",
      "table.headerBackgroundColor",
      "table.rowEvenBackgroundColor",
      "table.rowOddBackgroundColor",
      "taskList.checkedTextColor",
      "highlight.backgroundColor",
      "spoiler.color",
      "video.backgroundColor",
      "math.backgroundColor",
      "inlineMath.color",
      "blockquote.admonitions.caution.color",
    ]) {
      expect(paths).toContain(path);
    }
  });

  it("has no text under 11 pt", () => {
    const sizes = leaves(MARKDOWN_STYLE, (key) => key === "fontSize");
    expect(sizes.length).toBeGreaterThan(8);
    expect(sizes.filter(([, size]) => typeof size !== "number" || size < MIN_TEXT_SIZE)).toEqual(
      [],
    );
  });

  it("follows the type scale of the design", () => {
    expect(MARKDOWN_STYLE.paragraph).toMatchObject(TYPE.body);
    expect(MARKDOWN_STYLE.h1).toMatchObject({ ...TYPE.heading1, color: "#e06c75" });
    expect(MARKDOWN_STYLE.h2).toMatchObject(TYPE.heading2);
    expect(MARKDOWN_STYLE.h3).toMatchObject(TYPE.heading3);
    expect(MARKDOWN_STYLE.code).toMatchObject({ fontSize: 12.5, color: "#60e654" });
    expect(MARKDOWN_STYLE.codeBlock).toMatchObject({
      fontSize: 12,
      lineHeight: 18,
      fontFamily: CODE_BLOCK_FONT,
      color: "#d8d3c3",
    });
    expect(MARKDOWN_STYLE.table).toMatchObject({ fontSize: 12.5, lineHeight: 17 });
  });

  it("names no font for inline code, so enriched takes SF Mono", () => {
    expect(MARKDOWN_STYLE.code).not.toHaveProperty("fontFamily");
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

describe("the fence style", () => {
  it("draws no box, because our code block frame draws it", () => {
    expect(FENCE_MARKDOWN_STYLE.codeBlock).toMatchObject({
      borderWidth: 0,
      padding: 0,
      marginBottom: 0,
      backgroundColor: NO_FILL,
    });
  });

  it("keeps the rest of the Markdown style", () => {
    expect(FENCE_MARKDOWN_STYLE.paragraph).toBe(MARKDOWN_STYLE.paragraph);
    expect(FENCE_MARKDOWN_STYLE.codeBlock?.syntaxColors).toBe(SYNTAX_COLORS);
  });
});
