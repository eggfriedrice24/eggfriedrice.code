// The guards of the native parts. jest-expo mocks the native module of each Expo package
// under the name that the package asks for, so a wrong name reads as a missing part here.
// The local modules of app/modules have no mock: the tests give them one.

import { describe, expect, it, jest } from "@jest/globals";

import type { NativeParts } from "./native-parts";

/** `nativeParts()` of a fresh copy of the module, with `locals` as the local modules. */
function partsWith(locals: { readonly files: boolean; readonly speech: boolean }): NativeParts {
  let parts: NativeParts | undefined;
  jest.isolateModules(() => {
    jest.doMock("../../../modules/efr-files", () => ({ EfrFiles: locals.files ? {} : null }));
    jest.doMock("../../../modules/efr-speech", () => ({ EfrSpeech: locals.speech ? {} : null }));
    const module: typeof import("./native-parts") = require("./native-parts");
    parts = module.nativeParts();
  });
  if (parts === undefined) {
    throw new Error("native-parts did not load");
  }
  return parts;
}

describe("nativeParts", () => {
  it("finds expo-notifications and the two pickers by the names of their packages", () => {
    expect(partsWith({ files: true, speech: true })).toEqual({
      attach: true,
      voice: true,
      push: true,
    });
  });

  it("hides the plus without efr-files and the microphone without efr-speech", () => {
    expect(partsWith({ files: false, speech: false })).toEqual({
      attach: false,
      voice: false,
      push: true,
    });
  });
});
