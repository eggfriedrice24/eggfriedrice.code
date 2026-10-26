// The two loops of the app as pure data: the braille loader and the blink of the block
// cursor (design 3.20 and section 4). The views run each loop on one RN Animated value
// with the native driver (use-loop-clock.ts), and these step tables turn the value into
// opacities, so no JavaScript runs on a frame.
//
// The braille cell is a 2 x 4 grid of dots in a 6 x 12 box (the S-Loaders board). Dot 0
// is at the top left; the numbers go clockwise. On each step of 100 ms the lit dot moves
// one place clockwise and leaves a trail of two dots: opacities 1, 0.7, 0.4, and 0.16 for
// the rest.

/** One loop: the clock value runs from 0 to `steps` in `cycleMs`, then starts again. */
export interface Loop {
  readonly cycleMs: number;
  readonly steps: number;
}

/** The braille loader: 8 steps in 0.8 s. */
export const BRAILLE_LOOP: Loop = { cycleMs: 800, steps: 8 };

/** The cursor blink: on for the first half of 1.06 s, low for the second half. */
export const BLINK_LOOP: Loop = { cycleMs: 1060, steps: 2 };

/** The centres of the 8 dots in the 6 x 12 box of the design, in clockwise order. */
export const BRAILLE_DOTS: readonly { readonly x: number; readonly y: number }[] = [
  { x: 1.5, y: 1.5 },
  { x: 4.5, y: 1.5 },
  { x: 4.5, y: 4.5 },
  { x: 4.5, y: 7.5 },
  { x: 4.5, y: 10.5 },
  { x: 1.5, y: 10.5 },
  { x: 1.5, y: 7.5 },
  { x: 1.5, y: 4.5 },
];

const BOX = { width: 6, height: 12 } as const;
const DOT_RADIUS = 1.1;

/** The opacity of a dot by its distance behind the lit dot: the lit dot, the trail, the rest. */
const TRAIL = [1, 0.7, 0.4] as const;
const DIM = 0.16;

/**
 * The still frame for Reduce Motion and for menus: dots 0 and 1 lit, dot 7 at half, the
 * rest dim.
 */
export const BRAILLE_STILL: readonly number[] = [1, 1, DIM, DIM, DIM, DIM, DIM, 0.5];

/** The opacity of `dot` on `step` of the braille loop. */
export function brailleOpacity(step: number, dot: number): number {
  const behind = (((step - dot) % BRAILLE_LOOP.steps) + BRAILLE_LOOP.steps) % BRAILLE_LOOP.steps;
  return TRAIL[behind] ?? DIM;
}

/** A dot of a braille cell in points: its top left corner and its diameter. */
export interface DotBox {
  /** The number of the dot, 0 to 7, in clockwise order. */
  readonly dot: number;
  readonly left: number;
  readonly top: number;
  readonly size: number;
}

/**
 * The 8 dots of a braille cell of `width` x `height` points. The box of the design is
 * scaled on each axis, and a dot keeps its round shape with the smaller scale.
 */
export function brailleDots(width: number, height: number): DotBox[] {
  const scaleX = width / BOX.width;
  const scaleY = height / BOX.height;
  const size = 2 * DOT_RADIUS * Math.min(scaleX, scaleY);
  return BRAILLE_DOTS.map((centre, dot) => ({
    dot,
    left: centre.x * scaleX - size / 2,
    top: centre.y * scaleY - size / 2,
    size,
  }));
}

/** An interpolation of RN Animated: two lists of the same length. */
export interface StepTable {
  readonly inputRange: number[];
  readonly outputRange: number[];
}

/** The small gap that makes each step of a table a flat line, not a ramp. */
const EDGE = 0.001;

/** A table that holds `values[s]` while the clock value is in [s, s + 1). */
export function stepTable(values: readonly number[]): StepTable {
  const inputRange: number[] = [];
  const outputRange: number[] = [];
  values.forEach((value, step) => {
    inputRange.push(step, step + 1 - EDGE);
    outputRange.push(value, value);
  });
  return { inputRange, outputRange };
}

/** The opacity table of `dot` over the braille loop. */
export function brailleTable(dot: number): StepTable {
  const values: number[] = [];
  for (let step = 0; step < BRAILLE_LOOP.steps; step++) {
    values.push(brailleOpacity(step, dot));
  }
  return stepTable(values);
}

/** The opacity table of a cursor that blinks down to `low`. */
export function blinkTable(low: number): StepTable {
  return stepTable([1, low]);
}

/** The value of `table` at clock value `value`, as RN Animated computes it (for tests). */
export function tableValue(table: StepTable, value: number): number {
  const { inputRange, outputRange } = table;
  let index = 0;
  while (index + 1 < inputRange.length && (inputRange[index + 1] ?? 0) <= value) {
    index += 1;
  }
  const start = inputRange[index] ?? 0;
  const end = inputRange[index + 1];
  const from = outputRange[index] ?? 0;
  const to = outputRange[index + 1];
  if (end === undefined || to === undefined || value <= start) {
    return from;
  }
  return from + ((to - from) * (Math.min(value, end) - start)) / (end - start);
}
