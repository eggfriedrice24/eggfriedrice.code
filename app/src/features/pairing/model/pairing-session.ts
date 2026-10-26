// The pairing session: the one pairing that the phone runs, from the scan to the success
// or the error (design 3.17, plan W3-PAIRING-UI). It lives outside React, so a scan that
// starts on the no-machine root goes on in the pairing modal, and no screen needs an
// effect.
//
// - The scan runs the system scanner (./scanner.ts). The first efr code wins.
// - The pairing is the pair mutation of the machines (`pairMachineMutationOptions`). The
//   session runs it with a MutationObserver of the app's QueryClient, so the mutation's
//   own onSuccess (refresh the machine's queries) runs as usual. The flow's steps
//   (connecting, confirm, saving) go into the session's store.
// - The haptics, the VoiceOver announcement of an error, the AppState listener (a retry
//   when the user comes back from Tailscale) and the links come in as effects, so the
//   session runs in Vitest.
//
// Each run has a number. A result of an older run (after a cancel or a new scan) changes
// nothing.

import {
  MutationObserver,
  type MutationObserverOptions,
  type QueryClient,
} from "@tanstack/react-query";
import { createAtom, type Atom, type Readable } from "@tanstack/store";

import type { PairVariables } from "@/features/machines/machine-queries";
import type { Machine } from "@/features/machines/model/machine";
import type { MachineId } from "@/features/machines/model/machine-ref";
import type { RegistryState } from "@/features/machines/model/machine-registry";
import { cancellation, PairingError, type PairingFailure, type PairingStep } from "./pair";
import { machineName } from "./paired-device";
import { parsePairingUrl } from "./pairing-url";
import { errorView, pairingProblem, plainText, type PairingProblem } from "./pairing-view";
import { scanPairingCode, type ScannerApi, type ScanResult } from "./scanner";

/** The machine that the user pairs again. */
export interface RepairTarget {
  readonly id: MachineId;
  /** Its short name, such as `laptop`, for the title and the errors. */
  readonly name: string;
}

/** Where the pairing is. */
export type PairingPhase =
  /** The first step: the command to run and Scan code. */
  | { readonly kind: "intro" }
  /** The system scanner is open over the screen. */
  | { readonly kind: "scanning" }
  /** The phone connects to the machine of the code and checks it with hello. */
  | { readonly kind: "connecting"; readonly machine: string }
  /**
   * The terminal of `machine` asks whether to add the phone. `saving` is true after the
   * yes, while the phone keeps the pairing; a cancel can no longer stop it then.
   */
  | {
      readonly kind: "confirm";
      readonly machine: string;
      readonly device: string;
      readonly fingerprint: string;
      readonly saving: boolean;
    }
  /** The machine is paired. `repaired` is true for a re-pair. */
  | { readonly kind: "paired"; readonly machine: Machine; readonly repaired: boolean }
  | { readonly kind: "failed"; readonly problem: PairingProblem };

/** The state of the session. */
export interface PairingState {
  /** The machine that the user pairs again, or null for a new machine. */
  readonly repair: MachineId | null;
  readonly phase: PairingPhase;
}

/** The session before any pairing. */
export const IDLE_PAIRING: PairingState = { repair: null, phase: { kind: "intro" } };

/** The options of the pair mutation (`pairMachineMutationOptions` of the machines). */
export type PairMutationOptions = MutationObserverOptions<Machine, Error, PairVariables>;

/** The haptics that the session plays. */
export type PairingHaptic = "success" | "error";

/** What the session needs. */
export interface PairingSessionDeps {
  readonly client: QueryClient;
  readonly mutation: PairMutationOptions;
  /** The paired machines, for the names of the re-pair targets. */
  readonly registry: Readable<RegistryState>;
  /** The system code scanner, or null when this build has none. */
  readonly scanner: ScannerApi | null;
  /** The name that efrd shows for the phone, such as "iPhone 17 Pro". */
  readonly deviceName: () => string;
  readonly haptic: (kind: PairingHaptic) => void;
  /** Says `text` with VoiceOver: the title of an error when it appears. */
  readonly announce: (text: string) => void;
  /** Calls `listener` with true when the app comes to the foreground. */
  readonly followAppState: (listener: (active: boolean) => void) => () => void;
  readonly openTailscale: () => void;
  /** Opens the app's page of the iOS Settings (the camera switch). */
  readonly openSettings: () => void;
  /** Makes `machine` the machine of the blank root (the last machine preference). */
  readonly selectMachine: (machine: MachineId) => void;
}

