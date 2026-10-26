// Writes the small queries to the disk, so that the title menu and the All conversations
// sheet show the last lists before the machines connect (plan section 2.2).
//
// - Only a query with `meta.persist` and a result goes to the disk: the recent lists and
//   the machine status. History pages and errors stay in memory.
// - No mutation goes to the disk. The outbox keeps the prompts that wait.
// - The buster is the app version plus PERSIST_SCHEMA_VERSION. A new app or a new shape
//   of the persisted data drops the old copy instead of reading it.
// - A copy older than PERSIST_MAX_AGE_MS is dropped.
//
// The storage is passed in: `expo-sqlite/kv-store` in the app, a map in the tests.

import { createAsyncStoragePersister } from "@tanstack/query-async-storage-persister";
import type { AsyncStorage, PersistQueryClientOptions } from "@tanstack/react-query-persist-client";

import { isPersistedMeta } from "./query-meta";

/** The storage key of the persisted cache. */
export const PERSIST_KEY = "efr.query-cache";

/** Change it when the shape of a persisted result changes. */
export const PERSIST_SCHEMA_VERSION = 1;

/** The oldest persisted copy that the app reads: 7 days. */
export const PERSIST_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

/** The shortest time between two writes. */
export const PERSIST_THROTTLE_MS = 1000;

/** The buster of the persisted cache of app version `appVersion`. */
export function persistBuster(appVersion: string): string {
  return `${appVersion}+${PERSIST_SCHEMA_VERSION}`;
}

/** The options of `PersistQueryClientProvider` (`persistOptions`). */
export type QueryPersistOptions = Omit<PersistQueryClientOptions, "queryClient">;

/** The persist options of the app over `storage`. */
export function queryPersistOptions(
  storage: AsyncStorage,
  appVersion: string,
): QueryPersistOptions {
  return {
    persister: createAsyncStoragePersister({
      storage,
      key: PERSIST_KEY,
      throttleTime: PERSIST_THROTTLE_MS,
    }),
    buster: persistBuster(appVersion),
    maxAge: PERSIST_MAX_AGE_MS,
    dehydrateOptions: {
      shouldDehydrateQuery: (query) =>
        isPersistedMeta(query.meta) && query.state.status === "success",
      shouldDehydrateMutation: () => false,
    },
  };
}
