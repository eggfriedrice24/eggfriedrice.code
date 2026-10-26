// The paragraphs of a plain text, such as your message or the reasoning of a thought:
// the parts between blank lines. Each one has the offset where it begins as its key, so
// a view keeps its paragraphs while the text grows.

/** One paragraph of a text. */
export interface Paragraph {
  /** Where the paragraph begins in the text. */
  readonly key: number;
  readonly text: string;
}

/** The paragraphs of `text`, without the blank lines between them. */
export function paragraphs(text: string): Paragraph[] {
  const found: Paragraph[] = [];
  const blank = /\n[ \t]*\n\s*/g;
  let start = 0;
  for (const match of text.matchAll(blank)) {
    push(found, text, start, match.index);
    start = match.index + match[0].length;
  }
  push(found, text, start, text.length);
  return found;
}

function push(found: Paragraph[], text: string, start: number, end: number): void {
  const part = text.slice(start, end);
  if (part.trim() !== "") {
    found.push({ key: start, text: part });
  }
}
