// The accounts of a machine as the phone names them, and how full their limits are
// (S-ModelSheet, MS-SignedOut, AC-Out, AC-Limits).
//
// - An account is a provider id of efrd: the kind, then a dot and the id for a second
//   account of the kind, such as `openai-subscription.b`. The phone names it as the
//   sheet header does: the kind ("Claude API", "OpenAI API", "ChatGPT plan"), then the
//   label of `[accounts]` or the id. The default account of a kind has no id, so it is
//   the kind alone. Never an email, never a key hint.
// - A short name stands where the kind is clear from the model: the label or the id
//   ("b"), else the kind ("ChatGPT plan").
// - Limits: efrd sends what the last call of the account told. A value whose reset time
//   passed counts no more ("limit reached" clears after its reset time). The phone shows
//   a limit only from 80 %, and "limit reached" at 100 % or when a limit stops the
//   account. No pooled view.
//
// Pure TypeScript.

import type {
  AccountLimits,
  CatalogStatus,
  LimitKind,
  LimitReached,
  LimitWindow,
  ModelInfo,
} from "@/lib/protocol/generated";
import { clockTime } from "./durations";

const KIND_NAMES: Readonly<Record<string, string>> = {
  "anthropic-api": "Claude API",
  "openai-api": "OpenAI API",
  "openai-subscription": "ChatGPT plan",
};

/** The word of `efr login` for each kind, as the terminal prints the command. */
const LOGIN_WORDS: Readonly<Record<string, string>> = {
  "anthropic-api": "anthropic",
  "openai-api": "openai-api",
  "openai-subscription": "openai",
};

/** The kind of account `provider`, such as `openai-subscription`. */
export function accountKind(provider: string): string {
  const dot = provider.indexOf(".");
  return dot < 0 ? provider : provider.slice(0, dot);
}

/** The id of account `provider` in its kind, such as `b`; none for the default account. */
export function accountId(provider: string): string | undefined {
  const dot = provider.indexOf(".");
  return dot < 0 || dot === provider.length - 1 ? undefined : provider.slice(dot + 1);
}

/** The name of the kind of `provider`: "ChatGPT plan"; an unknown kind as it is. */
export function kindName(provider: string): string {
  const kind = accountKind(provider);
  return KIND_NAMES[kind] ?? kind;
}

/** The label or the id of `provider`, or none for the default account without a label. */
function ownName(provider: string, label: string | null | undefined): string | undefined {
  const own = label?.trim();
  return own === undefined || own === "" ? accountId(provider) : own;
}

/** The name of the account in the sheet header: "Claude API · work", "ChatGPT plan". */
export function accountName(provider: string, label?: string | null): string {
  const own = ownName(provider, label);
  return own === undefined ? kindName(provider) : `${kindName(provider)} · ${own}`;
}

/** The short name of the account: "b", "work", or "ChatGPT plan" for a default account. */
export function accountShort(provider: string, label?: string | null): string {
  return ownName(provider, label) ?? kindName(provider);
}

/** The default account of each kind inside a sentence, as the terminal says it. */
const DEFAULT_SUBJECTS: Readonly<Record<string, string>> = {
  "anthropic-api": "the Claude API key",
  "openai-api": "the OpenAI API key",
  "openai-subscription": "the ChatGPT plan",
};

/**
 * The account as the subject of a sentence, as the terminal names it (errata 12): its
 * label or id ("b", "Family"), else the default account of its kind ("the ChatGPT plan").
 */
export function accountSubject(provider: string, label?: string | null): string {
  return ownName(provider, label) ?? DEFAULT_SUBJECTS[provider] ?? `the account ${provider}`;
}

/** The command that logs in to the account again: `efr login anthropic --account personal`. */
export function loginCommand(provider: string): string {
  const kind = accountKind(provider);
  const id = accountId(provider);
  const word = LOGIN_WORDS[kind] ?? kind;
  return id === undefined ? `efr login ${word}` : `efr login ${word} --account ${id}`;
}

