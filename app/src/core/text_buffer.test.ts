import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { applyDelta, bufferOf, EMPTY_BUFFER, MAX_PENDING_RUNS, seed } from "./text_buffer";
import { utf8Length } from "./utf8";
import { chunked, type Chunk } from "./testing";

const anyText = fc.string({ unit: "binary", maxLength: 200 });

describe("applyDelta", () => {
  it("appends at the end", () => {
    const buffer = applyDelta(applyDelta(EMPTY_BUFFER, 0, "héllo"), 6, " wörld");
    expect(buffer).toEqual({ text: "héllo wörld", bytes: 13, pending: [] });
  });

  it("trims an overlap by bytes", () => {
    const buffer = applyDelta(bufferOf("ab😀"), 2, "😀c");
    expect(buffer.text).toBe("ab😀c");
    expect(buffer.bytes).toBe(7);
  });

  it("returns the same buffer for a duplicate or an empty delta", () => {
    const buffer = bufferOf("abc");
    expect(applyDelta(buffer, 1, "bc")).toBe(buffer);
    expect(applyDelta(buffer, 3, "")).toBe(buffer);
  });

  it("drops an overlap that ends inside a character", () => {
    const buffer = bufferOf("a€");
    expect(applyDelta(buffer, 2, "€Z")).toBe(buffer);
  });

  it("keeps a gap pending until the gap fills", () => {
    const gap = applyDelta(bufferOf("ab"), 4, "ef");
    expect(gap.text).toBe("ab");
    expect(gap.pending).toEqual([{ offset: 4, text: "ef", bytes: 2 }]);
    const filled = applyDelta(gap, 2, "cd");
    expect(filled).toEqual({ text: "abcdef", bytes: 6, pending: [] });
  });

  it("merges pending runs that touch", () => {
    const buffer = applyDelta(applyDelta(EMPTY_BUFFER, 3, "de"), 5, "fg");
    expect(buffer.pending).toEqual([{ offset: 3, text: "defg", bytes: 4 }]);
  });

  it("keeps at most MAX_PENDING_RUNS runs, the ones with the smallest offsets", () => {
    let buffer = EMPTY_BUFFER;
    for (let run = MAX_PENDING_RUNS + 4; run >= 1; run--) {
      buffer = applyDelta(buffer, run * 10, "x");
    }
    expect(buffer.pending.length).toBe(MAX_PENDING_RUNS);
    expect(buffer.pending[0]?.offset).toBe(10);
  });
});

describe("seed", () => {
  it("takes a longer text and drains the pending runs it reaches", () => {
    const gap = applyDelta(bufferOf("ab"), 4, "ef");
    expect(seed(gap, "abcd")).toEqual({ text: "abcdef", bytes: 6, pending: [] });
  });

  it("ignores a text that is not longer", () => {
    const buffer = bufferOf("abcd");
    expect(seed(buffer, "ab")).toBe(buffer);
  });
});

describe("properties", () => {
  it("any chunking in order gives the whole text with exact byte counts", () => {
    fc.assert(
      fc.property(anyText, fc.array(fc.nat(), { maxLength: 30 }), (text, cuts) => {
        let buffer = EMPTY_BUFFER;
        for (const chunk of chunked(text, cuts)) {
          buffer = applyDelta(buffer, chunk.offset, chunk.delta);
          expect(text.startsWith(buffer.text)).toBe(true);
          expect(buffer.bytes).toBe(utf8Length(buffer.text));
        }
        expect(buffer.text).toBe(text);
      }),
    );
  });

  it("any order of the chunks gives the whole text", () => {
    fc.assert(
      fc.property(
        anyText,
        fc.array(fc.nat(), { maxLength: MAX_PENDING_RUNS - 1 }),
        fc.nat(),
        (text, cuts, seedValue) => {
          const chunks = shuffle(chunked(text, cuts), seedValue);
          let buffer = EMPTY_BUFFER;
          for (const chunk of chunks) {
            buffer = applyDelta(buffer, chunk.offset, chunk.delta);
            expect(text.startsWith(buffer.text)).toBe(true);
          }
          expect(buffer.text).toBe(text);
          expect(buffer.pending).toEqual([]);
        },
      ),
    );
  });

  it("overlapping resends and drops never break the prefix rule", () => {
    fc.assert(
      fc.property(
        anyText,
        fc.array(fc.nat(), { maxLength: 30 }),
        fc.array(fc.boolean(), { maxLength: 30 }),
        (text, cuts, keep) => {
          let buffer = EMPTY_BUFFER;
          const chunks = chunked(text, cuts);
          chunks.forEach((chunk, i) => {
            if (keep[i] === false) {
              return;
            }
            // Each kept chunk also comes again from the start of the previous chunk.
            const previous = chunks[i - 1];
            if (previous !== undefined) {
              buffer = applyDelta(buffer, previous.offset, previous.delta + chunk.delta);
            }
            buffer = applyDelta(buffer, chunk.offset, chunk.delta);
            expect(text.startsWith(buffer.text)).toBe(true);
            expect(buffer.bytes).toBe(utf8Length(buffer.text));
          });
          // The completed text repairs any gap.
          expect(seed(buffer, text).text).toBe(text);
        },
      ),
    );
  });
});

function shuffle(chunks: readonly Chunk[], seedValue: number): Chunk[] {
  const copy = [...chunks];
  let state = seedValue + 1;
  for (let i = copy.length - 1; i > 0; i--) {
    state = (state * 1_103_515_245 + 12_345) % 2_147_483_648;
    const j = state % (i + 1);
    const a = copy[i];
    const b = copy[j];
    if (a !== undefined && b !== undefined) {
      copy[i] = b;
      copy[j] = a;
    }
  }
  return copy;
}
