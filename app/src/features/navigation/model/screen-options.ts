// The options of the root stack and of each route (plan section 2.5, design section 2).
// Every bar is transparent, has no large title and shows the back button without a
// title; a long press on it opens the history menu. The app is dark only, so the bar and
// its menus use the dark style. Pure: the root stack gives it the Liquid Glass flag.
//
// - iOS 26 (Liquid Glass), as t3code: the bar has no fill, no shadow and no blur of its
//   own. The system soft scroll edge at the top of the transcript is its material. The
//   other edges have no effect, so nothing blurs over the composer.
// - Before iOS 26: the system chrome blur of the bar. Those versions have no scroll edge
//   effects, so all edges stay hidden.

import type { NativeStackNavigationOptions } from "expo-router";

import { NO_FILL, roles } from "@/theme/roles";
import { entersInstantly } from "./routes";

/** The options of every bar, on every version of iOS. */
const BAR_OPTIONS: NativeStackNavigationOptions = {
  headerTransparent: true,
  headerBackButtonDisplayMode: "minimal",
  headerBackButtonMenuEnabled: true,
  headerLargeTitle: false,
  headerUserInterfaceStyle: "dark",
  headerTintColor: roles.text_primary,
  headerTitleStyle: { color: roles.text_bright },
  contentStyle: { backgroundColor: roles.surface_1 },
};

/** The bar on iOS 26: no fill, no shadow, no blur; the soft scroll edge at the top. */
const LIQUID_GLASS_OPTIONS: NativeStackNavigationOptions = {
  ...BAR_OPTIONS,
  headerStyle: { backgroundColor: NO_FILL },
  headerShadowVisible: false,
  scrollEdgeEffects: { top: "automatic", bottom: "hidden", left: "hidden", right: "hidden" },
};

/** The bar before iOS 26: the system chrome blur, and no scroll edge effects. */
const BLUR_OPTIONS: NativeStackNavigationOptions = {
  ...BAR_OPTIONS,
  headerBlurEffect: "systemChromeMaterialDark",
  scrollEdgeEffects: { top: "hidden", bottom: "hidden", left: "hidden", right: "hidden" },
};

/** The options of every route, for iOS 26 (`liquidGlass`) or an older iOS. */
export function headerOptions(liquidGlass: boolean): NativeStackNavigationOptions {
  return liquidGlass ? LIQUID_GLASS_OPTIONS : BLUR_OPTIONS;
}

/**
 * The options of a route with `params`. The push of the first send on the blank root
 * (`enter=instant`) has no animation.
 */
export function screenOptions(params: unknown, liquidGlass: boolean): NativeStackNavigationOptions {
  return {
    ...headerOptions(liquidGlass),
    animation: entersInstantly(params) ? "none" : "default",
  };
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