/** The logo of the account's models: the Claude spark, the OpenAI mark, or none. */
export type ProviderLogoKind = "claude" | "openai" | "none";

/** The logo of the models of `provider`. */
export function providerLogo(provider: string | undefined): ProviderLogoKind {
  const kind = provider === undefined ? "" : accountKind(provider);
  if (kind.startsWith("anthropic")) {
    return "claude";
  }
  if (kind.startsWith("openai")) {
    return "openai";
  }
  return "none";
}

/** The label of `provider` in the catalogs of `models.list`. */
export function labelOf(
  catalogs: readonly CatalogStatus[],
  provider: string | undefined,
): string | undefined {
  if (provider === undefined) {
    return undefined;
  }
  return catalogs.find((catalog) => catalog.provider === provider)?.label ?? undefined;
}

/** True when more than one account lists model `id`: a name of the model names the account. */
export function runsOnMany(models: readonly ModelInfo[], id: string): boolean {
  let first: string | null | undefined;
  for (const model of models) {
    if (model.id !== id) {
      continue;
    }
    const provider = model.provider ?? null;
    if (first === undefined) {
      first = provider;
    } else if (provider !== first) {
      return true;
    }
  }
  return false;
}

/** The name of a usage window from its length: "5-hour", "daily", "weekly", "monthly". */
export function windowName(minutes: number | null | undefined): string {
  if (minutes === undefined || minutes === null || minutes <= 0) {
    return "window";
  }
  if (minutes === 1440) {
    return "daily";
  }
  if (minutes === 10_080) {
    return "weekly";
  }
  if (minutes >= 40_320 && minutes <= 44_640) {
    return "monthly";
  }
  if (minutes % 1440 === 0) {
    return `${String(minutes / 1440)}-day`;
  }
  if (minutes % 60 === 0) {
    return `${String(minutes / 60)}-hour`;
  }
  return `${String(minutes)}-minute`;
}

const LIMIT_WORDS: Readonly<Record<LimitKind, string>> = {
  usage: "usage",
  requests: "requests",
  tokens: "tokens",
  input_tokens: "input tokens",
  output_tokens: "output tokens",
  too_many_requests: "too many requests",
  credit: "credit",
  spend_limit: "spend limit",
  unknown: "limit",
};

/** The name of a limit of kind `kind`, such as "requests". */
export function limitWords(kind: LimitKind): string {
  return LIMIT_WORDS[kind];
}

/** The limits that still count at `now`: a value after its reset time is gone. */
export interface LimitsNow {
  readonly observedAt: number | undefined;
  readonly windows: readonly LimitWindow[];
  readonly reached: LimitReached | undefined;
}

/**
 * An ISO time of efrd in milliseconds, or `undefined` when it is missing or does not
 * parse. The model sheet, Settings and the turn end all read a reset through it, so they
 * agree.
 */
export function isoMs(time: string | null | undefined): number | undefined {
  if (time === undefined || time === null) {
    return undefined;
  }
  const parsed = Date.parse(time);
  return Number.isNaN(parsed) ? undefined : parsed;
}

function passed(resetsAt: string | null | undefined, now: number): boolean {
  const at = isoMs(resetsAt);
  return at !== undefined && at <= now;
}

/** The limits of `limits` that count at `now`; undefined when none does. */
export function limitsNow(
  limits: AccountLimits | null | undefined,
  now: number,
): LimitsNow | undefined {
  if (limits === undefined || limits === null) {
    return undefined;
  }
  const windows = (limits.windows ?? []).filter((window) => !passed(window.resets_at, now));
  const held = limits.reached ?? undefined;
  const reached = held !== undefined && !passed(held.resets_at, now) ? held : undefined;
  if (windows.length === 0 && reached === undefined) {
    return undefined;
  }
  return { observedAt: isoMs(limits.observed_at), windows, reached };
}

/** How full an account is, as the sheet header and Settings show it. */
export type LimitLevel =
  | { readonly kind: "none" }
  /** From 80 %: the warning glyph and "5-hour 92%". */
  | { readonly kind: "warn"; readonly window: string; readonly percent: number }
  /** At 100 %, or a limit stops the account: "limit reached", with its reset when known. */
  | { readonly kind: "reached"; readonly resetsAt: number | undefined };

