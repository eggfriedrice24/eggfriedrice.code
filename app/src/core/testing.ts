// Helpers for the tests of the core. Only test files import this module.

import { utf8Length } from "./utf8";

/** One delta of a text: `delta` at UTF-8 byte `offset`. */
export interface Chunk {
  readonly offset: number;
  readonly delta: string;
}

/**
 * Splits `text` into deltas at code point boundaries. Each value of `cuts` picks one cut
 * point; equal cuts give one cut. The deltas are in order and together give `text`.
 */
export function chunked(text: string, cuts: readonly number[]): Chunk[] {
  const points = Array.from(text);
  const at = [...new Set(cuts.map((cut) => cut % (points.length + 1)))].sort((a, b) => a - b);
  const bounds = [0, ...at.filter((cut) => cut > 0 && cut < points.length), points.length];
  const chunks: Chunk[] = [];
  let offset = 0;
  for (let i = 1; i < bounds.length; i++) {
    const delta = points.slice(bounds[i - 1], bounds[i]).join("");
    if (delta.length > 0) {
      chunks.push({ offset, delta });
      offset += utf8Length(delta);
    }
  }
  return chunks;
}
