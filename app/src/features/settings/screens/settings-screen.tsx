// The settings: the paired machines, the haptics switch and the version (plan 2.5).
// NOTE: A stub until the real screen comes (wave 3).

import { useRouter } from "expo-router";

import { ScreenStub, StubLink } from "@/components/ui/screen-stub";

/** The settings screen. */
export function SettingsScreen() {
  const router = useRouter();
  return (
    <ScreenStub name="settings">
      <StubLink label="Pair a new machine" onPress={() => router.push("/pair")} />
    </ScreenStub>
  );
}