/** A limit shows from this percent on (AC-Limits). */
export const LIMIT_WARN_PERCENT = 80;

/** The limit shows in the status line of a running turn from this percent on. */
export const LIMIT_STATUS_PERCENT = 90;

/** The window with the highest percent, or undefined without a window. */
export function tightestWindow(windows: readonly LimitWindow[]): LimitWindow | undefined {
  let tightest: LimitWindow | undefined;
  for (const window of windows) {
    if (tightest === undefined || window.used_percent > tightest.used_percent) {
      tightest = window;
    }
  }
  return tightest;
}

/** The level of `limits` at `now`; a level under `from` percent is none. */
export function limitLevel(
  limits: AccountLimits | null | undefined,
  now: number,
  from = LIMIT_WARN_PERCENT,
): LimitLevel {
  const held = limitsNow(limits, now);
  if (held === undefined) {
    return { kind: "none" };
  }
  if (held.reached !== undefined) {
    return { kind: "reached", resetsAt: isoMs(held.reached.resets_at) };
  }
  const full = held.windows.find((window) => window.used_percent >= 100);
  if (full !== undefined) {
    return { kind: "reached", resetsAt: isoMs(full.resets_at) };
  }
  const tightest = tightestWindow(held.windows);
  if (tightest === undefined || tightest.used_percent < from) {
    return { kind: "none" };
  }
  return { kind: "warn", window: windowName(tightest.minutes), percent: tightest.used_percent };
}

/**
 * The limit in the status line of a running turn (AC-Limits 3): the tightest window from
 * 90 %, such as "5-hour 92%"; undefined below. Only the windows that still count at
 * `now` (`limitsNow`): a window whose reset passed says nothing.
 */
export function statusLimit(
  limits: AccountLimits | null | undefined,
  now: number,
): string | undefined {
  const tightest = tightestWindow(limitsNow(limits, now)?.windows ?? []);
  if (tightest === undefined || tightest.used_percent < LIMIT_STATUS_PERCENT) {
    return undefined;
  }
  return `${windowName(tightest.minutes)} ${String(tightest.used_percent)}%`;
}

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** The time of a reset: "14:20" today, "Mon 09:00" on another day. */
export function resetClock(at: number, now: number): string {
  const date = new Date(at);
  const today = new Date(now);
  const sameDay =
    date.getFullYear() === today.getFullYear() &&
    date.getMonth() === today.getMonth() &&
    date.getDate() === today.getDate();
  return sameDay ? clockTime(at) : `${DAYS[date.getDay()] ?? ""} ${clockTime(at)}`;
}

/** When a limit resets: "resets 14:20" today, "resets Mon 09:00" on another day. */
export function resetText(at: number, now: number): string {
  return `resets ${resetClock(at, now)}`;
}

/** How old a value is: "just now", "12m ago", "3h ago", "2d ago". */
export function ageText(then: number, now: number): string {
  const minutes = Math.floor(Math.max(0, now - then) / 60_000);
  if (minutes < 1) {
    return "just now";
  }
  if (minutes < 60) {
    return `${String(minutes)}m ago`;
  }
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${String(hours)}h ago` : `${String(Math.floor(hours / 24))}d ago`;
}

/** A value older than this says its age in the sheet header, as `efr status` does. */
export const OLD_LIMIT_MS = 5 * 60_000;

/** The words after "limit reached": the reset and, for an old value, its age. */
export function reachedWords(
  level: Extract<LimitLevel, { kind: "reached" }>,
  observedAt: number | undefined,
  now: number,
): string {
  const parts = ["limit reached"];
  if (level.resetsAt !== undefined) {
    parts.push(resetText(level.resetsAt, now));
  }
  if (observedAt !== undefined && now - observedAt > OLD_LIMIT_MS) {
    parts.push(ageText(observedAt, now));
  }
  return parts.join(" · ");
}
