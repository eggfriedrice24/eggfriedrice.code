// The runtime of the machines: the registry of the paired machines and one supervisor for
// each (plan section 2.2, "Multi-machine"). It lives outside React; the root layout makes
// it once with `createRuntime` and gives it to the screens through a context.
//
// - `load` reads the pairing records and opens the event caches, then starts every
//   supervisor at the same time, so all machines connect in parallel.
// - One AppState listener and one NetInfo listener fan out to every supervisor.
// - `pair` runs the pairing flow and then adds the machine, or gives a re-paired machine
//   its new key; the other machines keep their connections.
// - `forget` closes the machine's supervisor, removes its record and its key, and runs the
//   purges (the event cache rows, the outbox, the drafts) that the root layout gives.
//
// Every effect comes in as a parameter, so the runtime runs in Vitest with fake sockets, a
// fake clock and fake key stores.

import { createAtom, type Atom, type Readable } from "@tanstack/store";

import type { UnaryMethodName } from "@/lib/net/connection";
import { ConnectionError } from "@/lib/net/errors";
import type { MethodParams } from "@/lib/protocol/generated";
import type { CallResults } from "@/lib/protocol/wire";
import type { EventCaches } from "@/lib/store/event-cache";
import type { FrameScheduler } from "@/lib/store/frame-store";
import {
  pair,
  PairingError,
  type PairingDeps,
  type PairingOptions,
} from "@/features/pairing/model/pair";
import type { CancelSignal, PairingStep } from "@/features/pairing/model/pair";
import { forgetPairing, loadPairings, type Pairing } from "@/features/pairing/model/paired-device";
import { machineOf, type Machine } from "./machine";
import type { MachineStatus } from "./machine-connection";
import type { ConversationRef, MachineId } from "./machine-ref";
import {
  LOADING_REGISTRY,
  loadedRegistry,
  withMachine,
  withoutMachine,
  type RegistryState,
} from "./machine-registry";
import {
  MachineSupervisor,
  type ClientTuning,
  type HoldMode,
  type StreamHold,
} from "./machine-supervisor";
import { pathChanged, type NetworkPath } from "./network-path";
import type { ConversationStreamFactory, SupervisedStream } from "./stream-factory";

/** The listeners of the app's life that the runtime fans out to every machine. */
export interface Lifecycle {
  /** Calls `listener` with true in the foreground and false in the background. */
  followAppState(listener: (active: boolean) => void): () => void;
  /** Calls `listener` with the network path, at the start and on each change. */
  followNetwork(listener: (path: NetworkPath) => void): () => void;
}

/**
 * Removes what the app keeps of a forgotten machine, such as its event cache rows, its
 * outbox entries or its drafts. `caches` are the event caches that the runtime opened.
 */
export type MachinePurge = (
  machine: MachineId,
  caches: EventCaches | undefined,
) => Promise<void> | void;

/** What the runtime needs. `S` is the type of the streams that `streams` makes. */
export interface RuntimeDeps<S extends SupervisedStream = SupervisedStream> {
  /**
   * The effects of the pairing flow: the sockets, the clock, the key store, the record
   * store and the rest. Null when the build has no device key module (Expo Go): the app
   * then has no machine and cannot pair.
   */
  readonly pairing: PairingDeps | null;
  /** A random value in [0, 1) for the backoff ladders. */
  readonly random: () => number;
  /** Makes the stream of a conversation (the conversation feature implements it). */
  readonly streams: ConversationStreamFactory<S>;
  /** Runs a callback on the next display frame (requestAnimationFrame in the app). */
  readonly schedule: FrameScheduler;
  /** Opens the event caches once; without them the streams keep nothing. */
  readonly openCache?: () => Promise<EventCaches>;
  readonly lifecycle?: Lifecycle;
  /** Run by `forget`, in parallel, after the record and the key are gone. */
  readonly purges?: readonly MachinePurge[];
  /** Called with errors that the runtime goes on after, such as a failed cache. */
  readonly onError?: (error: unknown) => void;
  /** How long a stream stays after its last hold. */
  readonly retainMs?: number;
  readonly tuning?: ClientTuning;
}

/** The options of `pair`. */
export interface PairOptions {
  /** The name that efrd shows for the phone. */
  readonly name: string;
  /** The machine that the user pairs again; a code of another machine then fails. */
  readonly repair?: MachineId;
  readonly signal?: CancelSignal;
  readonly onStep?: (step: PairingStep) => void;
}

