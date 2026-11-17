import { describe, expect, it } from "vitest";

import { holdsEveryWord, matchRuns, searchWords } from "./search-words";

/** The runs of `text` as strings, with the matches in brackets. */
function marked(text: string, query: string): string {
  return matchRuns(text, searchWords(query))
    .map((run) => (run.match ? `[${run.text}]` : run.text))
    .join("");
}

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

describe("the runs of a match", () => {
  it("marks each place of each word, in any case", () => {
    expect(marked("Steer and queue frames for steer", "STEER")).toBe(
      "[Steer] and queue frames for [steer]",
    );
    expect(marked("Fix steer race in turn loop", "turn steer")).toBe(
      "Fix [steer] race in [turn] loop",
    );
  });

  it("joins words that touch or overlap into one run", () => {
    expect(marked("steering", "steer ring")).toBe("[steering]");
    expect(marked("aaa", "aa")).toBe("[aaa]");
  });

  it("gives the start of each run, and one plain run without a word", () => {
    expect(matchRuns("Fix steer", ["steer"])).toEqual([
      { text: "Fix ", start: 0, match: false },
      { text: "steer", start: 4, match: true },
    ]);
    expect(matchRuns("Why is the fan loud", [])).toEqual([
      { text: "Why is the fan loud", start: 0, match: false },
    ]);
    expect(matchRuns("", ["x"])).toEqual([]);
    expect(matchRuns("ab", [""])).toEqual([{ text: "ab", start: 0, match: false }]);
  });

  it("marks nothing when the lower case changes the length", () => {
    expect(marked("İstanbul trip", "trip")).toBe("İstanbul trip");
  });
});
