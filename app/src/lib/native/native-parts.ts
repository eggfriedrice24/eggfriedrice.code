// The native parts of 0.0.13 that this build has. An older dev build runs the newer
// JavaScript from Metro, so each part shows only when its native modules are in the
// binary, as the effort slider does:
//
// - `attach`: the plus of the composer needs efr-files and both pickers of iOS.
// - `voice`: the microphone needs efr-speech. One line of app/package.json can take that
//   module out of a build.
// - `push`: the row "Notifications" of Settings needs expo-notifications.
//
// A package of Expo whose native module is missing throws when it loads, so the code of a
// part imports its package only when the part is here.

import { requireOptionalNativeModule } from "expo";

import { EfrFiles } from "../../../modules/efr-files";
import { EfrSpeech } from "../../../modules/efr-speech";

/** The native parts that this build has. */
export interface NativeParts {
  readonly attach: boolean;
  readonly voice: boolean;
  readonly push: boolean;
}

/** True when this build has the native module `name`. */
function linked(name: string): boolean {
  return requireOptionalNativeModule(name) !== null;
}

/** Reads the native modules of the build. */
export function nativeParts(): NativeParts {
  return {
    attach: EfrFiles !== null && linked("ExponentImagePicker") && linked("ExpoDocumentPicker"),
    voice: EfrSpeech !== null,
    push: linked("ExpoPushTokenManager"),
  };
}

/** The native parts of this build. They do not change while the app runs. */
export const NATIVE_PARTS: NativeParts = nativeParts();
