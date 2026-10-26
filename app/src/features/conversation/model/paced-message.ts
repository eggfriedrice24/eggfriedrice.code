// The paced view of one message row: what the row shows of its message on each display
// frame (research R4c.4 steps 4 to 6; the port of `usePacedMessage` of the old UI).
//
// It is an external store for useSyncExternalStore, so the row needs no effect and no
// state of its own:
//
// - The first subscriber starts it: it reads the stream store and follows its changes.
//   Each change of the message goes to the pacer as an arrival at the time it came
//   (paced-text.ts), and asks for a frame.
// - On each frame the pacer gives the length to show. The snapshot changes only when the
//   shown text changes, so the row renders at most once per frame, and a message that
//   does not change renders never. A settled message asks for no more frames.
// - The shown text goes through the block splitter, which scans only the tail, and the
//   segments of message-segments.ts.
// - The last subscriber stops it. A new subscriber continues from what it showed, so
//   the text never moves back.
//
// Pure: the caller gives the frame scheduler, the clock and the Reduce Motion setting.

import type { Readable } from "@tanstack/store";

import type { FrameScheduler } from "@/lib/store/frame-store";
import type { MessageView, StreamState } from "@/lib/stream/reducer";
import { type BlockSplit, EMPTY_SPLIT, splitBlocks } from "./blocks";
import { messageSegments, type Segment, type SegmentCache } from "./message-segments";
import { pacedArrive, pacedFrame, type PacedInput, type PacedText, startPaced } from "./paced-text";

/** What a message row shows on one frame. */
export interface ShownMessage {
  /** The message of the store, or undefined before it exists. */
  readonly message: MessageView | undefined;
  /** The part of the text to show now: a prefix of the message's text. */
  readonly text: string;
  readonly segments: readonly Segment[];
  /** True while text still comes: the message is not final, or the pacer still shows more. */
  readonly streaming: boolean;
}

/** What the paced view needs from the platform. */
export interface PacedMessageDeps {
  readonly schedule: FrameScheduler;
  /** The time in ms, for the pacer. */
  readonly now: () => number;
  /** True while Reduce Motion is on: every frame shows the whole text. */
  readonly reduceMotion: () => boolean;
}

/** The paced view of one message, as an external store. */
export interface PacedMessage {
  readonly subscribe: (listener: () => void) => () => void;
  readonly snapshot: () => ShownMessage;
}

function input(message: MessageView): PacedInput {
  return { text: message.buffer.text, final: message.final, generation: message.generation };
}

/** The paced view of message `index` of turn `turnId` in `store`. */
export function createPacedMessage(
  store: Readable<StreamState>,
  turnId: string,
  index: number,
  deps: PacedMessageDeps,
): PacedMessage {
  const select = (): MessageView | undefined => store.get().turns.get(turnId)?.messages.get(index);
  let message = select();
  let paced: PacedText | undefined;
  let split: BlockSplit = EMPTY_SPLIT;
  let segmentCache: SegmentCache = new Map();
  let shown: ShownMessage;
  const listeners = new Set<() => void>();
  let cancelFrame: (() => void) | undefined;
  let stopStore: (() => void) | undefined;

  const show = (current: MessageView | undefined, length: number, settled: boolean): void => {
    if (current === undefined) {
      shown = { message: current, text: "", segments: [], streaming: false };
      return;
    }
    const full = current.buffer.text;
    const text = length >= full.length ? full : full.slice(0, length);
    const final = current.final && text === full;
    split = splitBlocks(text, split);
    shown = {
      message: current,
      text,
      segments: messageSegments(text, split, final, segmentCache),
      streaming: !final || !settled,
    };
  };

  const frame = (): void => {
    cancelFrame = undefined;
    const current = message;
    if (current === undefined || paced === undefined) {
      return;
    }
    const result = pacedFrame(paced, current.buffer.text, deps.now(), deps.reduceMotion());
    paced = result.state;
    const previous = shown;
    const textChanged = previous.message !== current || previous.text.length !== result.shown;
    if (textChanged || previous.streaming !== (!current.final || !result.settled)) {
      show(current, result.shown, result.settled);
      for (const listener of listeners) {
        listener();
      }
    }
    if (!result.settled) {
      cancelFrame = deps.schedule(frame);
    }
  };

  const change = (): void => {
    const next = select();
    if (next === message && paced !== undefined) {
      return;
    }
    if (next !== undefined) {
      if (message !== undefined && next.generation !== message.generation) {
        split = EMPTY_SPLIT;
        segmentCache = new Map();
      }
      const now = deps.now();
      paced =
        paced === undefined ? startPaced(input(next), now) : pacedArrive(paced, input(next), now);
    }
    message = next;
    cancelFrame ??= deps.schedule(frame);
  };

  // The first snapshot: a row that mounts shows the text that it has, as the pacer's
  // catch-up rule says, so a row that scrolls into view does not type its text again.
  if (message === undefined) {
    show(undefined, 0, true);
  } else {
    const now = deps.now();
    paced = startPaced(input(message), now);
    const first = pacedFrame(paced, message.buffer.text, now, deps.reduceMotion());
    paced = first.state;
    show(message, first.shown, first.settled);
  }

  return {
    subscribe: (listener) => {
      listeners.add(listener);
      if (stopStore === undefined) {
        const subscription = store.subscribe(() => change());
        stopStore = () => subscription.unsubscribe();
        change();
        // The pacer may still owe text from before, such as after a stop.
        if (shown.streaming) {
          cancelFrame ??= deps.schedule(frame);
        }
      }
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0) {
          stopStore?.();
          stopStore = undefined;
          cancelFrame?.();
          cancelFrame = undefined;
        }
      };
    },
    snapshot: () => shown,
  };
}
