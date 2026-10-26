import { describe, expect, it } from "vitest";

import { paragraphs } from "./paragraphs";

describe("paragraphs", () => {
  it("splits at blank lines and keys each part by its offset", () => {
    expect(paragraphs("Two things.\n\nFirst, keep it.\n \nSecond, test.")).toEqual([
      { key: 0, text: "Two things." },
      { key: 13, text: "First, keep it." },
      { key: 31, text: "Second, test." },
    ]);
  });

  it("keeps a single line break inside a paragraph", () => {
    expect(paragraphs("a\nb")).toEqual([{ key: 0, text: "a\nb" }]);
  });

  it("gives nothing for an empty or blank text", () => {
    expect(paragraphs("")).toEqual([]);
    expect(paragraphs("\n\n  \n")).toEqual([]);
  });

  it("keeps the keys of the first paragraphs while the text grows", () => {
    const before = paragraphs("one\n\ntwo");
    const after = paragraphs("one\n\ntwo\n\nthree");
    expect(after.slice(0, 2).map((part) => part.key)).toEqual(before.map((part) => part.key));
  });
});
