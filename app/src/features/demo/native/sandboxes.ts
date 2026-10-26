// The sandboxes of the gallery (model/demo-services.ts) on the demo server of the app:
// one per board, made again for each visit, so a board starts as the gallery drew it
// even after a Forget in the last visit. The sandbox of the visit before stops.

import { animationFrames } from "@/lib/store/frame-store";
import type { DemoMachineKey } from "../model/demo-machines";
import { sandboxServices, type Sandbox } from "../model/demo-services";
import { demoServer } from "./demo-machines";

const sandboxes = new Map<string, { readonly visit: string; readonly sandbox: Promise<Sandbox> }>();

/** Reports an error that the sandbox goes on after. Only a dev build shows it. */
function report(error: unknown): void {
  if (__DEV__) {
    console.warn(error);
  }
}

/** The sandbox of board `board` for visit `visit`, with `machines` paired. */
export function sandboxFor(
  board: string,
  visit: string,
  machines: readonly DemoMachineKey[],
): Promise<Sandbox> {
  const known = sandboxes.get(board);
  if (known !== undefined && known.visit === visit) {
    return known.sandbox;
  }
  void known?.sandbox.then((old) => old.services.dispose(), report);
  const sandbox = sandboxServices({
    server: demoServer,
    machines,
    schedule: animationFrames,
    onError: report,
  });
  sandboxes.set(board, { visit, sandbox });
  return sandbox;
}
