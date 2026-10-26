// The decode bench of the dev builds (plan section 2.3, item 12): it decodes every item
// of the 8 subscribe recordings of efrd many times, in three modes, and reports the cost
// of the first parse and the p50 and p95 per item and per kind. The draft path counts
// most: efrd sends a draft about every 16 ms while a reply streams. The budget is a p95
// under 0.5 ms per draft frame on the iPhone; else the app parses jitless, or the draft
// schemas get slimmer.
//
// The modes: `zod` is the decoder of the app, `jitless` is the same schemas without the
// compiled object parsers of zod (schema-decoder.ts), and `hand` is the hand guard from
// before the schemas (src/lib/protocol/hand-guard.ts).
//
// The bench pauses between chunks of items, so the screen stays live while it runs on
// the phone. The decoders, the clock and the pause come in as parameters. Pure.

import type { Recording } from "./recordings";

/** A mode of the bench. */
export type BenchMode = "zod" | "jitless" | "hand";

/** The modes in the order that the bench runs them. */
export const BENCH_MODES: readonly BenchMode[] = ["zod", "jitless", "hand"];

/** The rounds over all items, as the plan asks. */
export const BENCH_ROUNDS = 100;

/** The budget of a draft frame at p95, in ms. */
export const DRAFT_BUDGET_MS = 0.5;

/** The name of the row of all draft parts together. */
export const DRAFT_ROW = "draft (all parts)";

/** The name of the row of every item. */
export const ALL_ROW = "all items";

/** One item of a recording, parsed from its JSON, with its kind. */
export interface BenchItem {
  /** Such as `event turn_started`, `draft text` or `snapshot`. */
  readonly group: string;
  readonly raw: unknown;
}

/** A decoder: the bench only needs to know that it did not refuse the item. */
export type BenchDecoder = (raw: unknown) => { readonly ok: boolean };

/** The numbers of one row, in ms. */
export interface BenchStats {
  readonly group: string;
  readonly count: number;
  /** The first parse of an item of this row in this mode. */
  readonly firstMs: number;
  readonly meanMs: number;
  readonly p50Ms: number;
  readonly p95Ms: number;
  readonly maxMs: number;
}

/** What one mode gave. */
export interface ModeReport {
  readonly mode: BenchMode;
  /** One row per kind, then the draft row and the row of all items. */
  readonly rows: readonly BenchStats[];
  /** The p95 of a draft frame, in ms. */
  readonly draftP95Ms: number;
  readonly withinBudget: boolean;
  /** The items that the decoder refused. The recordings are valid, so this must be 0. */
  readonly refused: number;
}

/** What the bench gave. */
export interface BenchReport {
  readonly items: number;
  readonly rounds: number;
  readonly modes: readonly ModeReport[];
}

/** The options of a run. */
export interface BenchOptions {
  readonly decoders: Readonly<Record<BenchMode, BenchDecoder>>;
  /** The time in ms, as precise as the platform can. */
  readonly now: () => number;
  readonly rounds?: number;
  /** The modes to run; all by default. */
  readonly modes?: readonly BenchMode[];
  /** Waits a moment, so the screen can draw. Called after each chunk of items. */
  readonly pause?: () => Promise<void>;
  /** The items between two pauses. */
  readonly chunk?: number;
  /** Called after each chunk with the parses done and the parses in all. */
  readonly onProgress?: (done: number, total: number) => void;
}

/** The kind of an item, with the kind of its event or draft part. */
export function groupOf(raw: unknown): string {
  if (typeof raw !== "object" || raw === null) {
    return "not an object";
  }
  const kind: unknown = Reflect.get(raw, "kind");
  const inner = (member: string): unknown => {
    const value: unknown = Reflect.get(raw, member);
    return typeof value === "object" && value !== null ? Reflect.get(value, "kind") : undefined;
  };
  if (kind === "event") {
    return `event ${String(inner("event"))}`;
  }
  if (kind === "draft") {
    return `draft ${String(inner("draft"))}`;
  }
  return String(kind);
}

/** The items of `recordings`, in order. */
export function benchItems(recordings: readonly Recording[]): BenchItem[] {
  const items: BenchItem[] = [];
  for (const recording of recordings) {
    for (const line of recording.lines) {
      const raw: unknown = JSON.parse(line);
      items.push({ group: groupOf(raw), raw });
    }
  }
  return items;
}

/** The value at fraction `p` of the sorted list `sorted`. */
export function quantile(sorted: readonly number[], p: number): number {
  return sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] ?? Number.NaN;
}

/** The numbers of row `group` from its times and its first parse. */
export function statsOf(group: string, times: readonly number[], firstMs: number): BenchStats {
  const sorted = [...times].sort((a, b) => a - b);
  const total = times.reduce((sum, value) => sum + value, 0);
  return {
    group,
    count: times.length,
    firstMs,
    meanMs: times.length === 0 ? Number.NaN : total / times.length,
    p50Ms: quantile(sorted, 0.5),
    p95Ms: quantile(sorted, 0.95),
    maxMs: sorted[sorted.length - 1] ?? Number.NaN,
  };
}

/** Runs the bench over `items`. */
export async function runDecodeBench(
  items: readonly BenchItem[],
  options: BenchOptions,
): Promise<BenchReport> {
  const rounds = options.rounds ?? BENCH_ROUNDS;
  const modes = options.modes ?? BENCH_MODES;
  const chunk = Math.max(1, options.chunk ?? 400);
  const total = items.length * rounds * modes.length;
  let done = 0;
  const reports: ModeReport[] = [];
  for (const mode of modes) {
    const decode = options.decoders[mode];
    const times = new Map<string, number[]>();
    const first = new Map<string, number>();
    let refused = 0;
    let sincePause = 0;
    for (let round = 0; round < rounds; round += 1) {
      for (const item of items) {
        const start = options.now();
        const decoded = decode(item.raw);
        const took = options.now() - start;
        if (!decoded.ok) {
          refused += 1;
        }
        const draft = item.group.startsWith("draft");
        for (const key of draft ? [item.group, DRAFT_ROW, ALL_ROW] : [item.group, ALL_ROW]) {
          const list = times.get(key);
          if (list === undefined) {
            times.set(key, [took]);
          } else {
            list.push(took);
          }
          if (!first.has(key)) {
            first.set(key, took);
          }
        }
        done += 1;
        sincePause += 1;
        if (sincePause >= chunk) {
          sincePause = 0;
          options.onProgress?.(done, total);
          // The pauses keep the screen live. They fall between two parses, so no
          // measured time holds one.
          // oxlint-disable-next-line no-await-in-loop -- the bench waits for the screen on purpose.
          await options.pause?.();
        }
      }
    }
    options.onProgress?.(done, total);
    reports.push(modeReport(mode, times, first, refused));
  }
  return { items: items.length, rounds, modes: reports };
}

function modeReport(
  mode: BenchMode,
  times: ReadonlyMap<string, readonly number[]>,
  first: ReadonlyMap<string, number>,
  refused: number,
): ModeReport {
  const kinds = [...times.keys()]
    .filter((key) => key !== DRAFT_ROW && key !== ALL_ROW)
    .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  const rows = [...kinds, DRAFT_ROW, ALL_ROW]
    .filter((key) => times.has(key))
    .map((key) => statsOf(key, times.get(key) ?? [], first.get(key) ?? Number.NaN));
  const draft = rows.find((row) => row.group === DRAFT_ROW);
  const draftP95Ms = draft?.p95Ms ?? Number.NaN;
  return { mode, rows, draftP95Ms, withinBudget: draftP95Ms < DRAFT_BUDGET_MS, refused };
}
