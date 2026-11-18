// The services of the app in React (plan section 2.2, "Runtime in React"). The root
// layout makes them once (model/app-services.ts) and gives them with
// `AppServicesContext`; a component reads them with these hooks. Other features read
// the runtime, the outbox, the drafts and the preferences here.

import { createContext, use } from "react";

import type { Drafts } from "@/features/conversation/model/drafts";
import type { ModelChoices } from "@/features/conversation/model/model-choices";
import type { Outbox } from "@/features/conversation/model/outbox";
import type { Voice } from "@/features/conversation/model/voice";
import type { PreferencesStore } from "@/features/settings/model/preferences";
import type { AppLock } from "./model/app-lock";
import type { AppAttach, AppRuntime, AppServices } from "./model/app-services";

/** The services of the app. The root layout gives them. */
export const AppServicesContext = createContext<AppServices | null>(null);

/** The services of the app. It throws outside the providers of the root layout. */
export function useAppServices(): AppServices {
  const services = use(AppServicesContext);
  if (services === null) {
    throw new Error("useAppServices runs only inside the providers of the root layout.");
  }
  return services;
}

/** The runtime of the machines. */
export function useRuntime(): AppRuntime {
  return useAppServices().runtime;
}

/** The outbox of prompts. */
export function useOutbox(): Outbox {
  return useAppServices().outbox;
}

/** The drafts of the composer. */
export function useDrafts(): Drafts {
  return useAppServices().drafts;
}

/** The model and effort chosen per composer. */
export function useModelChoices(): ModelChoices {
  return useAppServices().modelChoices;
}

/** The preferences. */
export function usePreferences(): PreferencesStore {
  return useAppServices().preferences;
}

/** The Face ID unlock of the app. */
export function useAppLock(): AppLock {
  return useAppServices().appLock;
}

/** The attachments of the composer, or null in a build without efr-files and the pickers. */
export function useAttach(): AppAttach | null {
  return useAppServices().attach;
}

/** Voice input of the composer, or null in a build without efr-speech. */
export function useVoice(): Voice | null {
  return useAppServices().voice;
}
