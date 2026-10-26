import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { completeBlocks, fenceBody, splitBlocks, type Block } from "./blocks";

/** A fixed seed, so each run checks the same cases and a failure repeats. */
const SEED = 20261026;

const texts = (text: string, blocks: readonly Block[]) =>
  blocks.map((block) => [block.kind, text.slice(block.start, block.end)]);

/** Pieces of Markdown that a model writes, joined in any order. */
const FRAGMENTS = [
  "Some text here.\n",
  "\n",
  "\n\n",
  "- item one\n",
  "  continued item\n",
  "1. first\n",
  "> a quote\n",
  "```rust\nfn main() {}\n\nlet x = 1;\n```\n",
  "```diff\n@@ -1 +1 @@\n-old\n+new\n```\n",
  "~~~\nraw ~~~ text\n~~~\n",
  "| a | b |\n|---|---|\n| 1 | 2 |\n",
  "# Heading\n",
  "Plain 日本語 text 🙂 with **bold**",
  "    indented code\n",
  "```\nunclosed fence\n",
  "`inline` and ``` not a fence ``` in text\n",
];

const documents = fc
  .array(fc.constantFrom(...FRAGMENTS), { maxLength: 14 })
  .map((parts) => parts.join(""));

describe("splitBlocks", () => {
  it("ends a block at a blank line once the next block begins", () => {
    const text = "First para.\n\nSecond para.\n\nThird";
    const split = splitBlocks(text);
    expect(texts(text, split.blocks)).toEqual([
      ["markdown", "First para.\n\n"],
      ["markdown", "Second para.\n\n"],
    ]);
    expect(text.slice(split.tailStart)).toBe("Third");
  });

  it("keeps a block open while only blank lines follow it", () => {
    const split = splitBlocks("First para.\n\n\n");
    expect(split.blocks).toEqual([]);
    expect(split.tailStart).toBe(0);
  });

  it("keeps the blank lines of a fence inside its block", () => {
    const text = "```rust\nfn a() {}\n\nfn b() {}\n```\n\nAfter.\n";
    expect(texts(text, splitBlocks(text).blocks)).toEqual([
      ["markdown", "```rust\nfn a() {}\n\nfn b() {}\n```\n\n"],
    ]);
  });

  it("begins a block at a fence after a paragraph line, and after the closing fence", () => {
    const text = "Look:\n```diff\n-a\n+b\n```\nDone.\n";
    expect(texts(text, splitBlocks(text).blocks)).toEqual([
      ["markdown", "Look:\n"],
      ["diff", "```diff\n-a\n+b\n```\n"],
    ]);
  });

  it("continues a list item that goes on after a blank line", () => {
    const text = "- one\n\n  more of one\n- two\n\nNext.\n";
    expect(texts(text, splitBlocks(text).blocks)).toEqual([
      ["markdown", "- one\n\n  more of one\n- two\n\n"],
    ]);
  });

  it("opens a fence only on a whole line", () => {
    const text = "Para.\n```d";
    expect(splitBlocks(text).blocks).toEqual([]);
  });

  it("never changes a final block, whatever comes after it", () => {
    fc.assert(
      fc.property(documents, fc.nat(), (text, cut) => {
        const prefix = text.slice(0, cut % (text.length + 1));
        const whole = splitBlocks(text).blocks;
        const part = splitBlocks(prefix).blocks;
        expect(whole.slice(0, part.length)).toEqual(part);
      }),
      { seed: SEED },
    );
  });

  it("gives the same split when it continues an earlier split", () => {
    fc.assert(
      fc.property(documents, fc.array(fc.nat(), { maxLength: 8 }), (text, cuts) => {
        const points = [...cuts.map((cut) => cut % (text.length + 1)), text.length].sort(
          (a, b) => a - b,
        );
        let split = splitBlocks("");
        for (const point of points) {
          split = splitBlocks(text.slice(0, point), split);
        }
        expect(split).toEqual(splitBlocks(text));
      }),
      { seed: SEED },
    );
  });

  it("covers the text with blocks that touch each other", () => {
    fc.assert(
      fc.property(documents, (text) => {
        const split = splitBlocks(text);
        let at = 0;
        for (const block of split.blocks) {
          expect(block.start).toBe(at);
          expect(block.end).toBeGreaterThan(block.start);
          at = block.end;
        }
        expect(split.tailStart).toBe(at);
        const all = completeBlocks(text);
        expect(all.map((block) => text.slice(block.start, block.end)).join("")).toBe(text);
      }),
      { seed: SEED },
    );
  });

  it("starts over when the text does not continue the earlier one", () => {
    const first = splitBlocks("A.\n\nB.\n\nC");
    expect(splitBlocks("X.\n\nY", first)).toEqual(splitBlocks("X.\n\nY"));
  });
});

describe("completeBlocks and fenceBody", () => {
  it("makes the tail a block and finds a diff that ends the text", () => {
    const text = "Intro.\n\n```diff\n@@ -1 +1 @@\n-a\n+b\n```";
    const blocks = completeBlocks(text);
    expect(texts(text, blocks)).toEqual([
      ["markdown", "Intro.\n\n"],
      ["diff", "```diff\n@@ -1 +1 @@\n-a\n+b\n```"],
    ]);
    const diff = blocks[1];
    expect(diff === undefined ? "" : fenceBody(text.slice(diff.start, diff.end))).toBe(
      "@@ -1 +1 @@\n-a\n+b",
    );
  });

  it("does not take an unclosed diff fence for a diff", () => {
    const text = "```diff\n-a\n";
    expect(completeBlocks(text).map((block) => block.kind)).toEqual(["markdown"]);
  });
});
