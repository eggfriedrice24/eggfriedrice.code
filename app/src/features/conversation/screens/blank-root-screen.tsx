// The root of the stack: a blank conversation on the last used machine (plan 2.5).
// NOTE: A stub until the real screen comes (wave 4). It links to every other screen.

import { useRouter } from "expo-router";

import { ScreenStub, StubLink } from "@/components/ui/screen-stub";

/** The machine and the conversation that the stub links to. Not real ids of efrd. */
const STUB_MACHINE = "019a9b1c-3d00-7a10-8b20-0000000000aa";
const STUB_CONVERSATION = "019a9b1c-3d00-7a10-8b20-0000000000bb";

/** The blank root screen. */
export function BlankRootScreen() {
  const router = useRouter();
  return (
    <ScreenStub name="index">
      <StubLink
        label="A conversation"
        onPress={() =>
          router.push({
            pathname: "/c/[machine]/[id]",
            params: { machine: STUB_MACHINE, id: STUB_CONVERSATION },
          })
        }
      />
      <StubLink label="All conversations" onPress={() => router.push("/conversations")} />
      <StubLink label="Settings" onPress={() => router.push("/settings")} />
      <StubLink label="Pair a machine" onPress={() => router.push("/pair")} />
    </ScreenStub>
  );
}
