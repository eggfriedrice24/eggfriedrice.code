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
// - In a build with expo-notifications, the push service gives each machine the push
//   token of the switch "Notifications", and the taps on a notice reach the router as
//   links to their conversation, for the machines that the phone knows.
// - In a build with efr-files and the pickers, the attachments hold the files of each
//   draft, the drains upload the files of a prompt, and a sent prompt keeps only the
//   previews of its photos.
// - In a build with efr-speech, voice input writes speech into a draft.
//
// Every effect comes in as a parameter, so the services run in Vitest with fakes. The
// root layout gives the native ones.

import type { QueryClient } from "@tanstack/react-query";
import type { AsyncStorage } from "@tanstack/react-query-persist-client";

import { devWarn } from "@/lib/dev/dev-warn";
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
import type { EfrSpeechNative } from "../../../../modules/efr-speech";
import {
  createAttachments,
  type AttachNative,
  type Attachments,
} from "@/features/conversation/model/attachments";
import {
  createConversationStream,
  type ConversationStream,
} from "@/features/conversation/model/conversation-stream";
import { createDrafts, type Drafts } from "@/features/conversation/model/drafts";
import { createModelChoices, type ModelChoices } from "@/features/conversation/model/model-choices";
import { createVoice, type Voice } from "@/features/conversation/model/voice";
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
import { createPush, type Push, type PushNative } from "@/features/settings/model/push";
import { createAppLock, type AppLock, type LockPhase } from "./app-lock";
import type { LinkGate } from "./link-gate";
import type { NoticeSource, NoticeTaps } from "./notice-taps";
import { discardUpload, wireOutboxDrains, type OutboxDrainsDeps } from "./outbox-drains";
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
  /** The push notices, or null in a build without expo-notifications. */
  readonly push: Push | null;
  /** The attachments of the composer, or null in a build without efr-files and the pickers. */
  readonly attach: AppAttach | null;
  /** Voice input of the composer, or null in a build without efr-speech. */
  readonly voice: Voice | null;
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
  /** The clock of the outbox drains and of the limit of a recording. */
  readonly clock: Clock;
  /** The time now, ms since the epoch, for the order of the outbox and the recording time. */
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
  /** The effects of push and of the taps on a notice; none in a build without them. */
  readonly push?: (PushNative & NoticeSource) | null;
  /** The taps on a notice; the root layout gives the ones of the app. */
  readonly noticeTaps?: NoticeTaps | undefined;
  /** The managers of Query; the tests give fakes. */
  readonly queryManagers?: QueryManagers;
  /** The pickers of iOS and efr-files; none in a build without them. */
  readonly attach?: AttachNative | null;
  /** efr-speech and the haptic of a recording; none in a build without the module. */
  readonly voice?: VoiceEffects | null;
}

/** The native side of voice input. */
export interface VoiceEffects {
  readonly speech: EfrSpeechNative;
  readonly haptic: () => void;
}

/** The pickers and the files of the drafts and the prompts. */
export interface AppAttach {
  readonly native: AttachNative;
  readonly attachments: Attachments;
}

/** Shows the file `uri` full screen in Quick Look. A failure goes to the dev log only. */
export function quickLook(attach: AppAttach, uri: string, name: string): void {
  attach.native.files.quickLook(uri, name).catch((error: unknown) => {
    devWarn(`Quick Look of ${name} failed: ${String(error)}`);
  });
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
  const pickers = deps.attach ?? null;
  const attach: AppAttach | null =
    pickers === null
      ? null
      : {
          native: pickers,
          attachments: createAttachments({
            files: pickers.files,
            outbox,
            discard: (machine, uploadId) => discardUpload(runtime, machine, uploadId),
            newId: deps.newId,
            storage: deps.keyValue,
            onError,
          }),
        };
  // The purges start in this order: the attachments remove the files of the outbox
  // entries of the machine before the outbox removes the entries.
  const purges: MachinePurge[] = [
    (machine, caches) => caches?.removeMachine(machine),
    (machine) => {
      drafts.removeMachine(machine);
      modelChoices.removeMachine(machine);
      attach?.attachments.removeMachine(machine);
    },
    (machine) => outbox.removeMachine(machine),
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
  const drainsDeps: { -readonly [K in keyof OutboxDrainsDeps]: OutboxDrainsDeps[K] } = {
    runtime,
    outbox,
    hydrated,
    clock: deps.clock,
    random: deps.random,
    autoAllowed: () => appLock.store.get().on,
  };
  if (attach !== null) {
    drainsDeps.readChunk = (uri, offset, length) =>
      attach.native.files.readChunk(uri, offset, length);
    drainsDeps.onSent = (entry) => attach.attachments.sent(entry);
  }
  const stops = [
    wireQueryManagers(
      {
        appActive: (listener) => deps.lifecycle.followAppState(listener),
        networkOnline: deps.followOnline,
      },
      deps.queryManagers,
    ),
    invalidateOnReady(runtime, queryClient),
    wireOutboxDrains(drainsDeps),
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

  const native = deps.push ?? null;
  let push: Push | null = null;
  if (native !== null) {
    const created = createPush({
      native,
      runtime,
      preferences,
      followAppState: (listener) => deps.lifecycle.followAppState(listener),
      onError,
    });
    push = created;
    stops.push(() => created.dispose());
    deps.noticeTaps?.attach(native, async (machine) => {
      await loaded;
      return runtime.machine(machine) !== undefined;
    });
  }

  const speech = deps.voice ?? null;
  let voice: Voice | null = null;
  if (speech !== null) {
    const created = createVoice({
      speech: speech.speech,
      drafts,
      clock: deps.clock,
      now: deps.now,
      haptic: speech.haptic,
      onError,
    });
    voice = created;
    stops.push(() => created.dispose());
  }

  return {
    runtime,
    queryClient,
    persistOptions,
    outbox,
    drafts,
    preferences,
    modelChoices,
    appLock,
    push,
    attach,
    voice,
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
