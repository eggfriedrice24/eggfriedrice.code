// The clock of the decode bench: `performance.now()` of React Native, which counts
// fractions of a millisecond. The types of React Native do not declare `performance`, so
// this file reads it from the global object and checks it first. Without it, the bench
// falls back to `Date.now()`, which counts whole milliseconds only.

interface HighResolutionClock {
  now(): number;
}

function isClock(value: unknown): value is HighResolutionClock {
  return (
    typeof value === "object" && value !== null && typeof Reflect.get(value, "now") === "function"
  );
}

/** A function that gives the time in ms, as precise as the platform can. */
export function highResolutionNow(): { readonly now: () => number; readonly precise: boolean } {
  const performance: unknown = Reflect.get(globalThis, "performance");
  if (isClock(performance)) {
    return { now: () => performance.now(), precise: true };
  }
  return { now: () => Date.now(), precise: false };
}
