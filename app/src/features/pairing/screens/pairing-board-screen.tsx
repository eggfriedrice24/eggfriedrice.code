// One board of pairing (model/pairing-boards.ts) in the real pairing modal, with its
// sample data and no session, for the dev gallery (plan section 0). The buttons do
// nothing but Close and Start, which leave the board.

import { useRouter } from "expo-router";

import { PairingFlow } from "../components/pairing-flow";
import type { PairingBoard } from "../model/pairing-boards";

export interface PairingBoardScreenProps {
  readonly board: PairingBoard;
}

/** A pairing board for the dev gallery. */
export function PairingBoardScreen(props: PairingBoardScreenProps) {
  const router = useRouter();
  const { board } = props;
  const leave = (): void => router.back();
  return (
    <PairingFlow
      title={board.title}
      phase={board.phase}
      daemon={board.daemon}
      onClose={leave}
      onScan={() => undefined}
      onCancel={() => undefined}
      onAction={() => undefined}
      onStart={leave}
    />
  );
}
