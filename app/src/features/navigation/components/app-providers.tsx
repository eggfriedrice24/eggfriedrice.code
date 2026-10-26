// The providers of the app (plan section 2.2): the keyboard controller, TanStack Query
// with the persisted queries, and the services of the app.

import { PersistQueryClientProvider } from "@tanstack/react-query-persist-client";
import type { ReactNode } from "react";
import { KeyboardProvider } from "react-native-keyboard-controller";

import type { AppServices } from "../model/app-services";
import { AppServicesContext } from "../use-app-services";

export interface AppProvidersProps {
  readonly services: AppServices;
  readonly children: ReactNode;
}

/** The providers around the root stack. */
export function AppProviders(props: AppProvidersProps) {
  const { services } = props;
  return (
    <KeyboardProvider>
      <PersistQueryClientProvider
        client={services.queryClient}
        persistOptions={services.persistOptions}
      >
        <AppServicesContext value={services}>{props.children}</AppServicesContext>
      </PersistQueryClientProvider>
    </KeyboardProvider>
  );
}
