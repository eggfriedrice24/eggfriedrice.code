// The stream of one conversation on one machine: the sink of its subscription, the one
// stream reducer, the event cache and the store that the screens read (plan section
// 2.2). It lives outside React, in the runtime, as long as its screen is in the stack.
//
// - The sink keeps the newest state in a plain field. The frame store writes it to the
//   TanStack Store once per display frame, so a burst of drafts renders once.
// - Order on open: the cached events first, so a cold start shows the transcript at
//   once, then the subscription, which resumes with `after_seq` from the cursor. The
//   owner waits for `hydrated` before it subscribes. When the cache answers late, after
//   the subscription gave items, its turns go under the newer state and never replace
//   it.
// - Each event goes to the cache as efrd sent it, and a snapshot replaces the cached
//   events. The cache is not the truth: a cache that fails only makes the app wait for
//   efrd.
//
// This module implements the `ConversationStreamFactory` that the machine supervisor
// uses (plan section 4, W2-MACHINES).

import type { Readable } from "@tanstack/store";

import type { SubscriptionSink, SubscriptionStatus } from "@/lib/net/client";
import { decodeEventEnvelope, isJsonObject } from "@/lib/protocol/envelope";
import type { ConversationSummary } from "@/lib/protocol/generated";
import type { EventEnvelope, SubscribeItem } from "@/lib/protocol/wire";
import { createFrameStore, type FrameScheduler } from "@/lib/store/frame-store";
import { EMPTY_STREAM, reduce, type StreamState, underlay, withLink } from "@/lib/stream/reducer";
import { createRowUiStore, type RowUiStore } from "./row-ui-state";

/** A conversation on a machine. `machine` is the daemon id of the machine. */
export interface ConversationRef {
  readonly machine: string;
  readonly conversation: string;
}

/** One cached event: its sequence number and the JSON of its envelope as efrd sent it. */
export interface CachedEventRow {
  readonly seq: number;
  readonly json: string;
}

/** The event cache of one conversation, as the runtime adapts the machine's cache. */
export interface ConversationCache {
  /** The cached events, oldest first. */
  load(): Promise<readonly CachedEventRow[]>;
  /** Adds events; an event with a sequence number that the cache has replaces it. */
  append(events: readonly CachedEventRow[]): Promise<void>;
  /** Replaces all events, as after a snapshot. */
  replace(events: readonly CachedEventRow[]): Promise<void>;
}

/** What a stream needs from the runtime. */
export interface StreamDeps {
  /** The display frames (`animationFrames` in the app). */
  readonly schedule: FrameScheduler;
  readonly cache?: ConversationCache;
  /** Called with the summary of each snapshot, so the lists can show it. */
  readonly onSummary?: (summary: ConversationSummary) => void;
  /** Called when the cache fails; the stream goes on without it. */
  readonly onCacheError?: (error: unknown) => void;
}

/** The stream of one conversation, and the sink of its subscription. */
export interface ConversationStream extends SubscriptionSink {
  readonly ref: ConversationRef;
  /** The state of the last frame, for `useSelector` with the selectors of selectors.ts. */
  readonly store: Readable<StreamState>;
  /** The rows that the user opened. */
  readonly rowUi: RowUiStore;
  /** Settles when the cached events are in the state (also when the cache failed). */
  readonly hydrated: Promise<void>;
  /** The newest state, ahead of `store` by at most one frame. */
  latest(): StreamState;
  /** Writes the newest state to `store` now, without a frame. */
  flush(): void;
  /** The subscription changed its state. */
  status(status: SubscriptionStatus): void;
  /** Waits for the cache writes that are queued now. */
  settled(): Promise<void>;
  /** Stops the frames and the cache writes. Later items change nothing. */
  dispose(): void;
}

/** Makes the stream of a conversation (W2-MACHINES' `ConversationStreamFactory`). */
export type ConversationStreamFactory = (
  ref: ConversationRef,
  deps: StreamDeps,
) => ConversationStream;

