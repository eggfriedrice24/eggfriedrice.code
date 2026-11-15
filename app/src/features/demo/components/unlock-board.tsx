// The Settings-Unlock board of the gallery: the row "Face ID unlock" of Settings in the six
// states of Settings round 9 (frames 1 to 6), then the row as Settings marks it after "Turn
// on Face ID unlock" of the mode menu. The words come from lib/lock/owner-check.ts, as in
// Settings. A device shows only the state of its own iPhone, so this board is the one
// place to see the others. The switches change nothing.

import { InsetGroup } from "@/components/ui/inset-group";
import {
  noPasscodeNotice,
  type OwnerFacts,
  unlockName,
  unlockRowLine,
} from "@/lib/lock/owner-check";
import { UnlockRow } from "@/features/settings/components/unlock-row";
import { SheetCase, SheetScroll } from "./sheet-layout";

interface UnlockCase {
  readonly name: string;
  readonly facts: OwnerFacts;
  readonly on: boolean;
  readonly checking?: boolean;
  /** efr turned the unlock off because the passcode went (frame 6). */
  readonly notice?: boolean;
  readonly marked?: boolean;
}

const FACE_ID: OwnerFacts = { passcode: true, method: "Face ID" };

const CASES: readonly UnlockCase[] = [
  { name: "1 · off: the default", facts: FACE_ID, on: false },
  { name: "2 · on", facts: FACE_ID, on: true },
  { name: "3 · iOS checks: the switch has not moved", facts: FACE_ID, on: false, checking: true },
  {
    name: "4 · no Face ID or Touch ID: Passcode unlock",
    facts: { passcode: true, method: "the passcode" },
    on: false,
  },
  {
    name: "5 · no passcode: the row is greyed",
    facts: { passcode: false, method: "Face ID" },
    on: false,
  },
  {
    name: "6 · the passcode was removed: efr turned it off",
    facts: { passcode: false, method: "Face ID" },
    on: false,
    notice: true,
  },
  {
    name: "Touch ID: the name follows the iPhone",
    facts: { passcode: true, method: "Touch ID" },
    on: false,
  },
  {
    name: "marked: opened from Turn on Face ID unlock",
    facts: FACE_ID,
    on: false,
    marked: true,
  },
];

/** The row of `one`, with the words and the greyed state of UnlockGroup in Settings. */
function CaseRow(props: { readonly one: UnlockCase }) {
  const { one } = props;
  const disabled = !one.facts.passcode && !one.on;
  const notice = one.notice === true && disabled;
  return (
    <InsetGroup header="Privacy">
      <UnlockRow
        title={unlockName(one.facts.method)}
        line={notice ? noPasscodeNotice(one.facts.method) : unlockRowLine(one.facts)}
        notice={notice}
        marked={one.marked === true}
        value={one.on}
        checking={one.checking === true}
        disabled={disabled}
        onValueChange={() => undefined}
      />
    </InsetGroup>
  );
}

/** The board of the unlock row. */
export function UnlockBoard() {
  return (
    <SheetScroll>
      {CASES.map((one) => (
        <SheetCase key={one.name} name={one.name}>
          <CaseRow one={one} />
        </SheetCase>
      ))}
    </SheetScroll>
  );
}
