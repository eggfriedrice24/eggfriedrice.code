// What the Settings screen needs from the app: the runtime of the machines and the
// preferences. The root layout gives them once, with
// `<SettingsServicesContext value={{ runtime, preferences }}>` around the stack, and the
// screen reads them with `use()`.
//
// Without the context the screen still opens: it shows Settings with no machine, and the
// Haptics switch changes nothing. So the app stays navigable before the layout gives it.

import { createContext } from "react";

import type { Runtime } from "@/features/machines/model/runtime";
import type { PreferencesStore } from "../model/preferences";

/** The runtime and the preferences, as Settings uses them. */
export interface SettingsServices {
  readonly runtime: Runtime;
  readonly preferences: PreferencesStore;
}

/** The services of Settings, or null when the layout gives none. */
export const SettingsServicesContext = createContext<SettingsServices | null>(null);
