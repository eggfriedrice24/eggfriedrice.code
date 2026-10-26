import { Stack } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { KeyboardProvider } from "react-native-keyboard-controller";

import { semantic } from "@/theme/palette";
import { createDemoSession, type DemoSession } from "@/ui/demo_session";
import type { PhoneSession } from "@/ui/session";
import { SessionProvider, UNPAIRED } from "@/ui/session_context";

const screenOptions = {
  contentStyle: { backgroundColor: semantic.surface_1 },
  headerStyle: { backgroundColor: semantic.surface_1 },
  headerTintColor: semantic.text_primary,
  headerTitleStyle: { color: semantic.text_bright },
};

const DEMO_TIMERS = {
  schedule: (callback: () => void, ms: number) => {
    const timer = setTimeout(callback, ms);
    return () => clearTimeout(timer);
  },
  now: () => Date.now(),
};

const INDEX = { title: "Conversations" };
const PAIR = { title: "Pair", headerBackVisible: false };
const CONVERSATION = { title: "Conversation" };

// NOTE: the store (src/store) gives the session of the paired phone here. Until it
// does, the app starts as an unpaired phone, and a dev build can open the demo.
export default function RootLayout() {
  const [session, setSession] = useState<PhoneSession>(UNPAIRED);
  const [demo, setDemo] = useState<DemoSession | undefined>(undefined);
  const openDemo = useCallback(() => {
    const created = createDemoSession(DEMO_TIMERS);
    setDemo(created);
    setSession(created);
  }, []);
  useEffect(() => () => demo?.dispose(), [demo]);
  return (
    <KeyboardProvider>
      <SessionProvider session={session} {...(__DEV__ ? { onDemo: openDemo } : {})}>
        <Stack screenOptions={screenOptions}>
          <Stack.Screen name="index" options={INDEX} />
          <Stack.Screen name="pair" options={PAIR} />
          <Stack.Screen name="conversation/[id]" options={CONVERSATION} />
        </Stack>
      </SessionProvider>
    </KeyboardProvider>
  );
}
