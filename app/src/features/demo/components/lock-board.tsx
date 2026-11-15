// The S-Lock board of the gallery: the lock screen of the Face ID unlock in each state and
// the cover of the app switcher, drawn from the words of lib/lock/owner-check.ts. The
// controls change nothing.

import { View } from "react-native";

import { type FailedCheck, lockLine, type OwnerMethod, unlockHint } from "@/lib/lock/owner-check";
import { LockCover, LockScreen } from "@/features/navigation/components/lock-screen";
import { SheetCase, SheetScroll } from "./sheet-layout";

/** The height of one frame: the lock screen fills it. */
const FRAME = "h-640 overflow-hidden rounded-box border border-border-subtle";

const CASES: [string, OwnerMethod, FailedCheck | null][] = [
  ["while iOS asks: the mark only", "Face ID", null],
  ["Face ID did not pass", "Face ID", { kind: "failed", method: "Face ID" }],
  ["you cancelled", "Face ID", { kind: "cancelled", method: "Face ID" }],
  ["Face ID is locked: too many tries", "Face ID", { kind: "locked", method: "Face ID" }],
  ["Touch ID did not pass", "Touch ID", { kind: "failed", method: "Touch ID" }],
  [
    "the passcode only: you cancelled",
    "the passcode",
    { kind: "cancelled", method: "the passcode" },
  ],
];

/** The lock screen boards. */
export function LockBoard() {
  return (
    <SheetScroll>
      {CASES.map(([name, method, failed]) => (
        <SheetCase key={name} name={name}>
          <View className={FRAME}>
            <LockScreen
              line={failed === null ? undefined : lockLine(failed)}
              hint={unlockHint(method, failed)}
              onUnlock={() => undefined}
            />
          </View>
        </SheetCase>
      ))}
      <SheetCase name="the cover of the app switcher">
        <View className={FRAME}>
          <LockCover />
        </View>
      </SheetCase>
    </SheetScroll>
  );
}
