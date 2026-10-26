// The contract between a machine's supervisor and the stream of one conversation (plan
// section 4, W2-MACHINES). The supervisor makes a stream with the factory, holds it while a
// screen needs it, and feeds it from the client's subscription. The conversation feature
// implements the factory (features/conversation/model/conversation-stream.ts); the root
// layout gives it to the runtime.

import type { Readable } from "@tanstack/store";

import type { Clock } from "@/lib/net/clock";
import type { SubscriptionSink } from "@/lib/net/client";
import type { EventCache } from "@/lib/store/event-cache";
import type { FrameScheduler } from "@/lib/store/frame-store";
import type { StreamState } from "@/lib/stream/reducer";
import type { ConversationRef } from "./machine-ref";

/** What the supervisor gives each stream that it makes. */
export interface StreamDeps {
  /** The event cache, or undefined when it did not open. The stream reads and writes it. */
  readonly cache: EventCache | undefined;
  /** Runs a callback on the next display frame, so the store changes once per frame. */
  readonly schedule: FrameScheduler;
  readonly clock: Clock;
  /** Called with an error that the stream goes on after, such as a failed cache write. */
  readonly onError: (error: unknown) => void;
}

/** The stream of one conversation: the sink of its subscription and its store. */
export type ConversationStream = SubscriptionSink & {
  /** The state of the conversation. It changes at most once per display frame. */
  readonly store: Readable<StreamState>;
  /**
   * Reads the cached events into the store. The supervisor calls it once, before the
   * subscription starts, so that the subscription resumes with `after_seq` from the cache.
   * It must not reject; a failed cache only makes the stream wait for efrd.
   */
  hydrate(): Promise<void>;
  /** Stops every timer of the stream. The supervisor calls it once, at the end. */
  dispose(): void;
};

/** Makes the stream of `ref`. */
export type ConversationStreamFactory = (
  ref: ConversationRef,
  deps: StreamDeps,
) => ConversationStream;
