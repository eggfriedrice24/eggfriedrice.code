// The native config of each variant, as `expo config --type introspect` resolves it from
// app.json, app.config.ts and the config plugins. Linux builds no iOS app, so this is the
// check of the Info.plist and the entitlements before an EAS build (probe PB4).
//
// The plugins run in the reverse order of the list, after the static `ios.infoPlist`. A
// plugin option `false` deletes its key, also a key of `ios.infoPlist`, so the test reads
// the result and not the options.
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const APP = join(import.meta.dirname, "..");
const EXPO_CLI = createRequire(join(APP, "package.json")).resolve("expo/bin/cli");

/** The slow part: one run of the Expo CLI for each variant. */
const CONFIG_TIMEOUT_MS = 60_000;

const CAMERA = "efr uses the camera to scan a pairing code and to take photos that you attach.";
const VOICE =
  "efr turns your speech into text on this iPhone. The audio does not leave the iPhone.";
const FACE_ID = "efr checks that it is you when it opens.";

interface IosConfig {
  readonly bundleIdentifier: string;
  readonly infoPlist: Readonly<Record<string, unknown>>;
  readonly entitlements: Readonly<Record<string, unknown>>;
}

function introspect(variant: string): IosConfig {
  const run = spawnSync(process.execPath, [EXPO_CLI, "config", "--type", "introspect", "--json"], {
    cwd: APP,
    encoding: "utf8",
    env: { ...process.env, APP_VARIANT: variant, EXPO_OFFLINE: "1", EXPO_NO_TELEMETRY: "1" },
  });
  expect(run.stderr).not.toContain("Error");
  expect(run.status).toBe(0);
  const config: { readonly ios: IosConfig } = JSON.parse(run.stdout);
  return config.ios;
}

describe.each([
  ["development", "dev.eggfriedrice.efr.dev"],
  ["preview", "dev.eggfriedrice.efr.preview"],
  ["production", "dev.eggfriedrice.efr"],
])("the %s variant", (variant, bundleId) => {
  it(
    "has the usage texts, the push entitlement and no photo library text",
    () => {
      const ios = introspect(variant);
      expect(ios.bundleIdentifier).toBe(bundleId);
      expect(ios.infoPlist).toMatchObject({
        NSCameraUsageDescription: CAMERA,
        NSMicrophoneUsageDescription: VOICE,
        NSSpeechRecognitionUsageDescription: VOICE,
        NSFaceIDUsageDescription: FACE_ID,
      });
      // The system photo picker needs no access to the library.
      expect(ios.infoPlist).not.toHaveProperty("NSPhotoLibraryUsageDescription");
      expect(ios.infoPlist).not.toHaveProperty("NSPhotoLibraryAddUsageDescription");
      // A push only shows a notice: the app has no background mode for it.
      expect(ios.infoPlist).not.toHaveProperty("UIBackgroundModes");
      expect(ios.entitlements).toHaveProperty("aps-environment");
    },
    CONFIG_TIMEOUT_MS,
  );
});
