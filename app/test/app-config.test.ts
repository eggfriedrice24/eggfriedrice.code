// The native config of each variant, as `expo config --type introspect` resolves it from
// app.json, app.config.ts and the config plugins. Linux builds no iOS app, so this is the
// check of the Info.plist and the entitlements before an EAS build (probe PB4).
//
// The plugins run in the reverse order of the list, after the static `ios.infoPlist`. A
// plugin option `false` deletes its key, also a key of `ios.infoPlist`, so the test reads
// the result and not the options.
//
// Autolinking links each local module of app/modules by the class in its
// expo-module.config.json. A class that the Swift file does not declare fails only at
// the EAS build, so the test reads both.
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const APP = join(import.meta.dirname, "..");
const resolve = createRequire(join(APP, "package.json")).resolve;
const EXPO_CLI = resolve("expo/bin/cli");
const AUTOLINKING_CLI = resolve("expo/bin/autolinking");

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

/** What a CLI of Expo prints to stdout. */
function run(cli: string, args: readonly string[], env: Record<string, string> = {}): string {
  const result = spawnSync(process.execPath, [cli, ...args], {
    cwd: APP,
    encoding: "utf8",
    env: { ...process.env, ...env, EXPO_OFFLINE: "1", EXPO_NO_TELEMETRY: "1" },
  });
  expect(result.stderr).not.toContain("Error");
  expect(result.status).toBe(0);
  return result.stdout;
}

function introspect(variant: string): IosConfig {
  const args = ["config", "--type", "introspect", "--json"];
  const config: { readonly ios: IosConfig } = JSON.parse(
    run(EXPO_CLI, args, { APP_VARIANT: variant }),
  );
  return config.ios;
}

interface LinkedModule {
  readonly packageName: string;
  readonly pods: readonly { readonly podName: string; readonly podspecDir: string }[];
  readonly modules: readonly { readonly class: string }[];
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

describe("the local modules", () => {
  it(
    "are linked with a pod and the module class that their Swift file declares",
    () => {
      const resolved: { readonly modules: readonly LinkedModule[] } = JSON.parse(
        run(AUTOLINKING_CLI, ["resolve", "-p", "apple", "--json"]),
      );
      const local = resolved.modules.filter((linked) => linked.packageName.startsWith("efr-"));
      expect(local.map((linked) => linked.packageName).sort()).toEqual([
        "efr-device-key",
        "efr-files",
        "efr-speech",
      ]);
      for (const linked of local) {
        const [pod] = linked.pods;
        const [module] = linked.modules;
        expect(pod).toBeDefined();
        expect(module).toBeDefined();
        const swift = readFileSync(
          join(pod?.podspecDir ?? "", `${module?.class ?? ""}.swift`),
          "utf8",
        );
        expect(swift).toContain(`public class ${module?.class ?? ""}: Module`);
      }
    },
    CONFIG_TIMEOUT_MS,
  );
});
