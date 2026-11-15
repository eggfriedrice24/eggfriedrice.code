import { describe, expect, it } from "vitest";

import { holdsEveryWord, searchWords } from "./search-words";

describe("the word match of a search", () => {
  it("splits a search into its words in lower case", () => {
    expect(searchWords("  Opus  5\tFIVE ")).toEqual(["opus", "5", "five"]);
    expect(searchWords(" \n ")).toEqual([]);
  });

  it("matches when the texts hold every word, in any case and order", () => {
    const words = searchWords("deploy LAPTOP");
    expect(holdsEveryWord(words, ["Fix the deploy key", "laptop", "/srv/app"])).toBe(true);
    expect(holdsEveryWord(words, ["Fix the deploy key", "desktop"])).toBe(false);
    expect(holdsEveryWord([], ["anything"])).toBe(true);
  });
});
