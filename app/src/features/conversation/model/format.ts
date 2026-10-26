// Small text formats of the screens. They follow the terminal client where it has the
// same field, so the phone and the terminal say the same thing:
//
// - `size` is efr-cli's `format::size`: `512 B`, `3.2 KB`, `1.4 MB`, in thousands, cut
//   and not rounded.
// - `tokens` and `contextGauge` are efr-cli's `format::context`: `ctx 43%`, the tokens as
//   a percent of the limit, rounded down; `success` below 50%, `warning` from 50% and
//   `error` from 90%.

/** How full the model's context is (efr-protocol `ContextUse`). */
export interface ContextUse {
  readonly tokens: number;
  readonly limit: number;
  readonly window: number;
}

/** A size in bytes, as the terminal shows it. */
export function size(bytes: number): string {
  const n = Math.max(0, Math.floor(bytes));
  if (n < 1_000) {
    return `${n} B`;
  }
  if (n < 1_000_000) {
    return `${Math.floor(n / 1_000)}.${Math.floor(n / 100) % 10} KB`;
  }
  if (n < 1_000_000_000) {
    return `${Math.floor(n / 1_000_000)}.${Math.floor(n / 100_000) % 10} MB`;
  }
  return `${Math.floor(n / 1_000_000_000)}.${Math.floor(n / 100_000_000) % 10} GB`;
}

/** A count of tokens: as it is under 1000, `3.2k` under 10000, then `24k` and `1.2M`. */
export function tokens(count: number): string {
  const n = Math.max(0, Math.floor(count));
  if (n < 1_000) {
    return `${n}`;
  }
  if (n < 10_000) {
    return `${Math.floor(n / 1_000)}.${Math.floor(n / 100) % 10}k`;
  }
  if (n < 1_000_000) {
    return `${Math.floor(n / 1_000)}k`;
  }
  return `${Math.floor(n / 1_000_000)}.${Math.floor(n / 100_000) % 10}M`;
}

/** The level of the context gauge, by the role of its colour. */
export type GaugeLevel = "success" | "warning" | "error";

/** The `ctx N%` field. */
export interface ContextGauge {
  readonly percent: number;
  readonly level: GaugeLevel;
  /** `ctx 43%`. */
  readonly text: string;
  /** `89k of 207k tokens`, for the long form and for VoiceOver. */
  readonly detail: string;
}

/** The gauge of `context`, or `undefined` without a limit, which no daemon sends. */
export function contextGauge(context: ContextUse): ContextGauge | undefined {
  if (!(context.limit > 0)) {
    return undefined;
  }
  const percent = Math.floor((Math.max(0, context.tokens) * 100) / context.limit);
  const level: GaugeLevel = percent < 50 ? "success" : percent < 90 ? "warning" : "error";
  return {
    percent,
    level,
    text: `ctx ${percent}%`,
    detail: `${tokens(context.tokens)} of ${tokens(context.limit)} tokens`,
  };
}

/** The last `count` lines of `text`, without a final empty line. */
export function lastLines(text: string, count: number): string[] {
  if (count <= 0 || text.length === 0) {
    return [];
  }
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines.length > 0 && lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines.slice(Math.max(0, lines.length - count));
}

/** `4:05` for 245 seconds; never below `0:00`. */
export function clock(seconds: number): string {
  const total = Math.max(0, Math.ceil(seconds));
  const minutes = Math.floor(total / 60);
  const rest = total % 60;
  return `${minutes}:${rest < 10 ? "0" : ""}${rest}`;
}

/** How long ago `then` was, at `now` (both in ms): `now`, `5 min`, `3 h`, `2 d`. */
export function ago(then: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - then) / 1000));
  if (seconds < 60) {
    return "now";
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes} min`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours} h`;
  }
  return `${Math.floor(hours / 24)} d`;
}

/** The last part of a path, for a short label: `~/p/efr` gives `efr`. */
export function baseName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const at = trimmed.lastIndexOf("/");
  if (trimmed.length === 0) {
    return path.length > 0 ? "/" : "";
  }
  return at < 0 ? trimmed : trimmed.slice(at + 1);
}
