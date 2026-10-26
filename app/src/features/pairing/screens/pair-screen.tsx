// The pairing modal (design 3.17, plan 2.5): a full screen modal with its own nav row. It
// shows the pairing session (model/pairing-session.ts) for its target: a new machine, or
// the machine of `?machine=<id>` for a re-pair.
//
// The ways out (design note 13 and plan 5.2 items 8.7 and 8.18):
// - Close stops what runs, then leaves to the screen under the modal. After the success of
//   a new machine it does what Start does, without the machine switch.
// - Start on a new machine makes it the machine of the blank root and resets the stack to
//   the root. Start after a re-pair goes back to where the user was.
// - Done (efrd too old, no scanner) closes.

import type { MachineId } from "@/features/machines/model/machine-ref";

import { PairingFlow } from "../components/pairing-flow";
import type { ErrorAction } from "../model/pairing-view";
import { pairingTitle } from "../model/pairing-view";
import {
  usePairedStatus,
  usePairingExits,
  usePairingPhase,
  usePairingSession,
  useRepairTarget,
} from "../use-pairing";

export interface PairScreenProps {
  /** The daemon id of the machine to pair again; null for a new machine. */
  readonly machine: MachineId | null;
}

/** The pairing modal. */
export function PairScreen(props: PairScreenProps) {
  const session = usePairingSession();
  const repair = useRepairTarget(session, props.machine);
  const phase = usePairingPhase(session, repair?.id ?? null);
  const paired = phase.kind === "paired" ? phase : undefined;
  const status = usePairedStatus(paired?.machine.id ?? null);
  const exits = usePairingExits();

  const close = (): void => {
    session.close();
    if (paired !== undefined && !paired.repaired) {
      exits.toRoot();
    } else {
      exits.leave();
    }
  };
  const start = (): void => {
    if (paired === undefined || paired.repaired) {
      session.close();
      exits.leave();
      return;
    }
    session.start(paired.machine.id);
    exits.toRoot();
  };
  const act = (action: ErrorAction): void => {
    switch (action) {
      case "scan_again":
        void session.scan(repair);
        return;
      case "open_tailscale":
        session.openTailscale();
        return;
      case "open_settings":
        session.openSettings();
        return;
      case "try_again":
        session.tryAgain();
        return;
      default:
        close();
    }
  };

  return (
    <PairingFlow
      title={pairingTitle(repair?.name)}
      phase={phase}
      daemon={status}
      onClose={close}
      onScan={() => void session.scan(repair)}
      onCancel={() => session.cancel()}
      onAction={act}
      onStart={start}
    />
  );
}
