// The stream that one conversation screen holds (plan section 2.2, "Subscriptions"):
//
// - A screen in the stack keeps its stream in memory ("keep").
// - The screen with the focus also keeps the subscription up ("follow"). When the screen
//   gets the focus again, the client subscribes again with `after_seq` from the cursor of
//   the stream, so only the events after the last one come.
//
// The screen subscribes to this object with useSyncExternalStore: the subscribe takes the
// hold, and the unsubscribe gives it back. So React owns the lifetime of the hold, and the
// screen needs no effect. The first render has no stream yet; the render after the
// subscribe has it.
//
// Pure.

import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { conversationKey } from "@/features/machines/model/machine-ref";
import type { HoldMode, StreamHold } from "@/features/machines/model/machine-supervisor";
import type { SupervisedStream } from "@/features/machines/model/stream-factory";

/** The part of the runtime that holds streams. */
export interface StreamSource<S extends SupervisedStream> {
  /** Holds the stream of `ref`, or gives undefined when its machine is not paired. */
  hold(ref: ConversationRef, mode: HoldMode): StreamHold<S> | undefined;
}

interface Held<S> {
  readonly key: string;
  readonly stream: S;
}

/** The hold of one screen: at most one stream at a time. */
export class ScreenStream<S extends SupervisedStream> {
  private readonly source: StreamSource<S>;
  private held: Held<S> | undefined;

  constructor(source: StreamSource<S>) {
    this.source = source;
  }

  /**
   * Holds the stream of `ref` in `mode` and returns the function that gives it back. A
   * machine that is not paired gives no stream.
   */
  hold(ref: ConversationRef, mode: HoldMode): () => void {
    const hold = this.source.hold(ref, mode);
    if (hold === undefined) {
      return () => undefined;
    }
    const held: Held<S> = { key: conversationKey(ref), stream: hold.stream };
    this.held = held;
    let released = false;
    return () => {
      if (released) {
        return;
      }
      released = true;
      hold.release();
      if (this.held === held) {
        this.held = undefined;
      }
    };
  }

  /** The stream of `ref` while this screen holds it, else undefined. */
  stream(ref: ConversationRef): S | undefined {
    const held = this.held;
    return held !== undefined && held.key === conversationKey(ref) ? held.stream : undefined;
  }
}
