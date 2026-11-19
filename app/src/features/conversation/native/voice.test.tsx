// The guard of the native side of voice: the effects exist only when the build has the part.

import { beforeEach, describe, expect, it, jest } from "@jest/globals";

import { haptic } from "@/lib/native/haptics";
import { nativeVoice } from "./voice";

const mockParts = { attach: false, voice: true, push: false };
const mockSpeech = {};
// Getters: the adapter reads the parts and the module when it runs, after this file set
// them.
jest.mock("@/lib/native/native-parts", () => ({
  get NATIVE_PARTS() {
    return mockParts;
  },
}));
jest.mock("../../../../modules/efr-speech", () => ({
  get EfrSpeech() {
    return mockSpeech;
  },
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

beforeEach(() => {
  mockParts.voice = true;
  jest.clearAllMocks();
});

describe("nativeVoice", () => {
  it("is null in a build without efr-speech", () => {
    mockParts.voice = false;
    expect(nativeVoice()).toBeNull();
  });

  it("gives efr-speech and a light haptic", () => {
    const voice = nativeVoice();
    expect(voice?.speech).toBe(mockSpeech);
    voice?.haptic();
    expect(haptic).toHaveBeenCalledWith("light");
  });
});
