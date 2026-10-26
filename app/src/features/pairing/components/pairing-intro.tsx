// The first step of pairing (P-Intro), and the body of the no-machine root (R-NoMachine):
// one sentence, the command `efr device add` in a box with a copy button, the tailnet
// hint, then the footer hint and "Scan code". iOS asks for the camera on the first scan.

import { Text, View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { CopyButton } from "@/components/ui/copy-button";
import { MonoText } from "@/components/ui/mono-text";
import { PrimaryButton } from "@/components/ui/primary-button";

import { PAIR_COMMAND } from "../model/pairing-view";
import { PairingBody, PairingFooter } from "./pairing-layout";

/** The words of the intro. A test reads them. */
export const INTRO_TEXT = {
  sentence: "Run this in a terminal on the machine you want to pair; it shows a code to scan.",
  tailnet: "this iPhone and the machine must be on the same tailnet",
  next: "next: point the camera at the code in the terminal",
  scan: "Scan code",
} as const;

/** The command box: "❯ efr device add" and the copy button. */
export function CommandBox() {
  return (
    <View className="h-52 flex-row items-center gap-10 rounded-control border border-border-subtle pl-16 pr-6">
      <Text className="text-mono-large text-accent-primary" accessibilityElementsHidden>
        ❯
      </Text>
      <Text
        className="flex-1 text-mono-large text-text-primary"
        numberOfLines={1}
        accessibilityLabel={PAIR_COMMAND}
      >
        <Text className="text-accent-primary">efr</Text> device add
      </Text>
      <CopyButton variant="circle" text={PAIR_COMMAND} label="Copy command" />
    </View>
  );
}

export interface PairingIntroProps {
  readonly onScan: () => void;
  /**
   * The space above the body: 44 pt under the nav row of the modal by default; the
   * no-machine root gives its own.
   */
  readonly top?: number | undefined;
}

/** The intro body and its footer. */
export function PairingIntro(props: PairingIntroProps) {
  return (
    <>
      <PairingBody gap="step" top={props.top}>
        <BodyText>{INTRO_TEXT.sentence}</BodyText>
        <CommandBox />
        <MonoText>{INTRO_TEXT.tailnet}</MonoText>
      </PairingBody>
      <PairingFooter hint={INTRO_TEXT.next}>
        <PrimaryButton label={INTRO_TEXT.scan} symbol="qrcode.viewfinder" onPress={props.onScan} />
      </PairingFooter>
    </>
  );
}
