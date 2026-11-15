// The links from outside (`efr://pair`, `efr://conversations`, any route) on their way to
// the router. A link while the app runs waits until the Face ID unlock shows the app, so
// no sheet of a link opens over the lock screen or the cover. A link that starts the app
// goes at once: the stack mounts only after the unlock (LockGate), and opens on it then.

import { linkGate } from "@/features/navigation/model/link-gate";

export function redirectSystemPath(event: {
  readonly path: string;
  readonly initial: boolean;
}): string | Promise<string> {
  return event.initial ? event.path : linkGate.wait(event.path);
}
