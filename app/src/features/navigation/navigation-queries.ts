// The queries of the navigation: what the iPhone can check for the Face ID unlock (the
// name of the switch, a passcode or not). They are data of the device, so they live in
// TanStack Query; iOS answers again at each mount and each return to the app.

import { queryOptions } from "@tanstack/react-query";

import type { AppLock } from "./model/app-lock";

/** What the iPhone can check now (round 9). Never persisted. */
export function ownerFactsQueryOptions(lock: Pick<AppLock, "facts">) {
  return queryOptions({
    queryKey: ["device", "owner"] as const,
    queryFn: () => lock.facts(),
    staleTime: 0,
  });
}
