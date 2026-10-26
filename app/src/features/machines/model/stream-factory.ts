// The contract between a machine's supervisor and the stream of one conversation (plan
// section 4, W2-MACHINES). The supervisor makes a stream with the factory, holds it while a
// screen needs it, and feeds it from the client's subscription. The conversation feature
// implements the factory (features/conversation/model/conversation-stream.ts); the root
// layout gives it to the runtime.
//
// The supervisor and the runtime are generic in the type of the stream. So a hold gives
// the screens the full stream of the conversation feature, and the tests can use a small
// fake.

import type { Readable } from "@tanstack/store";

import type { SubscriptionSink } from "@/lib/net/client";
import type { ConversationSummary } from "@/lib/protocol/generated";
import type { ConversationCache } from "@/lib/store/event-cache";
import type { FrameScheduler } from "@/lib/store/frame-store";
import type { StreamState } from "@/lib/stream/reducer";
import type { ConversationRef } from "./machine-ref";

/** What the supervisor gives each stream that it makes. */
export interface StreamDeps {
  /** Runs a callback on the next display frame, so the store changes once per frame. */
  readonly schedule: FrameScheduler;
  /** The cache of this conversation; absent when the event cache did not open. */
  readonly cache?: ConversationCache;
  /** Called with the summary of each snapshot, so the lists can show it. */
  readonly onSummary?: (summary: ConversationSummary) => void;
  /** Called when the cache fails; the stream goes on without it. */
  readonly onCacheError?: (error: unknown) => void;
}

/** What the supervisor needs of the stream of one conversation. */
export interface SupervisedStream extends SubscriptionSink {
  /** The state of the conversation. It changes at most once per display frame. */
  readonly store: Readable<StreamState>;
  /**
   * Settles when the cached events are in the store, also when the cache failed. The
   * supervisor waits for it before the subscription starts, so that the subscription
   * resumes with `after_seq` from the cache. It must not reject.
   */
  readonly hydrated: Promise<void>;
  /** Stops every timer of the stream. The supervisor calls it once, at the end. */
  dispose(): void;
}

/** Makes the stream of `ref`. The stream starts to read its cache at once. */
export type ConversationStreamFactory<S extends SupervisedStream = SupervisedStream> = (
  ref: ConversationRef,
  deps: StreamDeps,
) => S;
