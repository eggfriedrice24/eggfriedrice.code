// Fakes for the tests of the screens: the services of the app over fake
// sockets, a fake clock, fake key stores and stores in memory. The machines are the test
// machines of the machines feature (desktop and laptop). Nothing reaches the network or
// the disk. Only test files import this module.

import type { QueryClient } from "@tanstack/react-query";

import {
  FakeClock,
  FakeSockets,
  completeHandshake,
  flush,
  remoteHello,
  type FakeSocket,
} from "@/lib/net/testing";
import type { ConversationStatus, ConversationsRecentResult } from "@/lib/protocol/generated";
import type { QueryManagers } from "@/lib/query/managers";
import { MemoryStorage } from "@/lib/query/testing";
import { MemoryEventCaches } from "@/lib/store/event-cache";
import type { KeyValueStorage } from "@/lib/store/key-value";
import type { SqlDatabase } from "@/lib/store/sqlite-cache";
import { MemoryOutboxStorage } from "@/features/conversation/model/outbox-storage";
import { conversationsKeys } from "@/features/conversations/conversations-queries";
import type { MachineId } from "@/features/machines/model/machine-ref";
import { DESKTOP, LAPTOP, machineRecord, pairedStores } from "@/features/machines/model/testing";
import type { PairedDevice } from "@/features/pairing/model/paired-device";
import type { MemoryPairedDevices } from "@/features/pairing/model/testing";
import { createAppServices, type AppServices } from "./app-services";

export { DESKTOP, LAPTOP };

/** A key-value store in memory. */
export class MemoryKeyValue implements KeyValueStorage {
  readonly values = new Map<string, string>();

  getItemSync(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  getItem(key: string): Promise<string | null> {
    return Promise.resolve(this.getItemSync(key));
  }

  setItem(key: string, value: string): Promise<void> {
    this.values.set(key, value);
    return Promise.resolve();
  }

  removeItem(key: string): Promise<void> {
    this.values.delete(key);
    return Promise.resolve();
  }
}

/** A database that the fakes never use: the caches and the outbox are in memory. */
const NO_DATABASE: SqlDatabase = {
  execAsync: () => Promise.reject(new Error("no database in the tests")),
  runAsync: () => Promise.reject(new Error("no database in the tests")),
  getAllAsync: () => Promise.reject(new Error("no database in the tests")),
};

/** Managers of Query that hear nothing, so a test never touches the global ones. */
const QUIET_MANAGERS: QueryManagers = {
  focus: { setEventListener: () => undefined },
  online: { setEventListener: () => undefined },
};

/** The services of a test and the fakes under them. */
export interface TestServices {
  readonly services: AppServices;
  readonly sockets: FakeSockets;
  readonly clock: FakeClock;
  readonly keyValue: MemoryKeyValue;
  /** The pairing records of the phone. */
  readonly devices: MemoryPairedDevices;
  readonly errors: unknown[];
  /** Tells the listeners of the app state that the app went to the foreground or not. */
  setActive(active: boolean): void;
  /** Opens the socket of `machine` and answers its handshake. */
  connect(machine: MachineId): Promise<void>;
}

/** The newest socket to `url`. */
export function lastSocket(sockets: FakeSockets, url: string): FakeSocket {
  for (let index = sockets.made.length - 1; index >= 0; index -= 1) {
    const socket = sockets.made[index];
    if (socket?.url === url) {
      return socket;
    }
  }
  throw new Error(`no socket to ${url}`);
}

/** The time of the tests: 2026-10-26 12:00 UTC. */
export const TEST_NOW = Date.parse("2026-10-26T12:00:00Z");

/** The services of the app with `records` paired (desktop and laptop by default). */
export async function testServices(
  records: readonly PairedDevice[] = [machineRecord(DESKTOP), machineRecord(LAPTOP)],
): Promise<TestServices> {
  const stores = await pairedStores(records);
  // The clock of the machines starts at the time of the tests, as the real one does.
  const clock = new FakeClock(TEST_NOW);
  const sockets = new FakeSockets();
  const keyValue = new MemoryKeyValue();
  const errors: unknown[] = [];
  const appState = new Set<(active: boolean) => void>();
  let ids = 0;
  let aliases = 0;
  const services = createAppServices({
    pairing: {
      sockets: sockets.factory,
      clock,
      keys: stores.keys,
      devices: stores.devices,
      sha256: () => Promise.resolve(Uint8Array.from({ length: 32 }, (_, i) => i)),
      newKeyAlias: () => `device-new-${++aliases}`,
      now: () => new Date(TEST_NOW),
      client: "efr-app test",
    },
    random: () => 0,
    schedule: (callback) => {
      callback();
      return () => undefined;
    },
    clock,
    now: () => TEST_NOW,
    newId: () => {
      ids += 1;
      return `019a9b1c-3d00-7a10-8b20-${String(ids).padStart(12, "0")}`;
    },
    openDatabase: () => Promise.resolve(NO_DATABASE),
    openEventCaches: () => Promise.resolve(new MemoryEventCaches()),
    openOutboxStorage: () => Promise.resolve(new MemoryOutboxStorage()),
    keyValue,
    queryStorage: new MemoryStorage(),
    appVersion: "0.0.0-test",
    lifecycle: {
      followAppState: (listener) => {
        appState.add(listener);
        return () => {
          appState.delete(listener);
        };
      },
      followNetwork: () => () => undefined,
    },
    followOnline: () => () => undefined,
    setHapticsEnabled: () => undefined,
    onError: (error) => errors.push(error),
    queryManagers: QUIET_MANAGERS,
  });
  await services.loaded;
  return {
    services,
    sockets,
    clock,
    keyValue,
    devices: stores.devices,
    errors,
    setActive(active) {
      for (const listener of appState) {
        listener(active);
      }
    },
    async connect(machine) {
      const record = records.find((one) => one.daemonId === machine);
      if (record === undefined) {
        throw new Error(`machine ${machine} is not paired in this test`);
      }
      const socket = lastSocket(sockets, `ws://${record.host}:${record.port}/`);
      await completeHandshake(socket, remoteHello({ daemon_id: machine }));
      await flush();
    },
  };
}

/** One row of a test recent list. */
export interface RecentRow {
  readonly id: string;
  readonly title: string;
  readonly status?: ConversationStatus;
  /** Minutes before TEST_NOW. */
  readonly minutesAgo?: number;
}

/** Puts a recent list of `machine` in the query cache, as if efrd had sent it now. */
export function setRecent(client: QueryClient, machine: MachineId, rows: readonly RecentRow[]) {
  const result: ConversationsRecentResult = {
    conversations: rows.map((row) => ({
      summary: {
        id: row.id,
        title: row.title,
        status: row.status ?? "idle",
        cwd: "/home/me/code/efr",
        created_at: "2026-10-26T09:00:00Z",
        updated_at: new Date(TEST_NOW - (row.minutesAgo ?? 1) * 60_000).toISOString(),
        last_seq: 1,
      },
      route: { kind: "outside" },
      turns: 1,
    })),
  };
  client.setQueryData(conversationsKeys.recent(machine), result);
}

/** A conversation id of the tests. */
export function conversationId(n: number): string {
  return `019a9b1c-3d00-7a10-8b20-${String(1000 + n).padStart(12, "0")}`;
}