/** The runtime of the machines, as the screens, the queries and the outbox use it. */
export interface Runtime<S extends SupervisedStream = SupervisedStream> {
  /** The paired machines. */
  readonly registry: Readable<RegistryState>;
  /** Reads the records and connects every machine. A second call gives the same promise. */
  load(): Promise<void>;
  /** The supervisor of machine `id`, or undefined when it is not paired. */
  machine(id: MachineId): MachineSupervisor<S> | undefined;
  /**
   * Runs a method on machine `machine`. It rejects with a `ConnectionError` of kind
   * `offline` when the machine is not paired or not connected.
   */
  call<M extends UnaryMethodName>(
    machine: MachineId,
    method: M,
    params: MethodParams[M],
    signal?: AbortSignal,
  ): Promise<CallResults[M]>;
  /** What efrd said about itself on the connection that is up now. */
  status(machine: MachineId): Promise<MachineStatus>;
  /** Holds the stream of `ref`, or gives undefined when its machine is not paired. */
  hold(ref: ConversationRef, mode: HoldMode): StreamHold<S> | undefined;
  /** Asks machine `machine` to connect now, for "Try again". */
  retry(machine: MachineId): void;
  /**
   * Pairs the phone with the machine of the QR code `code`. It rejects with a
   * `PairingError` when the flow gives no device.
   */
  pair(code: string, options: PairOptions): Promise<Machine>;
  /** Forgets machine `machine`: its connection, its record, its key and its local data. */
  forget(machine: MachineId): Promise<void>;
  /** Calls `listener` each time a machine's connection becomes ready. */
  onReady(listener: (machine: MachineId) => void): () => void;
  /** Closes every connection and stops every listener. */
  dispose(): void;
}

/** The sentence of a build without the device key module. */
const NO_KEY_MODULE = "This build of the app has no device key, so it cannot pair.";

/** Makes the runtime. It does nothing until `load`. */
export function createRuntime<S extends SupervisedStream>(deps: RuntimeDeps<S>): Runtime<S> {
  return new MachineRuntime(deps);
}

class MachineRuntime<S extends SupervisedStream> implements Runtime<S> {
  readonly registry: Readable<RegistryState>;
  private readonly atom: Atom<RegistryState>;
  private readonly deps: RuntimeDeps<S>;
  private readonly supervisors = new Map<MachineId, MachineSupervisor<S>>();
  private readonly readyListeners = new Set<(machine: MachineId) => void>();
  private readonly stops: (() => void)[] = [];
  private loading: Promise<void> | undefined;
  private caches: EventCaches | undefined;
  private path: NetworkPath | undefined;
  private disposed = false;

  constructor(deps: RuntimeDeps<S>) {
    this.deps = deps;
    this.atom = createAtom<RegistryState>(LOADING_REGISTRY);
    this.registry = this.atom;
  }

  load(): Promise<void> {
    this.loading ??= this.read();
    return this.loading;
  }

  machine(id: MachineId): MachineSupervisor<S> | undefined {
    return this.supervisors.get(id);
  }

  call<M extends UnaryMethodName>(
    machine: MachineId,
    method: M,
    params: MethodParams[M],
    signal?: AbortSignal,
  ): Promise<CallResults[M]> {
    const supervisor = this.supervisors.get(machine);
    if (supervisor === undefined) {
      return Promise.reject(notPaired());
    }
    return supervisor.call(method, params, signal === undefined ? {} : { signal });
  }

  status(machine: MachineId): Promise<MachineStatus> {
    const supervisor = this.supervisors.get(machine);
    if (supervisor === undefined) {
      return Promise.reject(notPaired());
    }
    try {
      return Promise.resolve(supervisor.status());
    } catch (error) {
      return Promise.reject(error instanceof Error ? error : notPaired());
    }
  }

  hold(ref: ConversationRef, mode: HoldMode): StreamHold<S> | undefined {
    return this.supervisors.get(ref.machine)?.hold(ref.id, mode);
  }

  retry(machine: MachineId): void {
    this.supervisors.get(machine)?.retry();
  }

  async pair(code: string, options: PairOptions): Promise<Machine> {
    await this.load();
    const pairing = this.deps.pairing;
    if (pairing === null) {
      throw new PairingError({ kind: "key_failed", message: NO_KEY_MODULE });
    }
    const result = await pair(code, pairing, pairingOptions(options));
    if (!result.ok) {
      throw new PairingError(result.failure);
    }
    const key = await pairing.keys.load(result.device.keyAlias).catch((error: unknown) => {
      this.report(error);
      return null;
    });
    return this.register({ device: result.device, key });
  }

  async forget(machine: MachineId): Promise<void> {
    await this.load();
    this.supervisors.get(machine)?.dispose();
    this.supervisors.delete(machine);
    this.atom.set((state) => withoutMachine(state, machine));
    const pairing = this.deps.pairing;
    let failure: unknown;
    if (pairing !== null) {
      try {
        await forgetPairing(pairing.keys, pairing.devices, machine);
      } catch (error) {
        failure = error;
      }
    }
    const { caches } = this;
    await Promise.all(
      (this.deps.purges ?? []).map(async (purge) => {
        try {
          await purge(machine, caches);
        } catch (error) {
          this.report(error);
        }
      }),
    );
    if (failure !== undefined) {
      throw failure;
    }
  }

