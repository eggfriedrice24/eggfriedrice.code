// The QR scan of a pairing code, with the system scanner of expo-camera
// (`CameraView.launchScanner`, VisionKit's DataScannerViewController on iOS 16 and later).
//
// The system scanner reports each code it sees and keeps scanning. The scan ends at the
// first code that is an `efr://pair` URL; other codes are skipped. The scanner does not
// tell the app when the user closes it, so the screen keeps a cancel button under it and
// cancels the scan with the signal.

import type { CancelSignal } from "./pair";
import { isPairingUrl } from "./pairing-url";

/** The functions of expo-camera that the scan uses. */
export interface ScannerApi {
  /** False when this phone has no system scanner (before iOS 16, or no camera). */
  readonly available: boolean;
  /** Asks for the camera permission. Gives true when the app may use the camera. */
  requestPermission(): Promise<boolean>;
  launch(): Promise<void>;
  dismiss(): Promise<void>;
  /** Calls `listener` with the text of each code that the scanner sees. */
  onScanned(listener: (data: string) => void): { remove(): void };
}

/** The result of a scan. */
export type ScanResult =
  | { readonly ok: true; readonly text: string }
  | { readonly ok: false; readonly reason: "unavailable" | "permission" | "cancelled" };

/** Scans for a pairing code until one is seen or `signal` cancels. */
export async function scanPairingCode(api: ScannerApi, signal?: CancelSignal): Promise<ScanResult> {
  if (!api.available) {
    return { ok: false, reason: "unavailable" };
  }
  if (!(await api.requestPermission())) {
    return { ok: false, reason: "permission" };
  }
  if (signal?.cancelled === true) {
    return { ok: false, reason: "cancelled" };
  }
  return new Promise<ScanResult>((resolve) => {
    let done = false;
    let subscription: { remove(): void } | undefined;
    let removeCancel: (() => void) | undefined;
    const finish = (result: ScanResult): void => {
      if (done) {
        return;
      }
      done = true;
      subscription?.remove();
      removeCancel?.();
      // NOTE: the scanner may already be gone (the user closed it); a failed dismiss
      // changes nothing.
      api.dismiss().catch(() => {});
      resolve(result);
    };
    subscription = api.onScanned((data) => {
      if (isPairingUrl(data)) {
        finish({ ok: true, text: data });
      }
    });
    removeCancel = signal?.onCancel(() => finish({ ok: false, reason: "cancelled" }));
    if (done) {
      subscription.remove();
      removeCancel?.();
      return;
    }
    api.launch().catch(() => finish({ ok: false, reason: "unavailable" }));
  });
}
