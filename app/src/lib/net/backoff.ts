// The reconnect backoff ladder of t3code (R3.9 of the research): before attempt `n`
// (from 0) the app waits `ceiling / 2 + random * ceiling / 2`, with
// `ceiling = min(300 s, 1 s * 2^(n + 1))`. A connection that stays up for 30 s resets the
// ladder. The random value is a parameter, so tests choose it.

/** The settings of the ladder. */
export interface BackoffConfig {
  readonly baseMs: number;
  readonly maxMs: number;
  /** A connection that lasts this long resets the ladder. */
  readonly resetAfterMs: number;
}

/** The ladder of t3code. */
export const DEFAULT_BACKOFF: BackoffConfig = {
  baseMs: 1_000,
  maxMs: 300_000,
  resetAfterMs: 30_000,
};

/** The largest wait before attempt `attempt`. */
export function backoffCeilingMs(attempt: number, config: BackoffConfig = DEFAULT_BACKOFF): number {
  // Past 2^30 the ceiling is far above any maximum; the bound keeps the power finite.
  const exponent = Math.min(Math.max(attempt, 0) + 1, 30);
  return Math.min(config.maxMs, config.baseMs * 2 ** exponent);
}

/** The wait before attempt `attempt`, for a `random` value in [0, 1). */
export function backoffDelayMs(
  attempt: number,
  random: number,
  config: BackoffConfig = DEFAULT_BACKOFF,
): number {
  const ceiling = backoffCeilingMs(attempt, config);
  const share = Math.min(Math.max(random, 0), 1);
  return Math.floor(ceiling / 2 + (share * ceiling) / 2);
}

/** Where the ladder is. */
export interface Backoff {
  /** The number of the next attempt, from 0. */
  readonly attempt: number;
  /** When the current connection came up, or `undefined` while there is none. */
  readonly connectedAt: number | undefined;
}

/** The ladder before the first attempt. */
export const INITIAL_BACKOFF: Backoff = { attempt: 0, connectedAt: undefined };

/** The connection came up at `now`. */
export function backoffConnected(state: Backoff, now: number): Backoff {
  return { ...state, connectedAt: now };
}

/**
 * The connection went down at `now`, or an attempt failed. Returns the new state and the
 * wait before the next attempt.
 */
export function backoffFailed(
  state: Backoff,
  now: number,
  random: number,
  config: BackoffConfig = DEFAULT_BACKOFF,
): { readonly state: Backoff; readonly delayMs: number } {
  const stable = state.connectedAt !== undefined && now - state.connectedAt >= config.resetAfterMs;
  const attempt = stable ? 0 : state.attempt;
  return {
    state: { attempt: attempt + 1, connectedAt: undefined },
    delayMs: backoffDelayMs(attempt, random, config),
  };
}
