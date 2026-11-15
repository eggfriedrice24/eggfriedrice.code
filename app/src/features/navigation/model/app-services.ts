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
// - A machine that the user forgets loses its cache rows, its outbox entries, its drafts,
//   its queries and its place as the last machine, also when the removal of its key
//   fails.
// - The summary of each snapshot goes into the lists of its machine at once.
// - The watch of the lists of each machine (`conversations.watch`) goes into its lists
//   too; a watch that stopped makes the recent list ask efrd again.
// - The drafts are saved at once when the app goes to the background: iOS can stop the
//   wait of a debounced save there, and then end the app.
//
// Every effect comes in as a parameter, so the services run in Vitest with fakes. The
// root layout gives the native ones.

import type { QueryClient } from "@tanstack/react-query";
import type { AsyncStorage } from "@tanstack/react-query-persist-client";

import { type CheckOwner, DEFAULT_FACTS, type OwnerFacts } from "@/lib/lock/owner-check";
import type { Clock } from "@/lib/net/clock";
import { wireQueryManagers, type QueryManagers, type Subscribe } from "@/lib/query/managers";
import { queryPersistOptions, type QueryPersistOptions } from "@/lib/query/persister";
import { machineKey } from "@/lib/query/keys";
import { createQueryClient } from "@/lib/query/query-client";
import type { ConversationSummary } from "@/lib/protocol/generated";
import type { EventCaches } from "@/lib/store/event-cache";
import type { FrameScheduler } from "@/lib/store/frame-store";
import type { KeyValueStorage } from "@/lib/store/key-value";
import type { SqlDatabase } from "@/lib/store/sqlite-cache";
import {
  createConversationStream,
  type ConversationStream,
} from "@/features/conversation/model/conversation-stream";
import { createDrafts, type Drafts } from "@/features/conversation/model/drafts";
import { createModelChoices, type ModelChoices } from "@/features/conversation/model/model-choices";
import {
  createOutbox,
  type Outbox,
  type OutboxStorage,
} from "@/features/conversation/model/outbox";
import {
  RECENT_LIMIT,
  applySummary,
  applyWatchItem,
  conversationsKeys,
} from "@/features/conversations/conversations-queries";
import { invalidateOnReady } from "@/features/machines/machine-queries";
import type { MachineId } from "@/features/machines/model/machine-ref";
import type {
  ConversationStreamFactory,
  SupervisedStream,
} from "@/features/machines/model/stream-factory";
import {
  createRuntime,
  type Lifecycle,
  type MachinePurge,
  type Runtime,
} from "@/features/machines/model/runtime";
import type { PairingDeps } from "@/features/pairing/model/pair";
import { createPreferences, type PreferencesStore } from "@/features/settings/model/preferences";
import { createAppLock, type AppLock, type LockPhase } from "./app-lock";
import type { LinkGate } from "./link-gate";
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
  /** The model and effort chosen per composer. */
  readonly modelChoices: ModelChoices;
  /** The Face ID unlock of the app (round 9). */
  readonly appLock: AppLock;
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
  /**
   * The effects of the Face ID unlock. Without them (a test of another feature), the lock
   * starts from the saved switch with a check that never passes, so it never turns on.
   */
  readonly lock?: AppLockEffects;
  /** The gate of the links from outside; the root layout gives the one of the app. */
  readonly linkGate?: LinkGate;
  /** The managers of Query; the tests give fakes. */
  readonly queryManagers?: QueryManagers;
}

/** The effects of the Face ID unlock: the phases of the app and the checks of iOS. */
export interface AppLockEffects {
  followAppPhase(listener: (phase: LockPhase) => void): () => void;
  appPhase(): LockPhase;
  readonly checkOwner: CheckOwner;
  readonly ownerFacts: () => Promise<OwnerFacts>;
  readonly hasPasscode: () => Promise<boolean>;
  /** Closes the keyboard and an open action sheet, so nothing waits under the lock. */
  readonly dismissInput: () => void;
  readonly announce: (line: string) => void;
}

