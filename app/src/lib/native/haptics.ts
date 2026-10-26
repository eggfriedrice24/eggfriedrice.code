// The haptics of the app. Every haptic goes through this file, because all of them follow
// the Haptics switch of Settings (design item 8.16): "Haptics for taps, copies, scans and
// errors."
//
// The switch is a preference of features/settings, and lib cannot import a feature. So
// the runtime gives this file a function that reads the switch (`setHapticsEnabled`).
// Until it does, haptics are on, which is the default of the switch.

import {
  ImpactFeedbackStyle,
  NotificationFeedbackType,
  impactAsync,
  notificationAsync,
} from "expo-haptics";

/**
 * The haptics of the design (design section 5):
 * - `light`: Jump to latest and a copy.
 * - `medium`: Allow and Deny.
 * - `success`: a scanned pairing code and a paired machine.
 * - `error`: a pairing error, and a failed turn while its conversation is on screen.
 */
export type Haptic = "light" | "medium" | "success" | "error";

let enabled: () => boolean = () => true;

/** Makes every haptic read the Haptics switch through `read`. */
export function setHapticsEnabled(read: () => boolean): void {
  enabled = read;
}

/**
 * Plays `kind` when the Haptics switch is on. It does not wait and never throws: a
 * haptic that fails (an iPhone without the Taptic Engine) changes nothing.
 */
export function haptic(kind: Haptic): void {
  if (!enabled()) {
    return;
  }
  PLAY[kind]().catch(() => undefined);
}

const PLAY: Readonly<Record<Haptic, () => Promise<void>>> = {
  light: () => impactAsync(ImpactFeedbackStyle.Light),
  medium: () => impactAsync(ImpactFeedbackStyle.Medium),
  success: () => notificationAsync(NotificationFeedbackType.Success),
  error: () => notificationAsync(NotificationFeedbackType.Error),
};
