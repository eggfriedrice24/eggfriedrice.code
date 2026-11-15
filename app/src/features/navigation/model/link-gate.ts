// The gate of the links from outside (`efr://pair`, `efr://conversations`, any route):
// while the Face ID unlock shows the lock screen or the cover, a link waits, so no sheet
// of a link opens over the lock. `+native-intent.ts` gives each link to `wait`; the
// services give the gate the lock of the app with `follow`.
//
// iOS can give a link before the app is "active", while the cover still shows, so the
// gate waits for the view "app" (`whenShown`), not only for "not locked".
//
// Pure TypeScript.

import type { AppLock } from "./app-lock";

/** The gate of the links. */
export interface LinkGate {
  /** Resolves with `path` when the app shows. Before `follow`, it waits. */
  wait(path: string): Promise<string>;
  /** Gives the gate the lock of the app. The services call it one time. */
  follow(lock: Pick<AppLock, "whenShown">): void;
}

/** `path` once `lock` shows the app. */
function pass(lock: Pick<AppLock, "whenShown">, path: string): Promise<string> {
  return lock.whenShown().then(() => path);
}

/** Makes a gate that holds each link until it follows a lock that shows the app. */
export function createLinkGate(): LinkGate {
  let lock: Pick<AppLock, "whenShown"> | undefined;
  let held: { readonly path: string; readonly resolve: (path: string) => void }[] = [];
  return {
    wait(path: string): Promise<string> {
      if (lock !== undefined) {
        return pass(lock, path);
      }
      return new Promise((resolve) => {
        held.push({ path, resolve });
      });
    },
    follow(next: Pick<AppLock, "whenShown">): void {
      lock = next;
      const waiting = held;
      held = [];
      for (const link of waiting) {
        void pass(next, link.path).then(link.resolve);
      }
    },
  };
}

/** The gate of the app. `+native-intent.ts` and the root layout share it. */
export const linkGate: LinkGate = createLinkGate();
