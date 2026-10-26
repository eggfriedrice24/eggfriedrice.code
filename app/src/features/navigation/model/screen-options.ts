// The options of the root stack and of each route (plan section 2.5, design section 2).
// Every bar is transparent with the system chrome blur, has no large title and shows the
// back button without a title; a long press on it opens the history menu. The app is
// dark only, so the bar and its menus use the dark style. Pure.

import type { NativeStackNavigationOptions } from "expo-router";

import { roles } from "@/theme/roles";
import { entersInstantly } from "./routes";

/** The options of every route. */
export const HEADER_OPTIONS: NativeStackNavigationOptions = {
  headerTransparent: true,
  headerBlurEffect: "systemChromeMaterialDark",
  // iOS 26 adds its own blur where content runs under a bar. With our header blur it
  // doubles, and on the inverted transcript list it covered the whole list when the
  // keyboard changed the insets. The header blur is enough.
  scrollEdgeEffects: { top: "hidden", bottom: "hidden", left: "hidden", right: "hidden" },
  headerBackButtonDisplayMode: "minimal",
  headerBackButtonMenuEnabled: true,
  headerLargeTitle: false,
  headerUserInterfaceStyle: "dark",
  headerTintColor: roles.text_primary,
  headerTitleStyle: { color: roles.text_bright },
  contentStyle: { backgroundColor: roles.surface_1 },
};

/**
 * The options of a route with `params`. The push of the first send on the blank root
 * (`enter=instant`) has no animation.
 */
export function screenOptions(params: unknown): NativeStackNavigationOptions {
  return { ...HEADER_OPTIONS, animation: entersInstantly(params) ? "none" : "default" };
}

/** The blank root. Its header sets its own title: the name of its machine. */
export const ROOT_OPTIONS: NativeStackNavigationOptions = { title: "" };

/** A conversation. Its header sets its own title. */
export const CONVERSATION_OPTIONS: NativeStackNavigationOptions = { title: "" };

/** Settings, pushed, with the native title. */
export const SETTINGS_OPTIONS: NativeStackNavigationOptions = { title: "Settings" };

/** The All conversations sheet: medium and large, with the grabber. */
export const CONVERSATIONS_OPTIONS: NativeStackNavigationOptions = {
  title: "All conversations",
  presentation: "formSheet",
  sheetAllowedDetents: [0.5, 1],
  sheetGrabberVisible: true,
};

/** Pairing, full screen, with its own nav row. */
export const PAIR_OPTIONS: NativeStackNavigationOptions = {
  presentation: "fullScreenModal",
  headerShown: false,
};
