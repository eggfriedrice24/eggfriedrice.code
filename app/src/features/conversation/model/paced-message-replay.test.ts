// Replay tests of the paced view of a message row: the scripted streams of scenarios.ts
// go through the conversation stream and its frame store, and each message gets the
// paced view that its row would read. On every frame the shown text must not move back
// within a generation, a final segment must keep its object (so its view never renders
// again), and at the end each message shows its completed text and stops streaming.

import { describe, expect, it } from "vitest";

import { scenarios, type TimedItem } from "@/features/demo/model/scenarios";
import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import { manualFrames } from "@/lib/stream/testing";
import { createConversationStream } from "./conversation-stream";
import type { Segment } from "./message-segments";
import { createPacedMessage, type PacedMessage, type ShownMessage } from "./paced-message";
import { FRAME_MS } from "./testing";

interface Watched {
  readonly paced: PacedMessage;
  last: ShownMessage;
  renders: number;
}

function isFinal(segment: Segment): boolean {
  return segment.kind === "diff" || (segment.kind === "code" ? segment.closed : !segment.open);
}

function replayPaced(items: readonly TimedItem[], reduceMotion: boolean) {
  const frames = manualFrames();
  let now = 0;
  const stream = createConversationStream(
    { machine: "replay", id: "replay" },
    { schedule: frames.schedule },
  );
  const watched = new Map<string, Watched>();
  const problems: string[] = [];
  const end = (items[items.length - 1]?.at ?? 0) + 1_000;
  let next = 0;
  for (now = 0; now <= end; now += FRAME_MS) {
    while (next < items.length && (items[next]?.at ?? 0) <= now) {
      const raw = items[next]?.item;
      next++;
      const decoded = decodeSubscribeItem(raw);
      if (decoded.ok) {
        stream.item(decoded.value, raw);
      }
    }
    frames.frame();
    for (const [turnId, turn] of stream.store.get().turns) {
      for (const index of turn.messages.keys()) {
        const key = `${turnId}:${String(index)}`;
        if (!watched.has(key)) {
          const paced = createPacedMessage(stream.store, turnId, index, {
            schedule: frames.schedule,
            now: () => now,
            reduceMotion: () => reduceMotion,
          });
          const entry: Watched = { paced, last: paced.snapshot(), renders: 0 };
          paced.subscribe(() => entry.renders++);
          watched.set(key, entry);
        }
      }
    }
    for (const [key, entry] of watched) {
      const shown = entry.paced.snapshot();
      const before = entry.last;
      const sameGeneration = before.message?.generation === shown.message?.generation;
      if (sameGeneration && !shown.text.startsWith(before.text)) {
        problems.push(`${key} at ${String(now)} ms: the shown text moved back`);
      }
      if (sameGeneration) {
        for (const segment of before.segments.filter(isFinal)) {
          const after = shown.segments.find((other) => other.key === segment.key);
          if (after !== undefined && after !== segment) {
            problems.push(`${key} at ${String(now)} ms: final segment ${segment.key} changed`);
          }
        }
      }
      entry.last = shown;
    }
  }
  stream.dispose();
  return { watched, problems };
}

describe("the paced view over the scripted streams", () => {
  it.each(scenarios().map((scenario) => [scenario.name, scenario] as const))(
    "ends with the completed text and keeps its final segments: %s",
    (_name, scenario) => {
      for (const reduceMotion of [false, true]) {
        const { watched, problems } = replayPaced(scenario.items, reduceMotion);
        const ends = [...scenario.texts].map(([key, text]) => {
          const last = watched.get(key)?.last;
          return { key, text: last?.text === text, streaming: last?.streaming };
        });
        expect({ problems, ends }).toEqual({
          problems: [],
          ends: [...scenario.texts.keys()].map((key) => ({ key, text: true, streaming: false })),
        });
      }
    },
  );
});
