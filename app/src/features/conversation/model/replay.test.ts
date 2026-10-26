/// <reference types="vite/client" />
// Replay tests of the transcript: the scripted streams of scenarios.ts and the
// recordings of efrd's TestDaemon (test/recordings/*.ndjson), with faults on top. Any
// chunking and any drop pattern must end with the completed text, the shown text must
// never move back, and a final block must never change.

import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import {
  type Scenario,
  scenarios,
  type TimedItem,
  toolAndApproval,
} from "@/features/demo/model/scenarios";
import { pendingApprovals } from "@/lib/stream/reducer";
import {
  dropDrafts,
  duplicateEvents,
  FRAME_MS,
  reconnectWithSnapshot,
  replay,
  type ReplayResult,
  resubscribeAfterSeq,
  seeded,
} from "./testing";

/** A fixed seed, so each run checks the same cases and a failure repeats. */
const SEED = 20261026;

const SCENARIOS = scenarios();

/** The recordings of efrd's TestDaemon, as text, by path. */
const RECORDINGS = import.meta.glob<string>("../../../../test/recordings/*.ndjson", {
  eager: true,
  query: "?raw",
  import: "default",
});

type Json = Record<string, unknown>;

function isJson(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * What is wrong with a replay that must end with `texts`: the problems of its frames,
 * then each message whose text or shown text is not its completed text.
 */
function problems(result: ReplayResult, texts: ReadonlyMap<string, string>): string[] {
  const found = [...result.problems];
  for (const [key, text] of texts) {
    const [turnId, index] = key.split(":");
    const held = result.stream.turns.get(turnId ?? "")?.messages.get(Number(index))?.buffer.text;
    if (held !== text) {
      found.push(`${key} holds ${JSON.stringify(held)}`);
    }
    if (result.shown.get(key) !== text) {
      found.push(`${key} shows ${JSON.stringify(result.shown.get(key))}`);
    }
  }
  return found;
}

/** An event item's sequence number and body, or `undefined` for another item. */
function eventOf(item: unknown): { seq: number; event: Json } | undefined {
  if (!isJson(item) || item["kind"] !== "event" || !isJson(item["event"])) {
    return undefined;
  }
  return { seq: typeof item["seq"] === "number" ? item["seq"] : 0, event: item["event"] };
}

/**
 * The completed texts that a phone holds after `reconnectWithSnapshot(cut, back, window)`
 * without paging `conversation.history`: a message whose completion came before the cut,
 * inside the snapshot's window, or after the reconnect. Older text needs a page of
 * history, which the store fetches.
 */
function reachable(s: Scenario, cut: number, back: number, window: number): Map<string, string> {
  const seqs = s.items.slice(0, back).flatMap(({ item }) => {
    const found = eventOf(item);
    return found === undefined ? [] : [found.seq];
  });
  const firstInWindow = seqs.slice(-window)[0] ?? Number.POSITIVE_INFINITY;
  const out = new Map<string, string>();
  s.items.forEach(({ item }, index) => {
    const found = eventOf(item);
    if (found?.event["kind"] !== "assistant_message_completed") {
      return;
    }
    const key = `${String(found.event["turn_id"])}:${String(found.event["index"])}`;
    const text = s.texts.get(key);
    if (text !== undefined && (index < cut || index >= back || found.seq >= firstInWindow)) {
      out.set(key, text);
    }
  });
  return out;
}

describe("the scripted streams", () => {
  it.each(SCENARIOS.map((scenario) => [scenario.name, scenario] as const))(
    "end with the completed text: %s",
    (_name, scenario) => {
      expect(problems(replay(scenario.items), scenario.texts)).toEqual([]);
    },
  );

  it("pace the text, and show every frame at once with Reduce Motion on", () => {
    for (const scenario of SCENARIOS) {
      const paced = replay(scenario.items);
      const still = replay(scenario.items, true);
      expect({
        name: scenario.name,
        paced: paced.pacedFrames > 0,
        still: still.pacedFrames,
        problems: problems(still, scenario.texts),
      }).toEqual({ name: scenario.name, paced: true, still: 0, problems: [] });
    }
  });

  it("hold the approval, its answer and the end of the turn", () => {
    const result = replay(toolAndApproval("deny").items);
    const turn = [...result.stream.turns.values()].at(-1);
    const approval = [...(turn?.approvals.values() ?? [])][0];
    expect(approval).toMatchObject({
      status: "denied",
      decision: "deny",
      answeredBy: "phone",
      interactive: true,
    });
    expect(approval?.expiresAt).toBe((approval?.askedAt ?? 0) + 120_000);
    expect(turn?.end).toMatchObject({ outcome: "completed", context: { tokens: 101_200 } });
    expect(pendingApprovals(result.stream)).toEqual([]);
    expect(turn?.parts.map((part) => part.kind)).toEqual([
      "message",
      "tool",
      "message",
      "tool",
      "approval",
      "message",
    ]);
  });

  it("keep the render budgets: one commit per frame, a new row list only for a new row", () => {
    for (const scenario of SCENARIOS) {
      const result = replay(scenario.items);
      expect({
        name: scenario.name,
        commits: result.maxCommitsPerFrame,
        // The list changes only on a frame where a row came or went.
        changes: result.structureChanges > 0 && result.structureChanges <= result.rowMoves,
        // While the text streams, most frames keep the list.
        quietFrames: result.structureChanges * 4 < result.frames,
        problems: result.problems,
      }).toEqual({
        name: scenario.name,
        commits: 1,
        changes: true,
        quietFrames: true,
        problems: [],
      });
    }
  });
});

describe("faults", () => {
  const scenario = fc.constantFrom(...SCENARIOS);

  it("any drop pattern of drafts ends with the completed text", () => {
    const failures: string[][] = [];
    fc.assert(
      fc.property(
        scenario,
        fc.integer(),
        fc.double({ min: 0, max: 1, noNaN: true }),
        (s, seed, rate) => {
          const random = seeded(seed);
          const found = problems(replay(dropDrafts(s.items, () => random() < rate)), s.texts);
          failures.push(found);
          return found.length === 0;
        },
      ),
      { numRuns: 60, seed: SEED },
    );
    expect(failures.flat()).toEqual([]);
  });

  it("events sent twice change nothing", () => {
    const failures: string[][] = [];
    fc.assert(
      fc.property(scenario, fc.integer(), (s, seed) => {
        const random = seeded(seed);
        const found = problems(replay(duplicateEvents(s.items, () => random() < 0.2)), s.texts);
        failures.push(found);
        return found.length === 0;
      }),
      { numRuns: 30, seed: SEED },
    );
    expect(failures.flat()).toEqual([]);
  });

  it("a reconnect in the middle of a message, with a snapshot, ends with the completed text", () => {
    const failures: string[][] = [];
    fc.assert(
      fc.property(
        scenario,
        fc.nat(),
        fc.nat(),
        fc.integer({ min: 1, max: 128 }),
        (s, a, b, window) => {
          const cut = a % s.items.length;
          const back = cut + (b % (s.items.length - cut));
          const items = reconnectWithSnapshot(s.items, cut, back, window);
          const found = problems(replay(items), reachable(s, cut, back, window));
          failures.push(found);
          return found.length === 0;
        },
      ),
      { numRuns: 80, seed: SEED },
    );
    expect(failures.flat()).toEqual([]);
  });

  it("a resubscribe with after_seq after an overflow ends with the completed text", () => {
    const failures: string[][] = [];
    fc.assert(
      fc.property(scenario, fc.nat(), fc.nat(), (s, a, b) => {
        const cut = a % s.items.length;
        const back = cut + (b % (s.items.length - cut));
        const found = problems(replay(resubscribeAfterSeq(s.items, cut, back)), s.texts);
        failures.push(found);
        return found.length === 0;
      }),
      { numRuns: 60, seed: SEED },
    );
    expect(failures.flat()).toEqual([]);
  });
});

/** The items of a recording: one JSON value per line, in one of the shapes of `unwrap`. */
function recordingItems(text: string): TimedItem[] {
  const items: TimedItem[] = [];
  let at = 0;
  for (const line of text.split("\n")) {
    if (line.trim() === "") {
      continue;
    }
    const found = unwrap(JSON.parse(line));
    if (found === undefined) {
      continue;
    }
    at = found.at ?? at + FRAME_MS;
    items.push({ at, item: found.item });
  }
  return items;
}

/**
 * A line is an item (`{"kind": ...}`), a server frame (`{"id": n, "item": {...}}`) or a
 * record with a time (`{"at_ms" | "t_ms" | "ms" | "elapsed_ms": n, "item" | "frame": ...}`,
 * where a frame may also be the JSON text of one).
 */
function unwrap(value: unknown): { item: unknown; at?: number } | undefined {
  if (!isJson(value)) {
    return undefined;
  }
  const kind = value["kind"];
  if (kind === "event" || kind === "draft" || kind === "snapshot") {
    return { item: value };
  }
  const time = [value["at_ms"], value["t_ms"], value["ms"], value["elapsed_ms"]].find(
    (candidate): candidate is number => typeof candidate === "number",
  );
  const inner = value["item"] ?? value["frame"];
  const found = unwrap(typeof inner === "string" ? JSON.parse(inner) : inner);
  if (found === undefined) {
    return undefined;
  }
  return time === undefined ? found : { item: found.item, at: time };
}

/** The completed text of each message of `items`, by `turnId:index`. */
function completedTexts(items: readonly TimedItem[]): Map<string, string> {
  const texts = new Map<string, string>();
  const take = (envelope: unknown) => {
    const event = eventOf(envelope)?.event;
    if (event?.["kind"] === "assistant_message_completed") {
      texts.set(`${String(event["turn_id"])}:${String(event["index"])}`, String(event["text"]));
    }
  };
  for (const { item } of items) {
    if (isJson(item) && item["kind"] === "snapshot" && Array.isArray(item["events"])) {
      for (const envelope of item["events"]) {
        if (isJson(envelope)) {
          take(Object.assign({}, envelope, { kind: "event" }));
        }
      }
    } else {
      take(item);
    }
  }
  return texts;
}

describe("the recordings of efrd", () => {
  const paths = Object.keys(RECORDINGS).sort();

  it.skipIf(paths.length === 0)(
    "each recording, with and without drafts, ends with its text",
    () => {
      for (const path of paths) {
        const items = recordingItems(RECORDINGS[path] ?? "");
        const texts = completedTexts(items);
        const random = seeded(path.length);
        expect({
          path,
          items: items.length > 0,
          clean: problems(replay(items), texts),
          dropped: problems(replay(dropDrafts(items, () => random() < 0.5)), texts),
        }).toEqual({ path, items: true, clean: [], dropped: [] });
      }
    },
  );
});
