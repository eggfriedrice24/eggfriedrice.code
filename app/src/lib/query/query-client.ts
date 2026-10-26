// The QueryClient of the app and its defaults (plan section 2.2).
//
// - `networkMode: "always"`: NetInfo tells only whether the phone has a network, not
//   whether efrd answers, so Query never pauses a request. A request to a machine that
//   is not connected fails at once with a `ConnectionError` of kind `offline`, and the
//   supervisor invalidates the machine's queries when it is ready again.
// - Retries: only for an error that can pass (a lost socket, `busy`). Never for
//   `offline`, for an error that needs the user (pair again, update), or for a refusal
//   of efrd such as `not_found`.
// - Mutations never retry on their own. A write carries a `command_id`, so the user can
//   send it again with the same variables and efrd runs it only once.

import { QueryClient } from "@tanstack/react-query";

import { ConnectionError, ServerError, retryAfterMs } from "@/lib/net/errors";
import type { ErrorCode } from "@/lib/protocol/generated";

/** How long a result stays fresh. */
export const STALE_TIME_MS = 20_000;

/** The most retries of one query fetch. */
export const MAX_QUERY_RETRIES = 2;

/** The longest wait between two tries. */
export const MAX_RETRY_DELAY_MS = 30_000;

/** The error codes of efrd after which a new try can succeed. */
const RETRY_CODES: ReadonlySet<ErrorCode> = new Set(["busy", "cancelled", "internal"]);

/**
 * True when Query tries a failed fetch again. `failureCount` counts the failures before
 * this one, as Query counts them (0 on the first failure).
 */
export function shouldRetryQuery(failureCount: number, error: unknown): boolean {
  if (failureCount >= MAX_QUERY_RETRIES) {
    return false;
  }
  if (error instanceof ConnectionError) {
    return error.kind !== "offline" && error.recovery === "retry";
  }
  if (error instanceof ServerError) {
    return RETRY_CODES.has(error.body.code);
  }
  return false;
}

/**
 * The wait before the next try: the wait that efrd asked for, or 1 s, 2 s, 4 s and so
 * on, at most `MAX_RETRY_DELAY_MS`. `failureCount` is as in `shouldRetryQuery`.
 */
export function queryRetryDelay(failureCount: number, error: unknown): number {
  if (error instanceof ServerError) {
    const asked = retryAfterMs(error.body);
    if (asked !== undefined) {
      return Math.min(asked, MAX_RETRY_DELAY_MS);
    }
  }
  if (error instanceof ConnectionError && error.details.retryAfterMs !== undefined) {
    return Math.min(error.details.retryAfterMs, MAX_RETRY_DELAY_MS);
  }
  return Math.min(1000 * 2 ** Math.max(0, failureCount), MAX_RETRY_DELAY_MS);
}

/** A new QueryClient with the defaults of the app. */
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        networkMode: "always",
        staleTime: STALE_TIME_MS,
        retry: shouldRetryQuery,
        retryDelay: queryRetryDelay,
      },
      mutations: {
        networkMode: "always",
        retry: false,
      },
    },
  });
}
