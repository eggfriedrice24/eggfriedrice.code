// An error of pairing (P-Errors): one layout for every error. A mono line with "✗" in
// state_error and a short word, the title (Title 3, machine names in mono), one plain
// reason, and exactly one action. Close always leaves. The session plays the error haptic
// and announces the title when the error appears.

import { Text } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { MonoText } from "@/components/ui/mono-text";
import { PrimaryButton } from "@/components/ui/primary-button";

import {
  ERROR_ACTION_LABEL,
  errorView,
  type ErrorAction,
  type PairingProblem,
} from "../model/pairing-view";
import { PairingBody, PairingFooter } from "./pairing-layout";
import { TextRuns } from "./text-runs";

export interface ErrorStepProps {
  readonly problem: PairingProblem;
  readonly onAction: (action: ErrorAction) => void;
}

/** The error step. */
export function ErrorStep(props: ErrorStepProps) {
  const view = errorView(props.problem);
  const action = view.action;
  return (
    <>
      <PairingBody gap="error">
        <MonoText size="monoSm" tone="text_muted">
          <Text className="text-state-error">✗</Text> {view.word}
        </MonoText>
        <BodyText size="pairingTitle" tone="text_bright" accessibilityRole="header">
          <TextRuns runs={view.title} monoClassName="text-mono-heading" />
        </BodyText>
        <BodyText>
          <TextRuns runs={view.reason} />
        </BodyText>
      </PairingBody>
      <PairingFooter>
        <PrimaryButton label={ERROR_ACTION_LABEL[action]} onPress={() => props.onAction(action)} />
      </PairingFooter>
    </>
  );
}
