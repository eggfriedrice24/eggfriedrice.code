// The platform adapters of copy and haptics, with expo-clipboard and expo-haptics
// replaced by fakes.

import { beforeEach, describe, expect, it, jest } from "@jest/globals";
import { setStringAsync } from "expo-clipboard";
import {
  ImpactFeedbackStyle,
  NotificationFeedbackType,
  impactAsync,
  notificationAsync,
} from "expo-haptics";

import { copyText } from "./clipboard";
import { haptic, setHapticsEnabled } from "./haptics";

jest.mock("expo-clipboard", () => ({ setStringAsync: jest.fn() }));
jest.mock("expo-haptics", () => ({
  ImpactFeedbackStyle: { Light: "light", Medium: "medium" },
  NotificationFeedbackType: { Success: "success", Error: "error" },
  impactAsync: jest.fn(() => Promise.resolve()),
  notificationAsync: jest.fn(() => Promise.resolve()),
}));

beforeEach(() => {
  jest.clearAllMocks();
  setHapticsEnabled(() => true);
});

describe("copyText", () => {
  it("puts the text on the clipboard with expo-clipboard", async () => {
    jest.mocked(setStringAsync).mockResolvedValue(true);
    await expect(copyText("efr device add")).resolves.toBe(true);
    expect(setStringAsync).toHaveBeenCalledWith("efr device add");
  });

  it("gives false and never rejects when the copy fails", async () => {
    jest.mocked(setStringAsync).mockRejectedValue(new Error("no pasteboard"));
    await expect(copyText("x")).resolves.toBe(false);
  });
});

describe("haptic", () => {
  it("plays the impact and notification haptics of the design", () => {
    haptic("light");
    haptic("medium");
    haptic("success");
    haptic("error");
    expect(jest.mocked(impactAsync).mock.calls).toEqual([
      [ImpactFeedbackStyle.Light],
      [ImpactFeedbackStyle.Medium],
    ]);
    expect(jest.mocked(notificationAsync).mock.calls).toEqual([
      [NotificationFeedbackType.Success],
      [NotificationFeedbackType.Error],
    ]);
  });

  it("plays nothing while the Haptics switch is off", () => {
    let on = false;
    setHapticsEnabled(() => on);
    haptic("medium");
    haptic("error");
    expect(impactAsync).not.toHaveBeenCalled();
    expect(notificationAsync).not.toHaveBeenCalled();
    on = true;
    haptic("medium");
    expect(impactAsync).toHaveBeenCalledTimes(1);
  });

  it("does not throw when the Taptic Engine fails", async () => {
    jest.mocked(impactAsync).mockRejectedValueOnce(new Error("unavailable"));
    expect(() => haptic("light")).not.toThrow();
    await Promise.resolve();
  });
});
