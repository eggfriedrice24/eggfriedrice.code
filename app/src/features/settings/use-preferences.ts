// The preferences of the app in React. The root layout makes the preferences store once
// and gives it through `PreferencesContext`; a screen reads it with `usePreferences()`.

import { createContext, use } from "react";

import type { PreferencesStore } from "./model/preferences";

/** The preferences store. The root layout gives it. */
export const PreferencesContext = createContext<PreferencesStore | null>(null);

/** The preferences store. It throws outside the provider of the root layout. */
export function usePreferences(): PreferencesStore {
  const preferences = use(PreferencesContext);
  if (preferences === null) {
    throw new Error("usePreferences needs the PreferencesContext of the root layout");
  }
  return preferences;
}
