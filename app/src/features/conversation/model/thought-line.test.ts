import { describe, expect, it } from "vitest";

import { thoughtExcerpt, thoughtLine, type ThoughtLineInput } from "./thought-line";

const BASE: ThoughtLineInput = {
  title: undefined,
  text: "",
  live: false,
  expanded: false,
  startedAt: 0,
  endedAt: undefined,
  now: 0,
};

const LONG =
  "The steer arrives on its own channel, so if the turn loop checks it only between tool calls";

describe("thoughtLine", () => {
  it("says thinking with the time since the start while live", () => {
    const line = thoughtLine({ ...BASE, title: "Planning the migration", live: true, now: 12_000 });
    expect(line).toMatchObject({ word: "thinking", title: "Planning the migration", time: "12s" });
    expect(line.label).toBe("thinking, Planning the migration, 12s");
  });

  it("says thought and how long the first thought took after", () => {
    const line = thoughtLine({ ...BASE, title: "Planning the migration", endedAt: 9_000 });
    expect(line).toMatchObject({ word: "thought", time: "9s", excerpt: undefined });
  });

  it("takes the first words without a bold title, and drops them when expanded", () => {
    expect(thoughtLine({ ...BASE, text: LONG }).excerpt).toBe(LONG);
    expect(thoughtLine({ ...BASE, text: LONG, expanded: true }).excerpt).toBeUndefined();
    expect(thoughtLine({ ...BASE, text: LONG, title: "A title" }).excerpt).toBeUndefined();
  });

  it("has no time under 2 s or when the end is not known", () => {
    expect(thoughtLine({ ...BASE, live: true, now: 1_500 }).time).toBeUndefined();
    expect(thoughtLine(BASE).time).toBeUndefined();
    expect(thoughtLine(BASE).label).toBe("thought");
  });
});

describe("thoughtExcerpt", () => {
  it("waits for ~8 words while live, so the line does not jitter", () => {
    expect(thoughtExcerpt("Checking 97 first", true)).toBeUndefined();
    expect(thoughtExcerpt("Checking 97 first", false)).toBe("Checking 97 first");
    expect(thoughtExcerpt("Checking 97 first, then 91 and 221 by", true)).toBe(
      "Checking 97 first, then 91 and 221 by",
    );
  });

  it("takes the Markdown marks out and joins the lines", () => {
    expect(thoughtExcerpt("## Plan\n- read **turn.rs**\n- run `cargo test`", false)).toBe(
      "Plan read turn.rs run cargo test",
    );
  });

  it("keeps at most 24 words", () => {
    const words = Array.from({ length: 30 }, (_, i) => `w${String(i)}`);
    expect(thoughtExcerpt(words.join(" "), false)?.split(" ")).toHaveLength(24);
  });

  it("has nothing for an empty text", () => {
    expect(thoughtExcerpt("", false)).toBeUndefined();
    expect(thoughtExcerpt("  \n ", true)).toBeUndefined();
  });
});
