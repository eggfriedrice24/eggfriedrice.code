// The stream that one conversation screen holds (plan section 2.2, "Subscriptions"):
//
// - A screen in the stack keeps its stream in memory ("keep").
// - The screen with the focus also keeps the subscription up ("follow"). When the screen
//   gets the focus again, the client subscribes again with `after_seq` from the cursor of
//   the stream, so only the events after the last one come.
// - While the screen has the focus, the watches of screen-watch.ts run on its stream:
//   the VoiceOver announcements, the haptic of a failed turn and the scroll to an
//   approval on request.
//
// The screen subscribes to this object with useSyncExternalStore: the subscribe takes the
// hold and starts the watches, and the unsubscribe stops them. So React owns the lifetime
// of the hold, and the screen needs no effect. The first render has no stream yet; the
// render after the subscribe has it.
//
// `subscriber` gives the same function for the same request. useSyncExternalStore
// subscribes again when its function changes, and each new subscription would close the
// subscription of efrd and open it again. So a render that changes nothing of the request
// keeps the hold, also without the memo of the React Compiler.
//
// Pure: the effects come in as parameters.

import type { Readable } from "@tanstack/store";

import type { FrameScheduler } from "@/lib/store/frame-store";
import type { StreamState } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { conversationKey } from "@/features/machines/model/machine-ref";
import type { HoldMode, StreamHold } from "@/features/machines/model/machine-supervisor";
import type { SupervisedStream } from "@/features/machines/model/stream-factory";
import type { Announcement } from "./announce";
import { followApprovalFocus, watchFocusedStream } from "./screen-watch";

/** The part of the runtime that holds streams. */
export interface StreamSource<S extends SupervisedStream> {
  /** Holds the stream of `ref`, or gives undefined when its machine is not paired. */
  hold(ref: ConversationRef, mode: HoldMode): StreamHold<S> | undefined;
}

/** What a screen does with its stream while it has the focus. */
export interface ScreenEffects {
  /** Says an announcement with VoiceOver. */
  readonly announce: (announcement: Announcement) => void;
  /** Plays the error haptic of a failed turn. */
  readonly failedTurn: () => void;
  /** Runs a callback on the next display frame. */
  readonly schedule: FrameScheduler;
  /** Scrolls the transcript to the row with `key`. */
  readonly scrollTo: (key: string) => void;
  /** The `focus` request of the screen of `ref` is done: clear it from the route. */
  readonly focusDone: (ref: ConversationRef) => void;
}

/** What a screen holds now. */
export interface HoldRequest {
  readonly machine: string;
  /** The conversation id, or undefined while efrd has not made it: then no hold. */
  readonly id: string | undefined;
  /** True while the screen has the focus: the subscription is up. */
  readonly focused: boolean;
  /** `approval`: scroll to the approval that waits. */
  readonly focus: "approval" | undefined;
}

/** The subscribe of useSyncExternalStore. */
export type Subscribe = (onChange: () => void) => () => void;

interface Held<S> {
  readonly key: string;
  readonly stream: S;
}

/** The hold of one screen: at most one stream at a time. */
export class ScreenStream<S extends SupervisedStream> {
  private readonly source: StreamSource<S>;
  private readonly effects: ScreenEffects;
  private held: Held<S> | undefined;
  private last: { readonly key: string; readonly subscribe: Subscribe } | undefined;

  constructor(source: StreamSource<S>, effects: ScreenEffects) {
    this.source = source;
    this.effects = effects;
  }

  /** The subscribe for `request`: the same function while the request is the same. */
  subscriber(request: HoldRequest): Subscribe {
    const key = [request.machine, request.id ?? "", request.focused, request.focus ?? ""].join(
      "\n",
    );
    if (this.last?.key === key) {
      return this.last.subscribe;
    }
    const subscribe: Subscribe = (onChange) => this.subscribe(request, onChange);
    this.last = { key, subscribe };
    return subscribe;
  }

  /** The stream of `ref` while this screen holds it, else undefined. */
  stream(ref: ConversationRef): S | undefined {
    const held = this.held;
    return held !== undefined && held.key === conversationKey(ref) ? held.stream : undefined;
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

  private subscribe(request: HoldRequest, onChange: () => void): () => void {
    const { machine, id, focused, focus } = request;
    if (id === undefined) {
      return () => undefined;
    }
    const ref: ConversationRef = { machine, id };
    const stops = [this.hold(ref, focused ? "follow" : "keep")];
    const store: Readable<StreamState> | undefined = this.stream(ref)?.store;
    if (focused && store !== undefined) {
      const effects = this.effects;
      stops.push(watchFocusedStream(store, effects));
      if (focus === "approval") {
        stops.push(
          followApprovalFocus(store, {
            schedule: effects.schedule,
            scrollTo: effects.scrollTo,
            done: () => effects.focusDone(ref),
          }),
        );
      }
    }
    // The snapshot has the stream now: the screen renders with it.
    onChange();
    return () => {
      for (let i = stops.length - 1; i >= 0; i--) {
        stops[i]?.();
      }
    };
  }
}
