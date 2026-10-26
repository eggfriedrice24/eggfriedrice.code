// The services of the app, made once when the root layout loads (plan section 2.2,
// "Runtime in React"): the runtime of the machines with the stream of each conversation,
// the QueryClient and its persister, the outbox and its drains, the drafts and the
// preferences. They live outside React; the providers give them to the screens.
//
// - The runtime starts at once: it reads the pairing records and connects every machine.
//   `loaded` settles when the records are read; the screens wait for it under Suspense.
// - The database opens once, in the background, for the event cache and the outbox.
// - The outbox reads the prompts of the last launch, and the drain of each machine sends
//   them when the machine is ready.
// - A machine that the user forgets loses its cache rows, its outbox entries, its drafts
//   and its place as the last machine.
//
// Every effect comes in as a parameter, so the services run in Vitest with fakes. The
// root layout gives the native ones.

import type { QueryClient } from "@tanstack/react-query";
import type { AsyncStorage } from "@tanstack/react-query-persist-client";

import type { Clock } from "@/lib/net/clock";
import { wireQueryManagers, type QueryManagers, type Subscribe } from "@/lib/query/managers";
import { queryPersistOptions, type QueryPersistOptions } from "@/lib/query/persister";
import { createQueryClient } from "@/lib/query/query-client";
import type { EventCaches } from "@/lib/store/event-cache";
import type { FrameScheduler } from "@/lib/store/frame-store";
import type { KeyValueStorage } from "@/lib/store/key-value";
import type { SqlDatabase } from "@/lib/store/sqlite-cache";
import {
  createConversationStream,
  type ConversationStream,
} from "@/features/conversation/model/conversation-stream";
import { createDrafts, type Drafts } from "@/features/conversation/model/drafts";
import {
  createOutbox,
  type Outbox,
  type OutboxStorage,
} from "@/features/conversation/model/outbox";
import { invalidateOnReady } from "@/features/machines/machine-queries";
import {
  createRuntime,
  type Lifecycle,
  type MachinePurge,
  type Runtime,
} from "@/features/machines/model/runtime";
import type { PairingDeps } from "@/features/pairing/model/pair";
import { createPreferences, type PreferencesStore } from "@/features/settings/model/preferences";
import { wireOutboxDrains } from "./outbox-drains";
import { withOutboxEcho } from "./outbox-echo";

/** The runtime of the app: its streams are the full streams of the conversations. */
export type AppRuntime = Runtime<ConversationStream>;

/** The services that the screens use. */
export interface AppServices {
  readonly runtime: AppRuntime;
  readonly queryClient: QueryClient;
  readonly persistOptions: QueryPersistOptions;
  readonly outbox: Outbox;
  readonly drafts: Drafts;
  readonly preferences: PreferencesStore;
  /** Settles when the runtime read the paired machines. It never rejects. */
  readonly loaded: Promise<void>;
  /** Stops every connection, drain and listener. */
  dispose(): void;
}

/** The effects of the app. */
export interface AppServicesDeps {
  /** The effects of pairing, or null in a build without the device key module. */
  readonly pairing: PairingDeps | null;
  /** A random value in [0, 1), for the backoff ladders. */
  readonly random: () => number;
  /** Runs a callback on the next display frame. */
  readonly schedule: FrameScheduler;
  /** The clock of the outbox drains. */
  readonly clock: Clock;
  /** The time now, ms since the epoch, for the order of the outbox. */
  readonly now: () => number;
  /** A new command id. */
  readonly newId: () => string;
  /** Opens the app's database. The services call it once. */
  readonly openDatabase: () => Promise<SqlDatabase>;
  readonly openEventCaches: (db: SqlDatabase) => Promise<EventCaches>;
  readonly openOutboxStorage: (db: SqlDatabase) => Promise<OutboxStorage>;
  /** The key-value store of the drafts and the preferences. */
  readonly keyValue: KeyValueStorage;
  /** The storage of the persisted queries. */
  readonly queryStorage: AsyncStorage;
  /** The version of the app, for the buster of the persisted queries. */
  readonly appVersion: string;
  readonly lifecycle: Lifecycle;
  /** True when the phone has a network, for the online manager of Query. */
  readonly followOnline: Subscribe<boolean>;
  /** Makes every haptic read the Haptics switch. */
  readonly setHapticsEnabled: (read: () => boolean) => void;
  /** Called with the errors that the app goes on after. */
  readonly onError: (error: unknown) => void;
  /** The managers of Query; the tests give fakes. */
  readonly queryManagers?: QueryManagers;
}

/** An outbox storage that waits for the database to open. */
function deferredOutboxStorage(open: () => Promise<OutboxStorage>): OutboxStorage {
  let storage: Promise<OutboxStorage> | undefined;
  const get = (): Promise<OutboxStorage> => {
    storage ??= open();
    return storage;
  };
  return {
    load: async () => (await get()).load(),
    put: async (entry) => (await get()).put(entry),
    remove: async (commandId) => (await get()).remove(commandId),
    removeMachine: async (machine) => (await get()).removeMachine(machine),
  };
}

/** Makes the services and starts them. */
export function createAppServices(deps: AppServicesDeps): AppServices {
  let database: Promise<SqlDatabase> | undefined;
  const openDatabase = (): Promise<SqlDatabase> => {
    database ??= deps.openDatabase();
    return database;
  };
  const onError = deps.onError;

  const preferences = createPreferences({ storage: deps.keyValue, onStorageError: onError });
  const drafts = createDrafts({ storage: deps.keyValue, onStorageError: onError });
  const outbox = createOutbox({
    storage: deferredOutboxStorage(async () => deps.openOutboxStorage(await openDatabase())),
    now: deps.now,
    newId: deps.newId,
    onStorageError: onError,
  });

  const purges: MachinePurge[] = [
    (machine, caches) => caches?.removeMachine(machine),
    (machine) => outbox.removeMachine(machine),
    (machine) => {
      drafts.removeMachine(machine);
    },
    (machine) => {
      preferences.forgetMachine(machine);
    },
  ];
  const runtime: AppRuntime = createRuntime({
    pairing: deps.pairing,
    random: deps.random,
    streams: withOutboxEcho(createConversationStream, outbox),
    schedule: deps.schedule,
    openCache: async () => deps.openEventCaches(await openDatabase()),
    lifecycle: deps.lifecycle,
    purges,
    onError,
  });

  const queryClient = createQueryClient();
  const persistOptions = queryPersistOptions(deps.queryStorage, deps.appVersion);
  deps.setHapticsEnabled(() => preferences.store.get().haptics);

  const hydrated = outbox.hydrate().catch((error: unknown) => {
    onError(error);
  });
  const stops = [
    wireQueryManagers(
      {
        appActive: (listener) => deps.lifecycle.followAppState(listener),
        networkOnline: deps.followOnline,
      },
      deps.queryManagers,
    ),
    invalidateOnReady(runtime, queryClient),
    wireOutboxDrains({ runtime, outbox, hydrated, clock: deps.clock, random: deps.random }),
  ];
  const loaded = runtime.load().catch((error: unknown) => {
    onError(error);
  });

  return {
    runtime,
    queryClient,
    persistOptions,
    outbox,
    drafts,
    preferences,
    loaded,
    dispose(): void {
      for (const stop of stops.splice(0)) {
        stop();
      }
      runtime.dispose();
    },
  };
}
