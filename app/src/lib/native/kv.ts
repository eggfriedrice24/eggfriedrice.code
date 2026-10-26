// The key-value store of the app: `Storage` of expo-sqlite/kv-store, a table in a SQLite
// database of its own. The drafts, the preferences and the cache of the queries keep
// their JSON in it. It holds no secret.
//
// `appStorage` has the full API of expo-sqlite, so the persister of TanStack Query can
// use it too. It is also a `KeyValueStorage` (src/lib/store/key-value.ts), which the
// drafts and the preferences take.

import { Storage } from "expo-sqlite/kv-store";

import type { KeyValueStorage } from "@/lib/store/key-value";

/** The key-value store of the app. */
export const appStorage = Storage satisfies KeyValueStorage;
