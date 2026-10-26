// The pacer: how much of a streamed message to show at a given time (R4c.4 step 4 of
// the research). efrd sends drafts at about 60 Hz, but the network delivers them in
// bursts. The pacer turns the bursts into a steady reveal by word, and it never shows a
// character later than `maxLagMs` after it arrived.
//
// The model: each arrival says "the text up to `end` arrived at `at`". Its deadline is
// `at + maxLagMs`. The reveal is a line through the deadlines: from the last passed
// deadline to the next one, it moves at the speed the text arrived. So the reveal is
// continuous, never moves back, and meets each deadline exactly. On a frame, the shown
// length snaps back to the end of the last whole word, but never below the text that is
// due.
//
// Positions are UTF-16 indexes into the message text, the unit of JS strings. All
// functions are pure; the caller passes the time.

import { isCodePointBoundary } from "./utf8";

/** The settings of the pacer. */
export interface PacerConfig {
  /** The longest time from the arrival of a character to its reveal. */
  readonly maxLagMs: number;
  /** An arrival that adds at least this many UTF-16 units shows at once (catch-up). */
  readonly catchUpUnits: number;
}

/** The settings that the research gives: at most 120 ms behind. */
export const DEFAULT_PACER_CONFIG: PacerConfig = { maxLagMs: 120, catchUpUnits: 4096 };

/** The text up to `end` arrived at time `at`. */
export interface Arrival {
  readonly end: number;
  readonly at: number;
}

/** The state of the pacer of one message. */
export interface Pacer {
  /** The length shown on the last frame. */
  readonly shown: number;
  /** The reveal is at `basePos` at time `baseAt`: the last deadline that passed. */
  readonly basePos: number;
  readonly baseAt: number;
  /** Arrivals whose deadline has not passed, oldest first. */
  readonly arrivals: readonly Arrival[];
}

/** A pacer that shows the first `shown` units. */
export function createPacer(shown = 0, now = 0): Pacer {
  return { shown, basePos: shown, baseAt: now, arrivals: [] };
}

/** The end of all text the pacer knows. */
export function knownEnd(pacer: Pacer): number {
  const last = pacer.arrivals[pacer.arrivals.length - 1];
  return last === undefined ? pacer.basePos : last.end;
}

/** Records that the text up to `end` arrived at `at`. */
export function pacerArrive(
  pacer: Pacer,
  end: number,
  at: number,
  config: PacerConfig = DEFAULT_PACER_CONFIG,
): Pacer {
  const known = knownEnd(pacer);
  if (end <= known) {
    return pacer;
  }
  if (end - known >= config.catchUpUnits) {
    return pacerJump(pacer, end, at);
  }
  return { ...pacer, arrivals: [...pacer.arrivals, { end, at }] };
}

/** Shows all text up to `end` at once: on catch-up, at the end of a message, after a replace. */
export function pacerJump(pacer: Pacer, end: number, at: number): Pacer {
  const shown = Math.max(pacer.shown, end);
  return { shown, basePos: shown, baseAt: at, arrivals: [] };
}

/** Where the reveal line is at `now`, before the snap to words. */
export function revealAt(pacer: Pacer, now: number, config: PacerConfig): number {
  let fromPos = pacer.basePos;
  let fromAt = pacer.baseAt;
  for (const arrival of pacer.arrivals) {
    const deadline = arrival.at + config.maxLagMs;
    if (now >= deadline) {
      fromPos = arrival.end;
      fromAt = deadline;
      continue;
    }
    const start = Math.max(arrival.at, fromAt);
    if (now <= start) {
      return fromPos;
    }
    const share = (now - start) / (deadline - start);
    return fromPos + Math.floor((arrival.end - fromPos) * share);
  }
  return fromPos;
}

/** Options of one frame. */
export interface FrameOptions {
  /** With Reduce Motion on, the pacer shows all text at once. */
  readonly reduceMotion?: boolean;
  readonly config?: PacerConfig;
}

/**
 * Computes the frame at `now` for the message `text`. The result's `shown` is the
 * length to show; it never goes back and never passes `text.length`.
 */
export function pacerFrame(
  pacer: Pacer,
  text: string,
  now: number,
  options: FrameOptions = {},
): Pacer {
  const config = options.config ?? DEFAULT_PACER_CONFIG;
  if (options.reduceMotion === true) {
    const all = pacerJump(pacer, text.length, now);
    return { ...all, shown: text.length };
  }
  const advanced = advance(pacer, now, config);
  const floor = Math.min(Math.max(pacer.shown, advanced.basePos), text.length);
  const target = Math.min(revealAt(advanced, now, config), text.length);
  const shown = Math.max(floor, wordBoundaryAtOrBefore(text, target, floor));
  return shown === pacer.shown && advanced === pacer ? pacer : { ...advanced, shown };
}

/** Drops the arrivals whose deadline passed, and moves the base to the last of them. */
function advance(pacer: Pacer, now: number, config: PacerConfig): Pacer {
  let passed = 0;
  let basePos = pacer.basePos;
  let baseAt = pacer.baseAt;
  for (const arrival of pacer.arrivals) {
    const deadline = arrival.at + config.maxLagMs;
    if (now < deadline) {
      break;
    }
    basePos = arrival.end;
    baseAt = deadline;
    passed++;
  }
  if (passed === 0) {
    return pacer;
  }
  return { ...pacer, basePos, baseAt, arrivals: pacer.arrivals.slice(passed) };
}

/**
 * The largest index from `floor` to `index` that ends a word, or `floor` when there is
 * none. A word ends after whitespace, and after each CJK character, because CJK text has
 * no spaces. The scan stops at `floor`, so a frame costs at most the text that it adds.
 */
export function wordBoundaryAtOrBefore(text: string, index: number, floor = 0): number {
  if (index >= text.length) {
    return text.length;
  }
  for (let i = index; i > floor; i--) {
    if (endsWord(text.charCodeAt(i - 1)) && isCodePointBoundary(text, i)) {
      return i;
    }
  }
  return floor;
}

function endsWord(unit: number): boolean {
  return (
    unit === 0x20 ||
    unit === 0x0a ||
    unit === 0x09 ||
    unit === 0x0d ||
    // CJK punctuation, kana, ideographs, Hangul syllables and compatibility ideographs.
    (unit >= 0x3000 && unit <= 0x30ff) ||
    (unit >= 0x3400 && unit <= 0x9fff) ||
    (unit >= 0xac00 && unit <= 0xd7af) ||
    (unit >= 0xf900 && unit <= 0xfaff)
  );
}
