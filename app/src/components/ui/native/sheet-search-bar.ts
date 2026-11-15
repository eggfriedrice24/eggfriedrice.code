// The props of the native search bar of a form sheet (the model sheet, All
// conversations): in the iOS 26 bottom toolbar (`Stack.Toolbar.SearchBarSlot`), with the
// header of the sheet and its rows in sight while the user types.

import type { Stack } from "expo-router";
import type { ComponentProps } from "react";

/** Spread these into `Stack.SearchBar` of a sheet. */
export const SHEET_SEARCH_BAR = {
  autoCapitalize: "none",
  hideNavigationBar: false,
  obscureBackground: false,
  allowToolbarIntegration: true,
} as const satisfies Partial<ComponentProps<typeof Stack.SearchBar>>;
