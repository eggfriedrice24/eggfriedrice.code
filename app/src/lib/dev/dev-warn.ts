// Diagnostics for a dev build. A warning shows in the Metro log, so a failure on the
// phone tells which step stopped. A release build and the Node tests write nothing.
//
// Pure TypeScript: React Native defines `__DEV__`, and Node does not.

/** True in a dev build: only then do the warnings show. */
export const DEV_WARNINGS: boolean = typeof __DEV__ === "boolean" && __DEV__;

/** Writes `message` to the log of a dev build, with the prefix "efr:". */
export function devWarn(message: string): void {
  if (DEV_WARNINGS) {
    console.warn(`efr: ${message}`);
  }
}
