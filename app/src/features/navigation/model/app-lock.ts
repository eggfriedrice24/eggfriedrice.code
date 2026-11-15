// The Face ID unlock of the app (round 9, S-Lock; research/app-lock-code-design.md): a pure
// state machine and its runner.
//
// - The switch of Settings is off by default. On, efr asks for Face ID, Touch ID or the
//   passcode of the iPhone when it opens, and when it comes back after 5 minutes or more
//   in the background (the wall clock from "background"). A cold start is always locked:
//   whether the app is open lives in memory only.
// - The switch turns on and off only after a passing check. Off sets every composer back
//   to cautious (the services do it in `onSwitch`). An iPhone that lost its passcode turns
//   the switch off, with a notice (D5), so the owner is never locked out.
// - While the app is open and not active, the window shows a cover, so the app switcher
//   never shows a transcript (D7). Our own check makes the app "inactive" too; that is
//   not "going away".
// - One check runs at a time. Only `settle` starts an unlock check, and only when `ask` is
//   true, so the return from the prompt of iOS never starts a second prompt by itself.
//
// `lockReduce` is pure and total; the runner gives each event the time of the clock.
//
// Pure TypeScript on TanStack Store.

import { createStore, type Readable } from "@tanstack/store";

import {
  type CheckOwner,
  type CheckPurpose,
  DEFAULT_FACTS,
  type FailedCheck,
  type OwnerCheck,
  type OwnerFacts,
} from "@/lib/lock/owner-check";
import { checkReason, lockLine } from "./owner-check-format";

/** The time in the background after which the app locks again (D1, fixed). */
export const LOCK_AFTER_MS = 5 * 60_000;

/** After this time, a tap on Unlock replaces a check that gave no answer. */
export const CHECK_RETRY_MS = 3_000;

/** The phase of the app, as iOS reports it. */
export type LockPhase = "active" | "inactive" | "background";

/** Why efr turned the switch off: the iPhone has no passcode (D5). */
export type LockNotice = "no_passcode";

/** A check of ours that runs. */
export interface LockCheck {
  readonly id: number;
  readonly purpose: CheckPurpose;
  readonly startedAt: number;
  /** True when the app went to the background while the check ran. */
  readonly interrupted: boolean;
}

/** The state of the lock. */
export interface LockState {
  /** The switch. Saved. */
  readonly on: boolean;
  /** Why efr turned the switch off. Saved. */
  readonly notice: LockNotice | null;
  /** True when the owner passed a check and the wait did not end. Memory only. */
  readonly open: boolean;
  /** True from the first time that the view is "app". Memory only. */
  readonly shown: boolean;
  readonly phase: LockPhase;
  /** The wall clock time of the last "background", or null in the foreground. */
  readonly leftAt: number | null;
  /** True when the next time in the foreground asks iOS without a tap. */
  readonly ask: boolean;
  readonly check: LockCheck | null;
  /** The last unlock check that did not pass, for the line of the lock screen. */
  readonly failed: FailedCheck | null;
  /** The number of checks so far; the id of the next one is `checks + 1`. */
  readonly checks: number;
}

/** What happens to the lock. `at` is the wall clock in milliseconds. */
export type LockEvent =
  | { readonly type: "started"; readonly at: number; readonly phase: LockPhase }
  | { readonly type: "phase"; readonly at: number; readonly phase: LockPhase }
  | { readonly type: "ask"; readonly at: number }
  | { readonly type: "toggle"; readonly at: number; readonly on: boolean }
  | {
      readonly type: "checked";
      readonly at: number;
      readonly id: number;
      readonly result: OwnerCheck;
    }
  | { readonly type: "passcode"; readonly at: number; readonly present: boolean };

/** The saved part of the lock. */
export interface SavedLock {
  readonly on: boolean;
  readonly notice: LockNotice | null;
}

/** The lock before the app started. */
export function initialLock(saved: SavedLock): LockState {
  return {
    on: saved.on,
    notice: saved.notice,
    open: false,
    shown: false,
    phase: "inactive",
    leftAt: null,
    ask: false,
    check: null,
    failed: null,
    checks: 0,
  };
}

/** The states of plan section 2. */
export type LockKind = "off" | "locked" | "checking" | "unlocked" | "away";

/** What the window shows. */
export type LockView = "app" | "cover" | "lock";

/** The state of plan section 2. */
export function lockKind(state: LockState): LockKind {
  if (!state.on) {
    return "off";
  }
  if (!state.open) {
    return state.check?.purpose === "unlock" ? "checking" : "locked";
  }
  return state.phase === "background" ? "away" : "unlocked";
}

/** What the window shows. Use it as the selector of `useSelector`. */
export function lockView(state: LockState): LockView {
  if (!state.on) {
    return "app";
  }
  if (!state.open) {
    return "lock";
  }
  return state.phase === "active" ? "app" : "cover";
}

