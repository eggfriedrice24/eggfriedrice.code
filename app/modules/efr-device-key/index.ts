// The local Expo module of the device key. Autolinking finds it in app/modules on an EAS
// or Xcode build. Expo Go and a build without the module give null here, and the app then
// cannot pair.

import { requireOptionalNativeModule } from "expo";

import type { EfrDeviceKeyNative } from "./src/EfrDeviceKey.types";

export type { EfrDeviceKeyNative } from "./src/EfrDeviceKey.types";

/** The native module, or null when this build does not have it. */
export const EfrDeviceKey: EfrDeviceKeyNative | null =
  requireOptionalNativeModule<EfrDeviceKeyNative>("EfrDeviceKey");