/** What a scan gave, for the screen that started it. */
export type ScanOutcome =
  /** A code was found, and the pairing runs. */
  | "pairing"
  /** The scan failed (no camera permission, no scanner): the error screen shows it. */
  | "failed"
  /** The user cancelled the scan. */
  | "cancelled"
  /** A scan or a pairing runs already; nothing new started. */
  | "busy";

/** The pairing session. */
export interface PairingSession {
  readonly state: Readable<PairingState>;
  readonly registry: Readable<RegistryState>;
  /** Opens the scanner, then pairs with the first efr code. */
  scan(repair: RepairTarget | null): Promise<ScanOutcome>;
  /** Pairs with the code `code` (the text of the QR code). */
  pair(code: string, repair: RepairTarget | null): void;
  /** Stops the scan or the pairing, and goes back to the intro. */
  cancel(): void;
  /** Stops what runs and forgets the target: the modal closes. */
  close(): void;
  /** Goes back to the intro after an error ("Try again"). */
  tryAgain(): void;
  /** Pairs again with the last code, for example after Tailscale came back. */
  retry(): void;
  openTailscale(): void;
  openSettings(): void;
  /** Makes the machine of a new pairing the machine of the blank root (Start). */
  start(machine: MachineId): void;
  /** Stops what runs and every listener. */
  dispose(): void;
}

/** Makes a pairing session. */
export function createPairingSession(deps: PairingSessionDeps): PairingSession {
  return new Session(deps);
}

const INTRO: PairingPhase = { kind: "intro" };

/**
 * The phase that the screen of target `repair` shows: the intro when the session runs
 * for another target.
 */
export function phaseFor(state: PairingState, repair: MachineId | null): PairingPhase {
  return state.repair === repair ? state.phase : INTRO;
}

/** True while a scan or a pairing runs. */
export function isBusy(phase: PairingPhase): boolean {
  return phase.kind === "scanning" || phase.kind === "connecting" || phase.kind === "confirm";
}

class Session implements PairingSession {
  readonly state: Readable<PairingState>;
  readonly registry: Readable<RegistryState>;
  private readonly atom: Atom<PairingState>;
  private readonly deps: PairingSessionDeps;
  /** The number of the run whose results count. */
  private run = 0;
  private stopRun: (() => void) | undefined;
  private stopWatch: (() => void) | undefined;
  private last: { readonly code: string; readonly repair: RepairTarget | null } | undefined;

  constructor(deps: PairingSessionDeps) {
    this.deps = deps;
    this.atom = createAtom<PairingState>(IDLE_PAIRING);
    this.state = this.atom;
    this.registry = deps.registry;
  }

  async scan(repair: RepairTarget | null): Promise<ScanOutcome> {
    if (isBusy(this.atom.get().phase)) {
      return "busy";
    }
    const run = this.next();
    const repairId = repair?.id ?? null;
    const scanner = this.deps.scanner;
    if (scanner === null) {
      this.fail(run, repairId, { kind: "no_scanner" });
      return "failed";
    }
    const { signal, cancel } = cancellation();
    this.stopRun = cancel;
    this.set(run, { repair: repairId, phase: { kind: "scanning" } });
    // NOTE: a permission request that throws means the same as no scanner.
    const result = await scanPairingCode(scanner, signal).catch(() => SCANNER_FAILED);
    if (run !== this.run) {
      return "cancelled";
    }
    if (result.ok) {
      this.deps.haptic("success");
      this.pair(result.text, repair);
      return "pairing";
    }
    switch (result.reason) {
      case "permission":
        this.fail(run, repairId, { kind: "camera" });
        return "failed";
      case "unavailable":
        this.fail(run, repairId, { kind: "no_scanner" });
        return "failed";
      default:
        this.set(run, { repair: repairId, phase: { kind: "intro" } });
        return "cancelled";
    }
  }

