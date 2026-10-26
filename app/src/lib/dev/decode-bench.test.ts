import { describe, expect, it } from "vitest";

import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import {
  ALL_ROW,
  BENCH_MODES,
  DRAFT_ROW,
  benchItems,
  groupOf,
  quantile,
  runDecodeBench,
  statsOf,
  type BenchDecoder,
} from "./decode-bench";
import { BENCH_DECODERS, zodCompiles } from "./decode-modes";
import { RECORDINGS } from "./recordings";
import { schemaDecoder } from "./schema-decoder";

const ITEMS = benchItems(RECORDINGS);

/** A decoder that refuses every item. */
const refuse: BenchDecoder = () => ({ ok: false });

/** A clock that moves `step` ms on each read, so each parse takes `step`. */
function steppingClock(step: number): () => number {
  let time = 0;
  return () => {
    time += step;
    return time;
  };
}

describe("the items of the bench", () => {
  it("are every line of the 8 recordings, with the kind of each", () => {
    const lines = RECORDINGS.reduce((sum, recording) => sum + recording.lines.length, 0);
    expect(RECORDINGS).toHaveLength(8);
    expect(ITEMS).toHaveLength(lines);
    const groups = new Set(ITEMS.map((item) => item.group));
    expect(groups.has("snapshot")).toBe(true);
    expect(groups.has("draft text")).toBe(true);
    expect(groups.has("event turn_started")).toBe(true);
  });

  it("names the kind of an event, a draft and anything else", () => {
    expect(groupOf({ kind: "event", event: { kind: "turn_completed" } })).toBe(
      "event turn_completed",
    );
    expect(groupOf({ kind: "draft", draft: { kind: "reasoning" } })).toBe("draft reasoning");
    expect(groupOf({ kind: "snapshot" })).toBe("snapshot");
    expect(groupOf(null)).toBe("not an object");
  });
});

describe("the schema decoder of the jitless mode", () => {
  it("decodes every item to the value of the app's decoder, with and without jitless", () => {
    const compiled = schemaDecoder({ jitless: false });
    const jitless = schemaDecoder({ jitless: true });
    for (const item of ITEMS) {
      const expected = decodeSubscribeItem(item.raw);
      expect(expected.ok).toBe(true);
      expect(compiled(item.raw)).toEqual(expected);
      expect(jitless(item.raw)).toEqual(expected);
    }
  });

  it("gives an unknown kind and an invalid known kind as the app's decoder does", () => {
    const decode = schemaDecoder({ jitless: true });
    const cases: unknown[] = [
      { kind: "later" },
      { kind: "draft", turn_id: "x", after_seq: 1, draft: { kind: "text" } },
      {
        kind: "event",
        seq: 1,
        at: "2026-10-26T12:00:00Z",
        event: { kind: "turn_started", turn_id: 5 },
      },
      { kind: "event", seq: -1, at: "2026-10-26T12:00:00Z", event: { kind: "later" } },
      "not an object",
    ];
    for (const raw of cases) {
      expect(decode(raw)).toEqual(decodeSubscribeItem(raw));
    }
  });

  it("decodes every item in each mode of the bench", () => {
    for (const mode of BENCH_MODES) {
      const decode = BENCH_DECODERS[mode];
      expect(ITEMS.every((item) => decode(item.raw).ok)).toBe(true);
    }
  });

  it("says whether zod compiles its parsers, which Node allows", () => {
    expect(zodCompiles()).toBe(true);
  });
});

describe("the numbers of the bench", () => {
  it("takes the quantile of a sorted list", () => {
    const sorted = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    expect(quantile(sorted, 0.5)).toBe(6);
    expect(quantile(sorted, 0.95)).toBe(10);
    expect(quantile([], 0.5)).toBeNaN();
  });

  it("gives the count, the mean, the quantiles and the longest time of a row", () => {
    expect(statsOf("draft text", [3, 1, 2], 9)).toEqual({
      group: "draft text",
      count: 3,
      firstMs: 9,
      meanMs: 2,
      p50Ms: 2,
      p95Ms: 3,
      maxMs: 3,
    });
  });
});

describe("a run of the bench", () => {
  const items = ITEMS.slice(0, 40);
  const draftCount = items.filter((item) => item.group.startsWith("draft")).length;

  it("times each parse per kind, for the drafts and for all items, in each mode", async () => {
    const report = await runDecodeBench(items, {
      decoders: BENCH_DECODERS,
      now: steppingClock(0.25),
      rounds: 3,
    });
    expect(report.items).toBe(40);
    expect(report.rounds).toBe(3);
    expect(report.modes.map((mode) => mode.mode)).toEqual(["zod", "jitless", "hand"]);
    for (const mode of report.modes) {
      expect(mode.refused).toBe(0);
      const all = mode.rows.find((row) => row.group === ALL_ROW);
      expect(all?.count).toBe(120);
      expect(all?.p95Ms).toBeCloseTo(0.25);
      expect(mode.rows.find((row) => row.group === DRAFT_ROW)?.count).toBe(draftCount * 3);
      expect(mode.draftP95Ms).toBeCloseTo(0.25);
      expect(mode.withinBudget).toBe(true);
      expect(mode.rows.at(-2)?.group).toBe(DRAFT_ROW);
      expect(mode.rows.at(-1)?.group).toBe(ALL_ROW);
    }
  });

  it("is over the budget when a draft takes 0.5 ms or more", async () => {
    const report = await runDecodeBench(items, {
      decoders: BENCH_DECODERS,
      now: steppingClock(0.5),
      rounds: 1,
      modes: ["hand"],
    });
    expect(report.modes).toHaveLength(1);
    expect(report.modes[0]?.withinBudget).toBe(false);
  });

  it("counts the items that a decoder refuses", async () => {
    const report = await runDecodeBench(items, {
      decoders: { zod: refuse, jitless: refuse, hand: refuse },
      now: steppingClock(0.1),
      rounds: 2,
      modes: ["zod"],
    });
    expect(report.modes[0]?.refused).toBe(80);
  });

  it("pauses after each chunk and reports the progress", async () => {
    const progress: [number, number][] = [];
    let pauses = 0;
    await runDecodeBench(items, {
      decoders: BENCH_DECODERS,
      now: steppingClock(0.1),
      rounds: 2,
      modes: ["zod", "hand"],
      chunk: 30,
      pause: () => {
        pauses += 1;
        return Promise.resolve();
      },
      onProgress: (done, total) => progress.push([done, total]),
    });
    // 80 parses per mode in chunks of 30: pauses after 30 and 60 in each mode.
    expect(pauses).toBe(4);
    expect(progress).toEqual([
      [30, 160],
      [60, 160],
      [80, 160],
      [110, 160],
      [140, 160],
      [160, 160],
    ]);
  });
});
