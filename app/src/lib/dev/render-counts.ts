// The render counter of dev builds (plan section 6, item 5): how often a part of a screen
// commits, and how long React spent on it. A React `Profiler` around the part
// (render-probe.tsx) records each commit here.
//
// The counts are plain fields, and a record notifies nobody: a screen that shows them
// reads them on its own timer. So the counter adds no render of its own to the part
// that it measures. Pure.

/** The counts of one part. */
export interface RenderSample {
  /** The commits of the part since the last reset, the first mount included. */
  readonly commits: number;
  /** The commits that mounted the part. */
  readonly mounts: number;
  /** The time that React spent on the part in its last commit, in ms. */
  readonly lastMs: number;
  /** The time over all commits, in ms. */
  readonly totalMs: number;
  /** The longest commit, in ms. */
  readonly maxMs: number;
  /** The clock time of the first commit after the reset, in ms. */
  readonly since: number;
}

/** The phase of a commit, as the Profiler names it. */
export type CommitPhase = "mount" | "update" | "nested-update";

/** The counts of every part that a probe measures. */
export interface RenderCounts {
  /** Records one commit of part `id` that took `actualMs`, at clock time `at`. */
  record(id: string, phase: CommitPhase, actualMs: number, at: number): void;
  /** The counts now, by part, sorted by name. */
  snapshot(): readonly (readonly [string, RenderSample])[];
  /** Forgets every count. */
  reset(): void;
}

/** A new, empty counter. */
export function createRenderCounts(): RenderCounts {
  const samples = new Map<string, RenderSample>();
  return {
    record(id, phase, actualMs, at) {
      const before = samples.get(id);
      samples.set(id, {
        commits: (before?.commits ?? 0) + 1,
        mounts: (before?.mounts ?? 0) + (phase === "mount" ? 1 : 0),
        lastMs: actualMs,
        totalMs: (before?.totalMs ?? 0) + actualMs,
        maxMs: Math.max(before?.maxMs ?? 0, actualMs),
        since: before?.since ?? at,
      });
    },
    snapshot() {
      return [...samples.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    },
    reset() {
      samples.clear();
    },
  };
}

/** The commits per second of `sample` at clock time `now`. */
export function commitsPerSecond(sample: RenderSample, now: number): number {
  const seconds = (now - sample.since) / 1000;
  return seconds <= 0 ? 0 : sample.commits / seconds;
}

/** The counter of the app. */
export const renderCounts: RenderCounts = createRenderCounts();