  pair(code: string, repair: RepairTarget | null): void {
    const run = this.next();
    const repairId = repair?.id ?? null;
    this.last = { code, repair };
    const parsed = parsePairingUrl(code);
    const machine = parsed.ok ? machineName(parsed.code) : "the machine";
    const { signal, cancel } = cancellation();
    this.stopRun = cancel;
    this.set(run, { repair: repairId, phase: { kind: "connecting", machine } });

    const variables: { -readonly [K in keyof PairVariables]: PairVariables[K] } = {
      code,
      name: this.deps.deviceName(),
      signal,
      onStep: (step) => this.step(run, repairId, machine, step),
    };
    if (repair !== null) {
      variables.repair = repair.id;
    }
    const observer = new MutationObserver(this.deps.client, this.deps.mutation);
    void this.finish(run, observer.mutate(variables), machine, repair);
  }

  /** Shows the end of run `run`: the success, or the error. */
  private async finish(
    run: number,
    pairing: Promise<Machine>,
    machine: string,
    repair: RepairTarget | null,
  ): Promise<void> {
    const repairId = repair?.id ?? null;
    let paired: Machine;
    try {
      paired = await pairing;
    } catch (error) {
      if (run !== this.run) {
        return;
      }
      this.stopRun = undefined;
      const problem = pairingProblem(failureOf(error), { machine, repairing: repair?.name });
      if (problem === null) {
        this.set(run, { repair: repairId, phase: { kind: "intro" } });
      } else {
        this.fail(run, repairId, problem);
      }
      return;
    }
    if (run !== this.run) {
      return;
    }
    this.stopRun = undefined;
    this.deps.haptic("success");
    this.set(run, {
      repair: repairId,
      phase: { kind: "paired", machine: paired, repaired: repair !== null },
    });
  }

  cancel(): void {
    const { repair } = this.atom.get();
    const run = this.next();
    this.set(run, { repair, phase: { kind: "intro" } });
  }

  close(): void {
    const run = this.next();
    this.set(run, IDLE_PAIRING);
  }

  tryAgain(): void {
    this.cancel();
  }

  retry(): void {
    const last = this.last;
    if (last === undefined || isBusy(this.atom.get().phase)) {
      return;
    }
    this.pair(last.code, last.repair);
  }

  openTailscale(): void {
    this.deps.openTailscale();
  }

  openSettings(): void {
    this.deps.openSettings();
  }

  start(machine: MachineId): void {
    this.deps.selectMachine(machine);
    this.close();
  }

  dispose(): void {
    this.next();
  }

  /** Starts a new run: stops the old one and its listener. */
  private next(): number {
    this.run += 1;
    const stop = this.stopRun;
    this.stopRun = undefined;
    stop?.();
    this.stopWatch?.();
    this.stopWatch = undefined;
    return this.run;
  }

  private step(run: number, repair: MachineId | null, machine: string, step: PairingStep): void {
    if (run !== this.run) {
      return;
    }
    switch (step.step) {
      case "connecting":
        this.set(run, { repair, phase: { kind: "connecting", machine } });
        return;
      case "confirm":
        this.set(run, {
          repair,
          phase: {
            kind: "confirm",
            machine,
            device: step.name,
            fingerprint: step.fingerprint,
            saving: false,
          },
        });
        return;
      default: {
        const { phase } = this.atom.get();
        if (phase.kind === "confirm") {
          this.set(run, { repair, phase: { ...phase, saving: true } });
        }
      }
    }
  }

  private fail(run: number, repair: MachineId | null, problem: PairingProblem): void {
    this.set(run, { repair, phase: { kind: "failed", problem } });
    this.deps.haptic("error");
    this.deps.announce(plainText(errorView(problem).title));
    if (problem.kind === "tailnet") {
      // The user turns Tailscale on in its app; back in efr, the pairing tries again.
      this.stopWatch = this.deps.followAppState((active) => {
        if (active && run === this.run) {
          this.retry();
        }
      });
    }
  }

  private set(run: number, state: PairingState): void {
    if (run === this.run) {
      this.atom.set(state);
    }
  }
}

const SCANNER_FAILED: ScanResult = { ok: false, reason: "unavailable" };

/** The failure of a rejected pair mutation. */
function failureOf(error: unknown): PairingFailure {
  if (error instanceof PairingError) {
    return error.failure;
  }
  return { kind: "failed", message: error instanceof Error ? error.message : String(error) };
}
