// The sheet with all conversations of all machines (plan 2.5).
// NOTE: A stub until the real sheet comes (wave 3).

import { useRouter } from "expo-router";

import { ScreenStub, StubLink } from "@/components/ui/screen-stub";

/** The all conversations sheet. */
export function ConversationsScreen() {
  const router = useRouter();
  return (
    <ScreenStub name="conversations">
      <StubLink label="Close" onPress={() => router.back()} />
    </ScreenStub>
  );
}