/** The effects of a lock that never turns on, for the tests of other features. */
const NO_LOCK: AppLockEffects = {
  followAppPhase: () => () => undefined,
  appPhase: () => "active",
  checkOwner: () => Promise.resolve({ kind: "failed", method: "Face ID" }),
  ownerFacts: () => Promise.resolve(DEFAULT_FACTS),
  hasPasscode: () => Promise.resolve(true),
  dismissInput: () => undefined,
  announce: () => undefined,
};

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
  const modelChoices = createModelChoices({ storage: deps.keyValue, onStorageError: onError });
  const outbox = createOutbox({
    storage: deferredOutboxStorage(async () => deps.openOutboxStorage(await openDatabase())),
    now: deps.now,
    newId: deps.newId,
    onStorageError: onError,
  });

  const queryClient = createQueryClient();
  const purges: MachinePurge[] = [
    (machine, caches) => caches?.removeMachine(machine),
    (machine) => outbox.removeMachine(machine),
    (machine) => {
      drafts.removeMachine(machine);
      modelChoices.removeMachine(machine);
    },
    (machine) => {
      preferences.forgetMachine(machine);
    },
    (machine) => {
      queryClient.removeQueries({ queryKey: machineKey(machine) });
    },
  ];
  const runtime: AppRuntime = createRuntime({
    pairing: deps.pairing,
    random: deps.random,
    streams: withSummaries(withOutboxEcho(createConversationStream, outbox), (machine, summary) =>
      applySummary(queryClient, machine, summary),
    ),
    schedule: deps.schedule,
    openCache: async () => deps.openEventCaches(await openDatabase()),
    lifecycle: deps.lifecycle,
    purges,
    onError,
    lists: {
      limit: RECENT_LIMIT,
      item: (machine, item) => applyWatchItem(queryClient, machine, item),
      stopped: (machine) => {
        void queryClient.invalidateQueries({ queryKey: conversationsKeys.recent(machine) });
      },
    },
  });

  const persistOptions = queryPersistOptions(deps.queryStorage, deps.appVersion);
  deps.setHapticsEnabled(() => preferences.store.get().haptics);

  const hydrated = outbox.hydrate().catch((error: unknown) => {
    onError(error);
  });
  const lock = deps.lock ?? NO_LOCK;
  const saved = preferences.store.get();
  const appLock = createAppLock({
    saved: { on: saved.faceIdUnlock, notice: saved.faceIdUnlockNotice },
    now: deps.now,
    checkOwner: lock.checkOwner,
    ownerFacts: lock.ownerFacts,
    hasPasscode: lock.hasPasscode,
    save: ({ on, notice }) => preferences.setFaceIdUnlock(on, notice),
    // NOTE: each change sets every composer back to cautious and drops the old auto of
    // the outbox (D3), so no auto comes back by itself: not an auto that an old build
    // stored with no switch, and not one whose clear did not reach the storage.
    onSwitch: () => {
      outbox.dropAuto();
      modelChoices.clearAuto();
    },
    onLock: lock.dismissInput,
    announce: lock.announce,
    onError,
  });
  deps.linkGate?.follow(appLock);
  const stops = [
    wireQueryManagers(
      {
        appActive: (listener) => deps.lifecycle.followAppState(listener),
        networkOnline: deps.followOnline,
      },
      deps.queryManagers,
    ),
    invalidateOnReady(runtime, queryClient),
    wireOutboxDrains({
      runtime,
      outbox,
      hydrated,
      clock: deps.clock,
      random: deps.random,
      autoAllowed: () => appLock.store.get().on,
    }),
    lock.followAppPhase((phase) => appLock.phase(phase)),
    deps.lifecycle.followAppState((active) => {
      if (!active) {
        drafts.flush();
      }
    }),
  ];
  appLock.start(lock.appPhase());
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
    modelChoices,
    appLock,
    loaded,
    dispose(): void {
      for (const stop of stops.splice(0)) {
        stop();
      }
      runtime.dispose();
    },
  };
}

/** `factory`, with streams that give the summary of each snapshot to `onSummary` too. */
function withSummaries<S extends SupervisedStream>(
  factory: ConversationStreamFactory<S>,
  onSummary: (machine: MachineId, summary: ConversationSummary) => void,
): ConversationStreamFactory<S> {
  return (ref, deps) => {
    const own = deps.onSummary;
    return factory(ref, {
      ...deps,
      onSummary: (summary) => {
        own?.(summary);
        onSummary(ref.machine, summary);
      },
    });
  };
}
