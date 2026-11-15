import { describe, expect, it } from "vitest";

import { notSentLine } from "./owner-check";

describe("the not-sent line", () => {
  it("says nothing for a pass", () => {
    expect(notSentLine({ kind: "passed" })).toBeUndefined();
  });

  it("names the check of the iPhone and how to try again (AU-Auto 5)", () => {
    expect(notSentLine({ kind: "failed", method: "Face ID" })).toEqual({
      head: "Not sent.",
      rest: "Face ID did not pass. Tap Send to try again.",
    });
    expect(notSentLine({ kind: "cancelled", method: "Face ID" })?.rest).toBe(
      "You cancelled Face ID. Tap Send to try again.",
    );
    expect(notSentLine({ kind: "locked", method: "Touch ID" })?.rest).toBe(
      "Touch ID is locked. Unlock the iPhone, then tap Send.",
    );
    expect(notSentLine({ kind: "failed", method: "the passcode" })?.rest).toBe(
      "The passcode did not pass. Tap Send to try again.",
    );
    expect(notSentLine({ kind: "no_passcode" })?.rest).toBe(
      "Auto needs a passcode on this iPhone. Set one, or send in cautious.",
    );
  });
});