  onReady(listener: (machine: MachineId) => void): () => void {
    this.readyListeners.add(listener);
    return () => {
      this.readyListeners.delete(listener);
    };
  }

  dispose(): void {
    this.disposed = true;
    for (const stop of this.stops.splice(0)) {
      stop();
    }
    for (const supervisor of this.supervisors.values()) {
      supervisor.dispose();
    }
    this.supervisors.clear();
  }

  private async read(): Promise<void> {
    const { pairing, openCache } = this.deps;
    const [caches, pairings] = await Promise.all([
      openCache === undefined
        ? Promise.resolve(undefined)
        : openCache().catch((error: unknown) => {
            this.report(error);
            return undefined;
          }),
      pairing === null
        ? Promise.resolve<readonly Pairing[]>([])
        : loadPairings(pairing.keys, pairing.devices).catch((error: unknown) => {
            this.report(error);
            return [];
          }),
    ]);
    if (this.disposed) {
      return;
    }
    this.caches = caches;
    const machines: Machine[] = [];
    for (const one of pairings) {
      this.supervisors.set(one.device.daemonId, this.supervisorOf(one));
      machines.push(machineOf(one.device, one.key === null));
    }
    this.atom.set(loadedRegistry(machines));
    this.follow();
    for (const supervisor of this.supervisors.values()) {
      supervisor.start();
    }
  }

  /** Adds a newly paired machine, or gives a re-paired one its new record and key. */
  private register(pairing: Pairing): Machine {
    const id = pairing.device.daemonId;
    const machine = machineOf(pairing.device, pairing.key === null);
    const existing = this.supervisors.get(id);
    if (existing === undefined) {
      const supervisor = this.supervisorOf(pairing);
      this.supervisors.set(id, supervisor);
      supervisor.start();
    } else {
      existing.replace(pairing);
    }
    this.atom.set((state) => withMachine(state, machine));
    return machine;
  }

  private supervisorOf(pairing: Pairing): MachineSupervisor<S> {
    const { deps } = this;
    const effects = deps.pairing;
    if (effects === null) {
      throw new Error(NO_KEY_MODULE);
    }
    return new MachineSupervisor(pairing, {
      sockets: effects.sockets,
      clock: effects.clock,
      random: deps.random,
      client: effects.client,
      streams: deps.streams,
      cache: this.caches?.forMachine(pairing.device.daemonId),
      schedule: deps.schedule,
      onError: (error) => this.report(error),
      onReady: (machine) => this.ready(machine),
      ...optionalSupervisorDeps(deps),
    });
  }

  /** One AppState and one NetInfo listener for all machines. */
  private follow(): void {
    const lifecycle = this.deps.lifecycle;
    if (lifecycle === undefined) {
      return;
    }
    this.stops.push(
      lifecycle.followAppState((active) => {
        for (const supervisor of this.supervisors.values()) {
          supervisor.setForeground(active);
        }
      }),
      lifecycle.followNetwork((path) => {
        const changed = pathChanged(this.path, path);
        this.path = path;
        if (changed) {
          for (const supervisor of this.supervisors.values()) {
            supervisor.networkChanged();
          }
        }
      }),
    );
  }

  private ready(machine: MachineId): void {
    for (const listener of this.readyListeners) {
      try {
        listener(machine);
      } catch (error) {
        this.report(error);
      }
    }
  }

  private report(error: unknown): void {
    this.deps.onError?.(error);
  }
}

/** The options of the pairing flow for `options`. */
function pairingOptions(options: PairOptions): PairingOptions {
  const result: { -readonly [K in keyof PairingOptions]: PairingOptions[K] } = {
    name: options.name,
  };
  if (options.repair !== undefined) {
    result.repair = options.repair;
  }
  if (options.signal !== undefined) {
    result.signal = options.signal;
  }
  if (options.onStep !== undefined) {
    result.onStep = options.onStep;
  }
  return result;
}

/** The optional settings of a supervisor that the runtime passes on. */
function optionalSupervisorDeps<S extends SupervisedStream>(
  deps: RuntimeDeps<S>,
): { retainMs?: number; tuning?: ClientTuning } {
  const result: { retainMs?: number; tuning?: ClientTuning } = {};
  if (deps.retainMs !== undefined) {
    result.retainMs = deps.retainMs;
  }
  if (deps.tuning !== undefined) {
    result.tuning = deps.tuning;
  }
  return result;
}

function notPaired(): ConnectionError {
  return new ConnectionError("offline", "this machine is not paired with the phone");
}
