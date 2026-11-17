// The word match of the searches of the app (the model sheet, All conversations): a
// search is its words, and a row matches when its texts hold every word, in any case.
// The title of a result shows where the words are (`matchRuns`).

/** The words of a search, in lower case. No word for a blank search. */
export function searchWords(query: string): string[] {
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter((word) => word !== "");
}

/** True when the texts together hold every word of `words` (from `searchWords`). */
export function holdsEveryWord(words: readonly string[], texts: readonly string[]): boolean {
  // NOTE: a word holds no white space, so it never spans two texts.
  const text = texts.join("\n").toLowerCase();
  return words.every((word) => text.includes(word));
}

/** A part of a text, and whether it holds a word of the search. */
export interface MatchRun {
  readonly text: string;
  /** The UTF-16 index of the run in the text, a stable key for a list of runs. */
  readonly start: number;
  readonly match: boolean;
}

/**
 * `text` cut into runs: the parts that hold a word of `words` (from `searchWords`), in
 * any case, and the parts between them. The runs join to `text`. When the lower case of
 * the text has another length (a rare letter), no run is a match, so an index never
 * points into the wrong letter.
 */
export function matchRuns(text: string, words: readonly string[]): MatchRun[] {
  const lower = text.toLowerCase();
  const matched = Array.from({ length: text.length }, () => false);
  if (lower.length === text.length) {
    // NOTE: an empty word would match at every index, and `indexOf` never passes the end.
    for (const word of words.filter((held) => held !== "")) {
      for (let at = lower.indexOf(word); at >= 0; at = lower.indexOf(word, at + 1)) {
        matched.fill(true, at, at + word.length);
      }
    }
  }
  const runs: MatchRun[] = [];
  let start = 0;
  for (let index = 1; index <= text.length; index += 1) {
    if (index === text.length || matched[index] !== matched[start]) {
      runs.push({ text: text.slice(start, index), start, match: matched[start] === true });
      start = index;
    }
  }
  return runs;
}
