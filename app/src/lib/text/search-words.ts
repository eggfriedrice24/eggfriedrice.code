// The word match of the searches of the app (the model sheet, All conversations): a
// search is its words, and a row matches when its texts hold every word, in any case.

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
