// The pairing flow, as a full screen modal with its own nav row (plan 2.5).
// NOTE: A stub until the real screens come (wave 3).

import { useRouter } from "expo-router";

import { ScreenStub, StubLink } from "@/components/ui/screen-stub";

export interface PairScreenProps {
  /** The daemon id of the machine to pair again; absent for a new machine. */
  readonly machine?: string;
}

/** The pairing screen. */
export function PairScreen(props: PairScreenProps) {
  const router = useRouter();
  const title = props.machine === undefined ? "Pair a machine" : `Re-pair ${props.machine}`;
  return (
    <ScreenStub name="pair" details={[title]}>
      <StubLink label="Close" onPress={() => router.back()} />
    </ScreenStub>
  );
}
