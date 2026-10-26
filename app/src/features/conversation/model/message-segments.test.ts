import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { EMPTY_SPLIT, splitBlocks } from "./blocks";
import {
  BLOCKS_FROM,
  codeLabel,
  fenceLanguage,
  messageSegments,
  type Segment,
} from "./message-segments";

function segmentsOf(text: string, final = false): Segment[] {
  return messageSegments(text, splitBlocks(text, EMPTY_SPLIT), final);
}

const REPLY = [
  "Guard it with the turn state:",
  "",
  "```rust",
  "pub fn steer(&self) {}",
  "```",
  "",
  "Then add a test.",
].join("\n");

describe("messageSegments", () => {
  it("gives a short text without fences one Markdown segment", () => {
    expect(segmentsOf("Found it.\n\nThe steer lands late.")).toEqual([
      { kind: "markdown", key: "m0", text: "Found it.\n\nThe steer lands late.", open: true },
    ]);
  });

  it("puts a closed fence in a code segment between Markdown segments", () => {
    const segments = segmentsOf(REPLY, true);
    expect(segments.map((segment) => segment.kind)).toEqual(["markdown", "code", "markdown"]);
    const code = segments[1];
    expect(code).toMatchObject({
      kind: "code",
      language: "rust",
      body: "pub fn steer(&self) {}",
      source: "```rust\npub fn steer(&self) {}\n```",
      closed: true,
    });
    expect(segments[2]).toMatchObject({ text: "Then add a test.", open: false });
  });

  it("shows an open fence as an open code segment, never as raw backticks", () => {
    const segments = segmentsOf("Guard it:\n\n```rust\npub fn steer(");
    expect(segments).toEqual([
      { kind: "markdown", key: "m0", text: "Guard it:", open: false },
      {
        kind: "code",
        key: "c11",
        language: "rust",
        source: "```rust\npub fn steer(",
        body: "pub fn steer(",
        closed: false,
      },
    ]);
  });

  it("holds back a last line that can still become a fence", () => {
    for (const partial of ["``", "```", "```ru", "~~~"]) {
      const segments = segmentsOf(`Guard it:\n${partial}`);
      expect(segments[0]).toMatchObject({ kind: "markdown", text: "Guard it:" });
      expect(segments[1]).toMatchObject({ kind: "code", closed: false, body: "" });
    }
    expect(segmentsOf("Use `x`")).toEqual([
      { kind: "markdown", key: "m0", text: "Use `x`", open: true },
    ]);
  });

  it("keeps the key of an open fence when it closes", () => {
    const open = segmentsOf("Guard it:\n\n```rust\nfn a() {}\n");
    const closed = segmentsOf("Guard it:\n\n```rust\nfn a() {}\n```\n");
    expect(open[1]?.key).toBe(closed[1]?.key);
    expect(closed[1]).toMatchObject({ kind: "code", closed: true });
  });

  it("makes a diff segment of a closed diff fence", () => {
    const segments = segmentsOf("```diff\n-a\n+b\n```\n\nDone.", true);
    expect(segments[0]).toEqual({ kind: "diff", key: "c0", body: "-a\n+b" });
  });

  it("closes an unclosed fence at the end of a final message, so it can be copied", () => {
    const segments = segmentsOf("```sh\nls", true);
    expect(segments).toEqual([
      { kind: "code", key: "c0", language: "sh", source: "```sh\nls", body: "ls", closed: true },
    ]);
  });

  it("gives each final block of a long message its own segment", () => {
    const paragraph = `${"word ".repeat(40)}\n\n`;
    const text = paragraph.repeat(Math.ceil(BLOCKS_FROM / paragraph.length) + 1);
    const segments = segmentsOf(text);
    expect(segments.length).toBeGreaterThan(10);
    expect(segments.every((segment) => segment.kind === "markdown")).toBe(true);
  });

  it("keeps each Markdown segment growing by appends while text streams", () => {
    fc.assert(
      fc.property(fc.integer({ min: 1, max: REPLY.length }), (cut) => {
        const before = segmentsOf(REPLY.slice(0, cut));
        const after = segmentsOf(REPLY.slice(0, Math.min(REPLY.length, cut + 7)));
        const shrunk = before.filter((segment) => {
          const next = after.find((other) => other.key === segment.key);
          return (
            segment.kind === "markdown" &&
            next?.kind === "markdown" &&
            !next.text.startsWith(segment.text)
          );
        });
        expect(shrunk).toEqual([]);
      }),
    );
  });
});

describe("fenceLanguage", () => {
  it("reads the first word of the info", () => {
    expect(fenceLanguage("```Rust title=x")).toBe("rust");
    expect(fenceLanguage("~~~")).toBe("");
    expect(fenceLanguage("text")).toBeUndefined();
    expect(fenceLanguage("``` a`b")).toBeUndefined();
  });
});

describe("codeLabel", () => {
  it("says streaming while the fence is open", () => {
    expect(codeLabel("rust", false)).toBe("rust · streaming");
    expect(codeLabel("rust", true)).toBe("rust");
    expect(codeLabel("", true)).toBe("code");
  });
});

describe("the segment cache", () => {
  it("gives the same object for a final segment on the next call", () => {
    const cache = new Map<string, Segment>();
    const text = "Intro\n\n```rust\nfn a() {}\n```\n\nMore";
    const first = messageSegments(text, splitBlocks(text, EMPTY_SPLIT), false, cache);
    const longer = `${text} text`;
    const second = messageSegments(longer, splitBlocks(longer, EMPTY_SPLIT), false, cache);
    expect(second[0]).toBe(first[0]);
    expect(second[1]).toBe(first[1]);
    expect(second[2]).not.toBe(first[2]);
  });

  it("keeps the segment of a fence that closed in the tail when it becomes final", () => {
    const cache = new Map<string, Segment>();
    const closed = "Guard it:\n\n```rust\nfn a() {}\n```\n";
    const inTail = messageSegments(closed, splitBlocks(closed, EMPTY_SPLIT), false, cache);
    const later = `${closed}\nThen test it.`;
    const asFinal = messageSegments(later, splitBlocks(later, EMPTY_SPLIT), false, cache);
    expect(inTail[1]).toMatchObject({ kind: "code", closed: true });
    expect(asFinal[1]).toBe(inTail[1]);
  });
});