export function selectLockOn(state: LockState): boolean {
  return state.on;
}

export function selectShown(state: LockState): boolean {
  return state.shown;
}

export function selectFailed(state: LockState): FailedCheck | null {
  return state.failed;
}

/** The purpose of the check that runs, or null. */
export function selectChecking(state: LockState): CheckPurpose | null {
  return state.check?.purpose ?? null;
}

export function selectNotice(state: LockState): LockNotice | null {
  return state.notice;
}

/** `state` with a new check of `purpose`. */
function withCheck(state: LockState, purpose: CheckPurpose, at: number): LockState {
  const id = state.checks + 1;
  return { ...state, check: { id, purpose, startedAt: at, interrupted: false }, checks: id };
}

/** The switch off because the iPhone has no passcode (D5). */
function noPasscode(state: LockState): LockState {
  return {
    ...state,
    on: false,
    open: false,
    notice: "no_passcode",
    check: null,
    failed: null,
    ask: false,
  };
}

/** The rows T3 to T13. */
function stepPhase(state: LockState, phase: LockPhase, at: number): LockState {
  if (phase === "inactive") {
    // T3: a check of ours runs; its prompt is not "going away". T4: on the way back.
    if (state.check !== null || state.phase === "background" || state.phase === "inactive") {
      return state;
    }
    return { ...state, phase: "inactive" };
  }
  if (phase === "background") {
    if (state.phase === "background") {
      return state;
    }
    const check = state.check === null ? null : { ...state.check, interrupted: true };
    return { ...state, phase: "background", leftAt: at, ask: state.on || state.ask, check };
  }
  if (state.phase === "active") {
    return state;
  }
  if (state.phase === "inactive") {
    return { ...state, phase: "active" };
  }
  if (!state.on || !state.open) {
    return { ...state, phase: "active", leftAt: null };
  }
  const away = state.leftAt === null ? 0 : at - state.leftAt;
  if (away >= 0 && away < LOCK_AFTER_MS) {
    return { ...state, phase: "active", leftAt: null, ask: false };
  }
  // T12, and T13: a wall clock that went back cannot tell the wait.
  return { ...state, phase: "active", leftAt: null, open: false, failed: null, ask: true };
}

/** The rows T22 to T32. */
function stepChecked(state: LockState, id: number, result: OwnerCheck): LockState {
  const check = state.check;
  if (check === null || check.id !== id) {
    return state;
  }
  if (check.purpose === "unlock") {
    if (result.kind === "passed") {
      return { ...state, open: true, failed: null, check: null, ask: false };
    }
    if (result.kind === "no_passcode") {
      return noPasscode(state);
    }
    // T26: iOS cancelled the prompt because the app left; `ask` asks again later.
    return check.interrupted
      ? { ...state, failed: null, check: null }
      : { ...state, failed: result, check: null };
  }
  if (check.purpose === "turn_on") {
    return result.kind === "passed"
      ? { ...state, on: true, open: true, notice: null, check: null }
      : { ...state, check: null };
  }
  if (!state.on || !state.open) {
    return { ...state, check: null };
  }
  if (result.kind === "passed") {
    return { ...state, on: false, open: false, check: null, failed: null, ask: false };
  }
  return result.kind === "no_passcode" ? noPasscode(state) : { ...state, check: null };
}

/** The transition table of the design (T1 to T35), before `settle`. */
function step(state: LockState, event: LockEvent): LockState {
  switch (event.type) {
    case "started":
      return state.on
        ? { ...state, phase: event.phase, open: false, ask: true }
        : { ...state, phase: event.phase };
    case "phase":
      return stepPhase(state, event.phase, event.at);
    case "ask": {
      if (!state.on || state.open) {
        return state;
      }
      if (state.check === null) {
        return state.ask ? state : { ...state, ask: true };
      }
      return event.at - state.check.startedAt >= CHECK_RETRY_MS
        ? { ...state, check: null, ask: true }
        : state;
    }
    case "toggle": {
      const busy = state.check !== null || state.phase !== "active";
      if (event.on) {
        return state.on || busy ? state : withCheck(state, "turn_on", event.at);
      }
      return !state.on || !state.open || busy ? state : withCheck(state, "turn_off", event.at);
    }
    case "checked":
      return stepChecked(state, event.id, event.result);
    default:
      if (event.present || !state.on || !state.open) {
        return state;
      }
      return { ...state, open: false, failed: null, ask: true };
  }
}

/** S1 and S2: start an unlock check when one is due, and mark the first "app" view. */
function settle(state: LockState, at: number): LockState {
  let next = state;
  if (next.on && !next.open && next.check === null && next.ask && next.phase === "active") {
    next = { ...withCheck(next, "unlock", at), ask: false, failed: null };
  }
  if (!next.shown && lockView(next) === "app") {
    next = { ...next, shown: true };
  }
  return next;
}

