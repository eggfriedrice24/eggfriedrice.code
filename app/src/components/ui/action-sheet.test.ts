import { describe, expect, it } from "vitest";

import { sheetAnswer, sheetOptions, type ActionSheet } from "./action-sheet";

const FORGET: ActionSheet = {
  title: "Forget desktop?",
  message:
    "This iPhone deletes its pairing key for desktop and stops showing its conversations. Nothing changes on desktop. To use it again, pair with efr device add.",
  actions: [{ id: "forget", label: "Forget desktop", destructive: true }],
};

describe("an action sheet", () => {
  it("puts cancel last and marks the destructive action", () => {
    expect(sheetOptions(FORGET)).toEqual({
      title: "Forget desktop?",
      message: FORGET.message,
      options: ["Forget desktop", "Cancel"],
      cancelButtonIndex: 1,
      destructiveButtonIndex: [0],
      userInterfaceStyle: "dark",
    });
  });

  it("sends no message key without a message, and takes another cancel label", () => {
    const options = sheetOptions({
      title: "t",
      actions: [
        { id: "a", label: "A" },
        { id: "b", label: "B", destructive: true },
      ],
      cancelLabel: "Keep",
    });
    expect("message" in options).toBe(false);
    expect(options.options).toEqual(["A", "B", "Keep"]);
    expect(options.cancelButtonIndex).toBe(2);
    expect(options.destructiveButtonIndex).toEqual([1]);
  });

  it("answers with the id of the action, and undefined for cancel", () => {
    expect(sheetAnswer(FORGET, 0)).toBe("forget");
    expect(sheetAnswer(FORGET, 1)).toBeUndefined();
    expect(sheetAnswer(FORGET, -1)).toBeUndefined();
  });
});
