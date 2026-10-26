import { createStore } from "@tanstack/store";
import { describe, expect, it } from "vitest";

import { EMPTY_STREAM } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import type { HoldMode, StreamHold } from "@/features/machines/model/machine-supervisor";
import type { SupervisedStream } from "@/features/machines/model/stream-factory";
import { ScreenStream, type StreamSource } from "./screen-stream";

const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const ONE: ConversationRef = { machine: DESKTOP, id: "019a9b1c-3d00-7a10-8b20-0000000000a1" };
const TWO: ConversationRef = { machine: DESKTOP, id: "019a9b1c-3d00-7a10-8b20-0000000000a2" };

/** A stream that does nothing. */
function stream(): SupervisedStream {
  return {
    store: createStore(EMPTY_STREAM),
    hydrated: Promise.resolve(),
    dispose: () => undefined,
    item: () => undefined,
    status: () => undefined,
    cursor: () => 0,
  };
}

/** A source that records each hold, and knows only the machine desktop. */
class FakeSource implements StreamSource<SupervisedStream> {
  readonly holds: { key: string; mode: HoldMode; released: number }[] = [];
  private readonly streams = new Map<string, SupervisedStream>();

  hold(ref: ConversationRef, mode: HoldMode): StreamHold | undefined {
    if (ref.machine !== DESKTOP) {
      return undefined;
    }
    let held = this.streams.get(ref.id);
    if (held === undefined) {
      held = stream();
      this.streams.set(ref.id, held);
    }
    const record = { key: ref.id, mode, released: 0 };
    this.holds.push(record);
    return {
      stream: held,
      release: () => {
        record.released += 1;
      },
    };
  }
}

describe("the stream of a screen", () => {
  it("has the stream while it holds it, and gives the hold back once", () => {
    const source = new FakeSource();
    const screen = new ScreenStream(source);
    expect(screen.stream(ONE)).toBeUndefined();
    const release = screen.hold(ONE, "follow");
    expect(screen.stream(ONE)).toBeDefined();
    expect(screen.stream(TWO)).toBeUndefined();
    release();
    release();
    expect(screen.stream(ONE)).toBeUndefined();
    expect(source.holds).toEqual([{ key: ONE.id, mode: "follow", released: 1 }]);
  });

  it("keeps the new hold when the old one goes after it, as React resubscribes", () => {
    const source = new FakeSource();
    const screen = new ScreenStream(source);
    const follow = screen.hold(ONE, "follow");
    const keep = screen.hold(ONE, "keep");
    follow();
    const held = screen.stream(ONE);
    expect(held).toBeDefined();
    keep();
    expect(screen.stream(ONE)).toBeUndefined();
    expect(source.holds.map((hold) => [hold.mode, hold.released])).toEqual([
      ["follow", 1],
      ["keep", 1],
    ]);
  });

  it("has no stream for a machine that is not paired", () => {
    const screen = new ScreenStream(new FakeSource());
    const other = { machine: "019a9b1c-3d00-7a10-8b20-0000000000d2", id: ONE.id };
    const release = screen.hold(other, "follow");
    expect(screen.stream(other)).toBeUndefined();
    release();
  });
});
