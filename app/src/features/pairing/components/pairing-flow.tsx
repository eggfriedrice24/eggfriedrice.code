// The pairing modal for one phase of the session (model/pairing-session.ts): the nav row
// and the step of that phase. It takes only props, so each board of the design (P-Intro,
// P-Scan, P-Wait, P-Success, P-Errors) and each undrawn state renders in a test and in the
// dev gallery.

import type { ErrorAction } from "../model/pairing-view";
import type { PairingPhase } from "../model/pairing-session";
import { ConfirmStep, ScanStep } from "./confirm-step";
import { ErrorStep } from "./error-step";
import { PairingIntro } from "./pairing-intro";
import { PairingModalFrame, PairingNavRow } from "./pairing-layout";
import { SuccessStep } from "./success-step";

export interface PairingFlowProps {
  /** "Pair a machine", or "Re-pair laptop". */
  readonly title: string;
  readonly phase: PairingPhase;
  /** What efrd said about itself after the pairing, once the machine is connected. */
  readonly daemon?: { readonly version: string; readonly protocol: number } | undefined;
  readonly onClose: () => void;
  readonly onScan: () => void;
  readonly onCancel: () => void;
  readonly onAction: (action: ErrorAction) => void;
  readonly onStart: () => void;
}

/** The pairing modal in the phase `phase`. */
export function PairingFlow(props: PairingFlowProps) {
  const { phase } = props;
  return (
    <PairingModalFrame
      navRow={
        <PairingNavRow
          title={props.title}
          onClose={props.onClose}
          dimmed={phase.kind === "scanning"}
        />
      }
    >
      <PairingStep {...props} />
    </PairingModalFrame>
  );
}

function PairingStep(props: PairingFlowProps) {
  const { phase } = props;
  switch (phase.kind) {
    case "scanning":
      return <ScanStep onCancel={props.onCancel} />;
    case "connecting":
      return <ConfirmStep machine={phase.machine} onCancel={props.onCancel} />;
    case "confirm":
      return (
        <ConfirmStep
          machine={phase.machine}
          device={{ name: phase.device, fingerprint: phase.fingerprint }}
          saving={phase.saving}
          onCancel={props.onCancel}
        />
      );
    case "paired":
      return (
        <SuccessStep
          machine={phase.machine.name}
          fqdn={phase.machine.fqdn}
          repaired={phase.repaired}
          daemon={props.daemon}
          onStart={props.onStart}
        />
      );
    case "failed":
      return <ErrorStep problem={phase.problem} onAction={props.onAction} />;
    default:
      return <PairingIntro onScan={props.onScan} />;
  }
}
