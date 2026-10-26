// One conversation of one machine, pushed on the stack (plan 2.5).
// NOTE: A stub until the real screen comes (wave 4).

import { useRouter } from "expo-router";

import { ScreenStub, StubLink } from "@/components/ui/screen-stub";

export interface ConversationScreenProps {
  /** The daemon id of the machine. */
  readonly machine: string;
  /** The conversation id. */
  readonly id: string;
}

/** The conversation screen. */
export function ConversationScreen(props: ConversationScreenProps) {
  const router = useRouter();
  return (
    <ScreenStub name="c/[machine]/[id]" details={[`machine ${props.machine}`, `id ${props.id}`]}>
      <StubLink label="+ New conversation" onPress={() => router.dismissAll()} />
      <StubLink label="Settings" onPress={() => router.push("/settings")} />
    </ScreenStub>
  );
}
