// The steps while a pairing runs (P-Scan and P-Wait):
//
// - Scanning: the system scanner covers the screen. Our screen behind it has the nav row
//   at 60% and one hint under it. The scanner does not tell the app when the user closes
//   it, so Cancel stays under it.
// - Connecting: the phone checks efrd with hello. The step is short; it has the layout of
//   the confirm step without the key box.
// - Confirm: the terminal of the machine asks whether to add this phone. The phone shows
//   its name and its key, so the user can compare them, and waits. Cancel stops the
//   request; after the yes the phone keeps the pairing, and Cancel can no longer stop it.

import { Text, View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { BorderedButton } from "@/components/ui/bordered-button";
import { BrailleCell } from "@/components/ui/braille-cell";
import { KeyValueBox } from "@/components/ui/key-value-box";
import { MonoText } from "@/components/ui/mono-text";
import { SPACING } from "@/theme/metrics";

import { PairingBody, PairingFooter } from "./pairing-layout";

/** The space between the nav row and the scanner hint (P-Scan: top 118). */
const SCAN_HINT_TOP = 12;

export interface ScanStepProps {
  readonly onCancel: () => void;
}

/** Our screen behind the system scanner. */
export function ScanStep(props: ScanStepProps) {
  return (
    <>
      <PairingBody gap="step" top={SCAN_HINT_TOP}>
        <MonoText size="monoSm" tone="text_primary" className="text-center">
          point the camera at the code in the terminal
        </MonoText>
      </PairingBody>
      <PairingFooter>
        <BorderedButton label="Cancel" onPress={props.onCancel} />
      </PairingFooter>
    </>
  );
}

export interface ConfirmStepProps {
  /** The short name of the machine, such as `desktop`. */
  readonly machine: string;
  /** The name and the key fingerprint of the phone; absent while the phone connects. */
  readonly device?: { readonly name: string; readonly fingerprint: string } | undefined;
  /** True after the yes, while the phone keeps the pairing. */
  readonly saving?: boolean;
  readonly onCancel: () => void;
}

/** The connecting and the confirm step. */
export function ConfirmStep(props: ConfirmStepProps) {
  const device = props.device;
  const saving = props.saving === true;
  return (
    <>
      <PairingBody gap="step" top={SPACING.hit}>
        <BodyText size="pairingTitle" tone="text_bright" accessibilityRole="header">
          {device === undefined ? "Connecting to " : "Confirm on "}
          <Text className="text-mono-heading">{props.machine}</Text>
        </BodyText>
        {device === undefined ? null : (
          <>
            <BodyText>
              {`The terminal on ${props.machine} is asking whether to add this phone. ` +
                "Check that it shows the same name and key, then answer there."}
            </BodyText>
            <KeyValueBox
              rows={[
                { key: "device", value: device.name },
                { key: "key", value: device.fingerprint, fingerprint: true },
              ]}
            />
          </>
        )}
        <View accessible role="status" className="flex-row items-center gap-8">
          <BrailleCell tint="wait" />
          <MonoText>{statusLine(props.machine, device !== undefined, saving)}</MonoText>
        </View>
      </PairingBody>
      <PairingFooter>
        <BorderedButton label="Cancel" disabled={saving} onPress={props.onCancel} />
      </PairingFooter>
    </>
  );
}

/** The words of the status line. */
export function statusLine(machine: string, confirming: boolean, saving: boolean): string {
  if (saving) {
    return "keeping the pairing on this iPhone";
  }
  return confirming ? `waiting for an answer on ${machine}` : `checking efrd on ${machine}`;
}
