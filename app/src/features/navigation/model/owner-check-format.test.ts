import { describe, expect, it } from "vitest";

import {
  autoNeedsLine,
  checkReason,
  lockLine,
  NO_PASSCODE_ROW_LINE,
  noPasscodeNotice,
  turnOnTitle,
  unlockHint,
  unlockName,
  unlockRowLine,
} from "./owner-check-format";

describe("the words of the Face ID unlock (Settings, S-Lock, S-ComposerR7 round 9)", () => {
  it("names the switch after what the iPhone checks first (D6)", () => {
    expect(unlockName("Face ID")).toBe("Face ID unlock");
    expect(unlockName("Touch ID")).toBe("Touch ID unlock");
    expect(unlockName("the passcode")).toBe("Passcode unlock");
  });

  it("gives iOS the reason of each check", () => {
    expect(checkReason("unlock", "Face ID")).toBe("Unlock efr");
    expect(checkReason("turn_on", "Face ID")).toBe("Turn on Face ID unlock");
    expect(checkReason("turn_off", "the passcode")).toBe("Turn off Passcode unlock");
  });

  it("says on the lock screen why the check did not pass", () => {
    expect(lockLine({ kind: "failed", method: "Face ID" })).toBe("Face ID did not pass.");
    expect(lockLine({ kind: "cancelled", method: "Touch ID" })).toBe("You cancelled Touch ID.");
    expect(lockLine({ kind: "cancelled", method: "the passcode" })).toBe(
      "You cancelled the passcode.",
    );
    expect(lockLine({ kind: "locked", method: "Face ID" })).toBe(
      "Face ID is locked. Unlock asks for the passcode.",
    );
    expect(unlockHint("Face ID", null)).toBe("Asks for Face ID.");
    expect(unlockHint("Face ID", { kind: "locked", method: "Face ID" })).toBe(
      "Asks for the passcode of this iPhone.",
    );
    expect(unlockHint("the passcode", null)).toBe("Asks for the passcode of this iPhone.");
  });

  it("gives the lines of the row, the notice and the mode menu", () => {
    expect(unlockRowLine({ passcode: true, method: "Face ID" })).toBe(
      "Opens efr with Face ID. Auto from this phone needs it.",
    );
    expect(unlockRowLine({ passcode: true, method: "the passcode" })).toBe(
      "Opens efr with the passcode. Auto from this phone needs it.",
    );
    expect(unlockRowLine({ passcode: false, method: "the passcode" })).toBe(NO_PASSCODE_ROW_LINE);
    expect(noPasscodeNotice("Face ID")).toBe("Face ID unlock is off: this iPhone has no passcode.");
    expect(autoNeedsLine("Face ID")).toBe("Needs Face ID unlock in Settings.");
    expect(turnOnTitle("the passcode")).toBe("Turn on Passcode unlock");
  });
});
