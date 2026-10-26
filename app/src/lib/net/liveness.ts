// The liveness rule of the phone connection (R3.9 of the research). React Native JS
// never sees WebSocket pongs, so efrd's WebSocket ping cannot tell the app that the link
// is up. The app counts only frames it decodes:
//
// - After `probeAfterMs` of silence the app sends one small request (a probe), so a live
//   efrd answers with a frame.
// - After `deadAfterMs` of silence the connection is dead: the app closes it and
//   reconnects with the backoff ladder, without waiting for `onclose`.
// - When the app comes back to the foreground after `foregroundReplaceAfterMs` or more in
//   the background, it opens a new socket at once and resubscribes with `after_seq`,
//   because iOS may have taken the old socket's resources.
//
// All functions are pure; the caller passes the time.

/** The settings of the liveness rule. */
export interface LivenessConfig {
  readonly probeAfterMs: number;
  readonly deadAfterMs: number;
  readonly foregroundReplaceAfterMs: number;
}

/** About 2N seconds of silence is dead, for a probe after N = 10 s. */
export const DEFAULT_LIVENESS: LivenessConfig = {
  probeAfterMs: 10_000,
  deadAfterMs: 20_000,
  foregroundReplaceAfterMs: 10_000,
};

/** What the connection must do now. */
export type LivenessVerdict = "alive" | "probe" | "dead";

/** What the app knows about the connection's frames. */
export interface LivenessState {
  /** When the app decoded the last frame, or when the connection opened. */
  readonly lastFrameAt: number;
  /** When the app sent the last probe, or `undefined` when it sent none. */
  readonly probeSentAt: number | undefined;
}

/** The verdict at `now`. A probe goes out once per silence. */
export function liveness(
  state: LivenessState,
  now: number,
  config: LivenessConfig = DEFAULT_LIVENESS,
): LivenessVerdict {
  const silence = now - state.lastFrameAt;
  if (silence >= config.deadAfterMs) {
    return "dead";
  }
  const probed = state.probeSentAt !== undefined && state.probeSentAt >= state.lastFrameAt;
  if (silence >= config.probeAfterMs && !probed) {
    return "probe";
  }
  return "alive";
}

/** The next time at which the verdict can change, so the caller sets one timer. */
export function nextLivenessCheckAt(
  state: LivenessState,
  config: LivenessConfig = DEFAULT_LIVENESS,
): number {
  const probed = state.probeSentAt !== undefined && state.probeSentAt >= state.lastFrameAt;
  return probed ? state.lastFrameAt + config.deadAfterMs : state.lastFrameAt + config.probeAfterMs;
}

/** True when the app must open a new socket now that it is active again. */
export function replaceOnForeground(
  backgroundAt: number | undefined,
  activeAt: number,
  config: LivenessConfig = DEFAULT_LIVENESS,
): boolean {
  return backgroundAt !== undefined && activeAt - backgroundAt >= config.foregroundReplaceAfterMs;
}