/** The pure machine. `at` of the event is the injected clock. */
export function lockReduce(state: LockState, event: LockEvent): LockState {
  return settle(step(state, event), event.at);
}

/** What the lock needs from the app. */
export interface AppLockDeps {
  readonly saved: SavedLock;
  /** Wall clock, milliseconds since the epoch (`Date.now`). */
  readonly now: () => number;
  readonly checkOwner: CheckOwner;
  readonly ownerFacts: () => Promise<OwnerFacts>;
  readonly hasPasscode: () => Promise<boolean>;
  /** Saves the switch and the notice. */
  readonly save: (saved: SavedLock) => void;
  /** The switch changed. The services remove auto here (D3). */
  readonly onSwitch: (on: boolean) => void;
  /** The view became "lock". The app closes the keyboard and an open action sheet here. */
  readonly onLock?: () => void;
  /** Says a line to VoiceOver. */
  readonly announce?: (line: string) => void;
  readonly onError?: (error: unknown) => void;
}

/** The lock of the app. */
export interface AppLock {
  readonly store: Readable<LockState>;
  /** The cold start. Call it one time. */
  start(phase: LockPhase): void;
  /** The app changed its phase. */
  phase(phase: LockPhase): void;
  /** The tap on Unlock. */
  unlock(): void;
  /** The tap on the switch. The switch moves only after a passing check. */
  setOn(on: boolean): void;
  /** What the iPhone can check, for the query of Settings and the mode menu. */
  facts(): Promise<OwnerFacts>;
  /**
   * The facts of the last read, for the lock screen: a check reads them just before it
   * asks iOS, so the lock screen reads them no second time.
   */
  readonly owner: Readable<OwnerFacts>;
  /** Resolves when the view is "app". */
  whenShown(): Promise<void>;
}

/** Makes the lock from the saved switch. It does nothing before `start`. */
export function createAppLock(deps: AppLockDeps): AppLock {
  const store = createStore<LockState>(initialLock(deps.saved));
  const owner = createStore<OwnerFacts>(DEFAULT_FACTS);
  let waiters: (() => void)[] = [];

  const facts = async (): Promise<OwnerFacts> => {
    try {
      const read = await deps.ownerFacts();
      owner.setState(() => read);
    } catch (error) {
      deps.onError?.(error);
    }
    return owner.state;
  };

  const runCheck = async (check: LockCheck): Promise<void> => {
    const { method } = await facts();
    let result: OwnerCheck;
    try {
      result = await deps.checkOwner(checkReason(check.purpose, method), method);
    } catch (error) {
      deps.onError?.(error);
      result = { kind: "failed", method };
    }
    dispatch({ type: "checked", at: deps.now(), id: check.id, result });
  };

  const askPasscode = async (): Promise<void> => {
    try {
      const present = await deps.hasPasscode();
      dispatch({ type: "passcode", at: deps.now(), present });
    } catch (error) {
      deps.onError?.(error);
    }
  };

  function dispatch(event: LockEvent): void {
    const prev = store.state;
    const next = lockReduce(prev, event);
    if (next === prev) {
      return;
    }
    store.setState(() => next);
    if (next.check !== null && next.check.id !== prev.check?.id) {
      void runCheck(next.check);
    }
    if (next.on !== prev.on || next.notice !== prev.notice) {
      deps.save({ on: next.on, notice: next.notice });
    }
    if (next.on !== prev.on) {
      deps.onSwitch(next.on);
    }
    if (lockView(prev) !== "lock" && lockView(next) === "lock") {
      deps.onLock?.();
    }
    if (next.failed !== null && next.failed !== prev.failed) {
      deps.announce?.(lockLine(next.failed));
    }
    // T11: back inside the wait. A passcode that went meanwhile locks (T34, D5).
    const back = event.type === "phase" && event.phase === "active" && prev.phase === "background";
    if (back && prev.on && prev.open && next.open) {
      void askPasscode();
    }
    if (lockView(next) === "app" && waiters.length > 0) {
      const ready = waiters;
      waiters = [];
      for (const resolve of ready) {
        resolve();
      }
    }
  }

  return {
    store,
    start(phase: LockPhase): void {
      dispatch({ type: "started", at: deps.now(), phase });
    },
    phase(phase: LockPhase): void {
      dispatch({ type: "phase", at: deps.now(), phase });
    },
    unlock(): void {
      dispatch({ type: "ask", at: deps.now() });
    },
    setOn(on: boolean): void {
      dispatch({ type: "toggle", at: deps.now(), on });
    },
    facts,
    owner,
    whenShown(): Promise<void> {
      if (lockView(store.state) === "app") {
        return Promise.resolve();
      }
      return new Promise((resolve) => {
        waiters.push(resolve);
      });
    },
  };
}
