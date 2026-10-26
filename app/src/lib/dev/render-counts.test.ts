import { describe, expect, it } from "vitest";

import { commitsPerSecond, createRenderCounts } from "./render-counts";

describe("the render counter", () => {
  it("counts the commits, the mounts and the time of each part", () => {
    const counts = createRenderCounts();
    counts.record("transcript", "mount", 4, 1000);
    counts.record("transcript", "update", 1.5, 1016);
    counts.record("transcript", "nested-update", 2.5, 1032);
    counts.record("composer", "mount", 1, 1000);
    expect(counts.snapshot()).toEqual([
      [
        "composer",
        { commits: 1, mounts: 1, lastMs: 1, totalMs: 1, maxMs: 1, since: 1000, lastAt: 1000 },
      ],
      [
        "transcript",
        { commits: 3, mounts: 1, lastMs: 2.5, totalMs: 8, maxMs: 4, since: 1000, lastAt: 1032 },
      ],
    ]);
  });

  it("gives the same snapshot until the next record", () => {
    const counts = createRenderCounts();
    counts.record("transcript", "mount", 4, 1000);
    const first = counts.snapshot();
    expect(counts.snapshot()).toBe(first);
    counts.record("transcript", "update", 1, 1016);
    expect(counts.snapshot()).not.toBe(first);
  });

  it("starts again after a reset", () => {
    const counts = createRenderCounts();
    counts.record("transcript", "mount", 4, 1000);
    counts.reset();
    expect(counts.snapshot()).toEqual([]);
    counts.record("transcript", "update", 2, 5000);
    expect(counts.snapshot()[0]?.[1].since).toBe(5000);
  });

  it("gives the commits per second from the first commit to the last", () => {
    const sample = { commits: 31, mounts: 1, lastMs: 1, totalMs: 31, maxMs: 1, since: 1000 };
    expect(commitsPerSecond({ ...sample, lastAt: 3000 })).toBe(15);
    expect(commitsPerSecond({ ...sample, lastAt: 1000 })).toBe(0);
  });
});
