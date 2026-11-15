// The Face ID unlock over the whole window (round 9, research/app-lock-code-design.md
// section 4). The rule: while the lock view is not "app", nothing of the app comes on top
// of the overlay.
//
// - `LockOverlay` renders the lock screen or the cover in `FullWindowOverlay` of
//   react-native-screens, so it lies over a form sheet that is open (the model sheet, the
//   approval sheet, All conversations). Its container goes into the window when it
//   mounts, so it mounts only while the view is not "app": an overlay that stayed mounted
//   would lie under each sheet that opens later.
// - `LockGate` mounts the stack of the app at the first time that the app shows. With the
//   unlock off that is the first render. With it on, a cold start shows only the lock
//   screen, and the stack (with the sheet of a link that started the app) builds after the
//   unlock. After that the stack stays mounted under each later lock.
// - The overlay reads no promise of the services, so it never suspends.

import { useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import type { ReactNode } from "react";
import { FullWindowOverlay } from "react-native-screens";

import { lockLine, unlockHint } from "@/lib/lock/owner-check";
import { type AppLock, lockView, selectFailed, selectShown } from "../model/app-lock";
import { ownerFactsQueryOptions } from "../navigation-queries";
import { useAppLock } from "../use-app-services";
import { LockCover, LockScreen } from "./lock-screen";

/** The lock screen with the state of `lock`. */
function WiredLockScreen(props: { readonly lock: AppLock }) {
  const { lock } = props;
  const failed = useSelector(lock.store, selectFailed);
  const method = useQuery(ownerFactsQueryOptions(lock)).data?.method ?? "Face ID";
  return (
    <LockScreen
      line={failed === null ? undefined : lockLine(failed)}
      hint={unlockHint(method, failed)}
      onUnlock={() => lock.unlock()}
    />
  );
}

/** The lock screen or the cover over the whole window, while the app is not open. */
export function LockOverlay() {
  const lock = useAppLock();
  const view = useSelector(lock.store, lockView);
  if (view === "app") {
    return null;
  }
  return (
    <FullWindowOverlay>
      {view === "lock" ? <WiredLockScreen lock={lock} /> : <LockCover />}
    </FullWindowOverlay>
  );
}

export interface LockGateProps {
  readonly children: ReactNode;
}

/** The stack of the app, from the first time that the app shows. */
export function LockGate(props: LockGateProps) {
  const shown = useSelector(useAppLock().store, selectShown);
  return shown ? props.children : null;
}
