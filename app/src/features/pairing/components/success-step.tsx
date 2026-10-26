// The success of a pairing (P-Success): "✓ paired" in green, the machine's name large,
// its MagicDNS name, the efrd line, and Start. A re-pair says "re-paired", and Start goes
// back to where the user was. The efrd line comes once the new connection is up.

import { View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { MonoText } from "@/components/ui/mono-text";
import { PrimaryButton } from "@/components/ui/primary-button";

import { daemonLine, successWords } from "../model/pairing-view";
import { PairingBody, PairingFooter } from "./pairing-layout";

export interface SuccessStepProps {
  /** The short name of the machine, such as `desktop`. */
  readonly machine: string;
  /** Its MagicDNS name, such as `desktop.tail3f2a.ts.net`, when the code had one. */
  readonly fqdn: string | undefined;
  readonly repaired: boolean;
  /** What efrd said about itself, once the machine is connected. */
  readonly daemon: { readonly version: string; readonly protocol: number } | undefined;
  readonly onStart: () => void;
}

/** The success step. */
export function SuccessStep(props: SuccessStepProps) {
  const words = successWords(props.machine, props.repaired);
  const { fqdn, daemon } = props;
  return (
    <>
      <PairingBody gap="success">
        <MonoText size="monoSm" tone="state_success">
          {words.status}
        </MonoText>
        <MonoText size="monoDisplay" tone="text_bright" accessibilityRole="header">
          {props.machine}
        </MonoText>
        {fqdn === undefined ? null : (
          <MonoText size="monoShell" tone="text_muted">
            {fqdn}
          </MonoText>
        )}
        {daemon === undefined ? (
          <View className="flex-row items-center gap-8">
            <BrailleCell tint="wait" label="Connecting" />
            <MonoText size="monoSm">connecting</MonoText>
          </View>
        ) : (
          <MonoText size="monoSm">{daemonLine(daemon)}</MonoText>
        )}
      </PairingBody>
      <PairingFooter hint={words.hint}>
        <PrimaryButton label="Start" onPress={props.onStart} />
      </PairingFooter>
    </>
  );
}