/** A new stream of `ref`. It starts to read the cache at once. */
export const createConversationStream: ConversationStreamFactory = (ref, deps) => {
  const frame = createFrameStore<StreamState>(EMPTY_STREAM, deps.schedule);
  const rowUi = createRowUiStore();
  let writes: Promise<void> = Promise.resolve();
  let disposed = false;
  let sawItems = false;

  const commit = (next: StreamState): void => {
    if (!disposed) {
      frame.set(next);
    }
  };

  const write = (run: (cache: ConversationCache) => Promise<void>): void => {
    const cache = deps.cache;
    if (cache === undefined || disposed) {
      return;
    }
    writes = writes.then(
      () => run(cache).catch((error: unknown) => deps.onCacheError?.(error)),
      () => undefined,
    );
  };

  const hydrate = async (): Promise<void> => {
    const cache = deps.cache;
    if (cache === undefined) {
      return;
    }
    let rows: readonly CachedEventRow[];
    try {
      rows = await cache.load();
    } catch (error) {
      deps.onCacheError?.(error);
      return;
    }
    if (disposed) {
      return;
    }
    const envelopes: EventEnvelope[] = [];
    for (const row of rows) {
      const envelope = parseEnvelope(row.json);
      if (envelope !== undefined) {
        envelopes.push(envelope);
      }
    }
    const current = frame.latest();
    let next: StreamState;
    if (sawItems) {
      next = underlay(current, envelopes);
    } else {
      next = current;
      for (const envelope of envelopes) {
        next = reduce(next, { kind: "event", ...envelope });
      }
    }
    commit(next);
    // A cold start shows the cached transcript at once, not on the next frame.
    frame.flush();
  };

  const item = (decoded: SubscribeItem, raw: unknown): void => {
    if (disposed) {
      return;
    }
    const current = frame.latest();
    if (decoded.kind === "event" && decoded.seq <= current.lastSeq) {
      return;
    }
    sawItems ||= decoded.kind !== "other";
    commit(reduce(current, decoded));
    switch (decoded.kind) {
      case "event":
        write((cache) => cache.append(cachedEvents([withoutKind(raw)], [decoded.seq])));
        return;
      case "snapshot": {
        deps.onSummary?.(decoded.conversation);
        const rawEvents = isJsonObject(raw) && Array.isArray(raw["events"]) ? raw["events"] : [];
        const seqs = decoded.events.map((envelope) => envelope.seq);
        write((cache) => cache.replace(cachedEvents(rawEvents, seqs)));
        return;
      }
      default:
        return;
    }
  };

  const status = (next: SubscriptionStatus): void => {
    if (!disposed) {
      commit(withLink(frame.latest(), next.state));
    }
  };

  return {
    ref,
    store: frame.store,
    rowUi,
    hydrated: hydrate(),
    latest: () => frame.latest(),
    flush: () => {
      frame.flush();
    },
    settled: () => writes,
    dispose: () => {
      disposed = true;
      frame.dispose();
    },
    cursor: () => {
      const seq = frame.latest().lastSeq;
      return seq > 0 ? seq : undefined;
    },
    item,
    status,
  };
};

/** The raw event item without its `kind` member: the envelope as efrd sent it. */
function withoutKind(raw: unknown): unknown {
  if (!isJsonObject(raw)) {
    return raw;
  }
  const { kind: _kind, ...envelope } = raw;
  return envelope;
}

function cachedEvents(raws: readonly unknown[], seqs: readonly number[]): CachedEventRow[] {
  return raws.flatMap((raw, index) => {
    const seq = seqs[index];
    return seq === undefined ? [] : [{ seq, json: JSON.stringify(raw) }];
  });
}

/** A cached envelope, decoded; `undefined` for a row that is not one. */
function parseEnvelope(json: string): EventEnvelope | undefined {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return undefined;
  }
  const decoded = decodeEventEnvelope(value);
  return decoded.ok ? decoded.value : undefined;
}
