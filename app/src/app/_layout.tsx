import "../../global.css";

import { Stack, type NativeStackNavigationOptions } from "expo-router";
import { KeyboardProvider } from "react-native-keyboard-controller";

import { semantic } from "@/theme/palette";

export { ErrorBoundary } from "expo-router";

// The root stack and the presentation of each route (plan 2.5). The screens are stubs
// until the waves of the UI rebuild replace them; the providers of the runtime come with
// the navigation package (wave 3).

const HEADER: NativeStackNavigationOptions = {
  headerTransparent: true,
  headerBlurEffect: "systemChromeMaterialDark",
  headerBackButtonDisplayMode: "minimal",
  headerBackButtonMenuEnabled: true,
  headerLargeTitle: false,
  headerTintColor: semantic.text_primary,
  headerTitleStyle: { color: semantic.text_bright },
  contentStyle: { backgroundColor: semantic.surface_1 },
};

/** True when the route was pushed with `enter=instant` (the first send on the root). */
function entersInstantly(params: unknown): boolean {
  return (
    typeof params === "object" && params !== null && "enter" in params && params.enter === "instant"
  );
}

const ROOT: NativeStackNavigationOptions = { title: "efr" };
const CONVERSATION: NativeStackNavigationOptions = { title: "Conversation" };
const SETTINGS: NativeStackNavigationOptions = { title: "Settings" };
const CONVERSATIONS: NativeStackNavigationOptions = {
  title: "All conversations",
  presentation: "formSheet",
  sheetAllowedDetents: [0.5, 1],
  sheetGrabberVisible: true,
};
const PAIR: NativeStackNavigationOptions = {
  presentation: "fullScreenModal",
  headerShown: false,
};

export default function RootLayout() {
  return (
    <KeyboardProvider>
      <Stack
        screenOptions={({ route }): NativeStackNavigationOptions => ({
          ...HEADER,
          animation: entersInstantly(route.params) ? "none" : "default",
        })}
      >
        <Stack.Screen name="index" options={ROOT} />
        <Stack.Screen name="c/[machine]/[id]" options={CONVERSATION} />
        <Stack.Screen name="settings" options={SETTINGS} />
        <Stack.Screen name="conversations" options={CONVERSATIONS} />
        <Stack.Screen name="pair" options={PAIR} />
      </Stack>
    </KeyboardProvider>
  );
}
