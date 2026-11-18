// The local Expo module of the attached files. Autolinking finds it in app/modules on an
// EAS or Xcode build. A build without the module gives null here, and the app then shows
// no plus button.

import { requireOptionalNativeModule } from "expo";

import type { EfrFilesNative } from "./src/EfrFiles.types";

export type { EfrFilesNative, FileInfo, PreparedImage } from "./src/EfrFiles.types";

/** The native module, or null when this build does not have it. */
export const EfrFiles: EfrFilesNative | null =
  requireOptionalNativeModule<EfrFilesNative>("EfrFiles");
