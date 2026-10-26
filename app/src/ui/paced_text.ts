// The paced text of one message row: how much of the message to show on each frame. It
// joins the pacer of the core (src/core/pacer.ts) with the rules of a row:
//
// - A row that mounts shows the text that it has at once, when that text is large (a
//   join in the middle of a reply or history); the core's catch-up rule decides that.
// - New text of the same generation goes to the pacer as an arrival.
// - A new generation (the completed text replaced the text) shows the whole text at
//   once; the view remounts, so its fade never misaligns.
// - The end of the message shows the whole text at once.
// - With Reduce Motion on, every frame shows the whole text.
//
// Pure: the row passes the time of each change and each frame.

import {
  createPacer,
  DEFAULT_PACER_CONFIG,
  type Pacer,
  pacerArrive,
  pacerFrame,
  pacerJump,
  type PacerConfig,
} from "@/lib/stream/pacer";

/** The pacing state of one message row. */
export interface PacedText {
  readonly generation: number;
  /** The length of the text that the pacer knows, in UTF-16 units. */
  readonly known: number;
  readonly final: boolean;
  readonly pacer: Pacer;
}

/** A message as the row sees it. */
export interface PacedInput {
  readonly text: string;
  readonly final: boolean;
  readonly generation: number;
}

/** The state of a row that mounts at `now` with `message`. */
export function startPaced(
  message: PacedInput,
  now: number,
  config: PacerConfig = DEFAULT_PACER_CONFIG,
): PacedText {
  return resumePaced(message, now, 0, config);
}

/**
 * The state of a row that already shows `shown` units of `message`, as after a restart of
 * its frame loop: it never moves back.
 */
export function resumePaced(
  message: PacedInput,
  now: number,
  shown: number,
  config: PacerConfig = DEFAULT_PACER_CONFIG,
): PacedText {
  const start: PacedText = {
    generation: message.generation,
    known: 0,
    final: false,
    pacer: createPacer(Math.min(Math.max(0, shown), message.text.length), now),
  };
  return pacedArrive(start, message, now, config);
}

/** Records the message after a change of the store, at `now`. */
export function pacedArrive(
  state: PacedText,
  message: PacedInput,
  now: number,
  config: PacerConfig = DEFAULT_PACER_CONFIG,
): PacedText {
  const length = message.text.length;
  if (message.generation !== state.generation) {
    return {
      generation: message.generation,
      known: length,
      final: message.final,
      pacer: createPacer(length, now),
    };
  }
  if (message.final) {
    if (state.final && state.known === length) {
      return state;
    }
    return { ...state, known: length, final: true, pacer: pacerJump(state.pacer, length, now) };
  }
  if (length <= state.known) {
    return state;
  }
  return { ...state, known: length, pacer: pacerArrive(state.pacer, length, now, config) };
}

/** One frame: the state after it, the length to show and whether the row can stop its frames. */
export interface PacedFrame {
  readonly state: PacedText;
  readonly shown: number;
  /** True when every frame from now on shows the same text until the next change. */
  readonly settled: boolean;
}

/** The frame at `now` for `text`, the text of the message now. */
export function pacedFrame(
  state: PacedText,
  text: string,
  now: number,
  reduceMotion: boolean,
  config: PacerConfig = DEFAULT_PACER_CONFIG,
): PacedFrame {
  const pacer = pacerFrame(state.pacer, text, now, { reduceMotion, config });
  const next = pacer === state.pacer ? state : { ...state, pacer };
  const shown = Math.min(pacer.shown, text.length);
  return {
    state: next,
    shown,
    settled: shown >= text.length && pacer.arrivals.length === 0,
  };
}
