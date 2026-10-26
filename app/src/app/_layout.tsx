import { Stack } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { KeyboardProvider } from "react-native-keyboard-controller";

import { createNativeSession } from "@/session/native";
import { semantic } from "@/theme/palette";
import { createDemoSession, type DemoSession } from "@/ui/demo_session";
import type { PhoneSession } from "@/ui/session";
import { SessionProvider } from "@/ui/session_context";

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

// The session of the phone (src/session): it reads the pairing, connects to efrd and
// keeps the connection up while the app runs. A dev build can swap in the demo, which
// reaches no computer.
export default function RootLayout() {
  const [live] = useState(createNativeSession);
  const [demo, setDemo] = useState<DemoSession | undefined>(undefined);
  const session: PhoneSession = demo ?? live;
  useEffect(() => {
    void live.start();
    return () => live.dispose();
  }, [live]);
  const openDemo = useCallback(() => {
    live.dispose();
    setDemo(createDemoSession(DEMO_TIMERS));
  }, [live]);
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
