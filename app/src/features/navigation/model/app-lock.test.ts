import * as fc from "fast-check";
import { describe, expect, it, vi } from "vitest";

import type { OwnerCheck, OwnerFacts } from "@/lib/lock/owner-check";

import {
  CHECK_RETRY_MS,
  createAppLock,
  initialLock,
  LOCK_AFTER_MS,
  type LockEvent,
  type LockPhase,
  type LockState,
  lockKind,
  lockReduce,
  lockView,
} from "./app-lock";
import { createLinkGate } from "./link-gate";

const T0 = 1_800_000_000_000;
const FAILED: OwnerCheck = { kind: "failed", method: "Face ID" };
const CANCELLED: OwnerCheck = { kind: "cancelled", method: "Face ID" };
const LOCKED: OwnerCheck = { kind: "locked", method: "Face ID" };
const PASSED: OwnerCheck = { kind: "passed" };
const NO_PASSCODE: OwnerCheck = { kind: "no_passcode" };

const OFF = initialLock({ on: false, notice: null });
const ON = initialLock({ on: true, notice: null });

/** `state` after each of `events`. */
function run(state: LockState, ...events: LockEvent[]): LockState {
  return events.reduce(lockReduce, state);
}

const started = (phase: LockPhase = "active", at = T0): LockEvent => ({
  type: "started",
  at,
  phase,
});
const phase = (to: LockPhase, at = T0): LockEvent => ({ type: "phase", at, phase: to });
const ask = (at = T0): LockEvent => ({ type: "ask", at });
const toggle = (on: boolean, at = T0): LockEvent => ({ type: "toggle", at, on });
const checked = (id: number, result: OwnerCheck, at = T0): LockEvent => ({
  type: "checked",
  at,
  id,
  result,
});
const passcode = (present: boolean, at = T0): LockEvent => ({ type: "passcode", at, present });

/** The switch on, unlocked and active, after check 1. */
const OPEN = run(ON, started(), checked(1, PASSED));

describe("the start (T1, T2)", () => {
  it("shows the app at once with the switch off", () => {
    const state = run(OFF, started());
    expect(lockView(state)).toBe("app");
    expect(state.shown).toBe(true);
    expect(state.check).toBeNull();
  });

  it("starts unlock check 1 at a cold start with the switch on", () => {
    const state = run(ON, started());
    expect(lockKind(state)).toBe("checking");
    expect(lockView(state)).toBe("lock");
    expect(state.check).toMatchObject({ id: 1, purpose: "unlock" });
    expect(state.shown).toBe(false);
  });

  it("waits for active when the app starts inactive", () => {
    const state = run(ON, started("inactive"));
    expect(lockKind(state)).toBe("locked");
    expect(state.check).toBeNull();
    expect(run(state, phase("active")).check).toMatchObject({ id: 1, purpose: "unlock" });
  });
});

describe("the phases (T3 to T13)", () => {
  it("does not count the prompt of our own check as going away (T3)", () => {
    const unlocking = run(ON, started());
    expect(run(unlocking, phase("inactive"))).toBe(unlocking);
    const turningOff = run(OPEN, toggle(false));
    expect(run(turningOff, phase("inactive"))).toBe(turningOff);
    const turningOn = run(OFF, started(), toggle(true));
    expect(run(turningOn, phase("inactive"))).toBe(turningOn);
  });

  it("keeps the background and its time on the inactive way back (T4)", () => {
    const away = run(OPEN, phase("background", T0 + 10));
    expect(run(away, phase("inactive", T0 + 20))).toBe(away);
  });

  it("covers an open app that is inactive, and never an app with the switch off (T5)", () => {
    expect(lockView(run(OPEN, phase("inactive")))).toBe("cover");
    expect(lockView(run(OFF, started(), phase("inactive")))).toBe("app");
  });

  it("keeps the time of the first background (T6)", () => {
    const state = run(OPEN, phase("background", T0 + 10), phase("background", T0 + 99));
    expect(state.leftAt).toBe(T0 + 10);
  });

  it("marks the time, asks next time, and marks a running check interrupted (T7)", () => {
    const away = run(OPEN, phase("background", T0 + 10));
    expect(away).toMatchObject({ phase: "background", leftAt: T0 + 10, ask: true });
    expect(lockKind(away)).toBe("away");
    const interrupted = run(ON, started(), phase("background"));
    expect(interrupted.check?.interrupted).toBe(true);
  });

  it("changes nothing for active while active (T8)", () => {
    expect(run(OPEN, phase("active"))).toBe(OPEN);
  });

  it("takes the cover away after inactive and starts no check (T9)", () => {
    const state = run(OPEN, phase("inactive"), phase("active"));
    expect(lockView(state)).toBe("app");
    expect(state.check).toBeNull();
  });

  it("starts no prompt loop after a cancel: inactive then active asks nothing (T9)", () => {
    const cancelled = run(ON, started(), checked(1, CANCELLED));
    const back = run(cancelled, phase("inactive"), phase("active"));
    expect(back.check).toBeNull();
    expect(back.failed).toEqual(CANCELLED);
  });

  it("comes back from the background with the switch off, and asks while locked (T10)", () => {
    expect(lockView(run(OFF, started(), phase("background"), phase("active")))).toBe("app");
    const locked = run(ON, started("background"), phase("active"));
    expect(locked.check).toMatchObject({ purpose: "unlock" });
  });

  it("stays open when back inside 5 minutes (T11)", () => {
    const state = run(
      OPEN,
      phase("background", T0 + 1000),
      phase("active", T0 + 1000 + LOCK_AFTER_MS - 1),
    );
    expect(state.open).toBe(true);
    expect(state.leftAt).toBeNull();
    expect(lockView(state)).toBe("app");
  });

  it("locks and asks when back after exactly 5 minutes (T12)", () => {
    const state = run(
      OPEN,
      phase("background", T0 + 1000),
      phase("active", T0 + 1000 + LOCK_AFTER_MS),
    );
    expect(state.open).toBe(false);
    expect(state.check).toMatchObject({ purpose: "unlock" });
  });

  it("locks without a second check while a turn-off check still runs (T12, T29)", () => {
    const turningOff = run(OPEN, toggle(false, T0 + 1));
    const back = run(
      turningOff,
      phase("background", T0 + 2),
      phase("active", T0 + 2 + LOCK_AFTER_MS),
    );
    expect(back.open).toBe(false);
    expect(back.check?.purpose).toBe("turn_off");
    // The turn-off answer does not count after the lock, and the unlock check follows.
    const after = run(back, checked(back.check?.id ?? 0, PASSED));
    expect(after.on).toBe(true);
    expect(after.check?.purpose).toBe("unlock");
  });

  it("locks when the wall clock went back (T13)", () => {
    const state = run(OPEN, phase("background", T0 + 5000), phase("active", T0 + 1000));
    expect(state.open).toBe(false);
  });
});

describe("Unlock and the switch (T14 to T21)", () => {
  it("changes nothing for a tap on Unlock while off or open (T14)", () => {
    const off = run(OFF, started());
    expect(run(off, ask())).toBe(off);
    expect(run(OPEN, ask())).toBe(OPEN);
  });

  it("asks again after a tap on Unlock, and waits in the background (T15)", () => {
    const failed = run(ON, started(), checked(1, FAILED));
    expect(run(failed, ask()).check).toMatchObject({ id: 2, purpose: "unlock" });
    const away = run(ON, started("background"), ask());
    expect(away.check).toBeNull();
    expect(run(away, phase("active")).check).toMatchObject({ purpose: "unlock" });
  });

  it("replaces a check that gave no answer after 3 seconds, and drops its late answer (T16, T22)", () => {
    const stuck = run(ON, started("active", T0));
    const again = run(stuck, ask(T0 + CHECK_RETRY_MS));
    expect(again.check).toMatchObject({ id: 2, purpose: "unlock" });
    expect(run(again, checked(1, PASSED))).toBe(again);
  });

  it("does not replace a younger check (T17)", () => {
    const stuck = run(ON, started("active", T0));
    expect(run(stuck, ask(T0 + CHECK_RETRY_MS - 1))).toBe(stuck);
  });

  it("does not start a turn-on check while on, while a check runs, or while not active (T18)", () => {
    expect(run(OPEN, toggle(true))).toBe(OPEN);
    const turning = run(OFF, started(), toggle(true));
    expect(run(turning, toggle(true))).toBe(turning);
    const inactive = run(OFF, started("inactive"));
    expect(run(inactive, toggle(true))).toBe(inactive);
  });

  it("starts a turn-on check and leaves the switch off until it passes (T19)", () => {
    const state = run(OFF, started(), toggle(true));
    expect(state.on).toBe(false);
    expect(state.check).toMatchObject({ purpose: "turn_on" });
  });

  it("does not start a turn-off check while off, locked, busy or not active (T20)", () => {
    const off = run(OFF, started());
    expect(run(off, toggle(false))).toBe(off);
    const locked = run(ON, started(), checked(1, FAILED));
    expect(run(locked, toggle(false))).toBe(locked);
    const busy = run(OPEN, toggle(false));
    expect(run(busy, toggle(false))).toBe(busy);
    const inactive = run(OPEN, phase("inactive"));
    expect(run(inactive, toggle(false))).toBe(inactive);
  });

  it("starts a turn-off check and leaves the switch on until it passes (T21)", () => {
    const state = run(OPEN, toggle(false));
    expect(state.on).toBe(true);
    expect(state.check).toMatchObject({ purpose: "turn_off" });
  });
});

describe("the answers of iOS (T22 to T32)", () => {
  it("ignores an answer with an old id or with no check (T22)", () => {
    const state = run(ON, started());
    expect(run(state, checked(7, PASSED))).toBe(state);
    expect(run(OPEN, checked(1, PASSED))).toBe(OPEN);
  });

  it("opens on a passed unlock (T23)", () => {
    expect(lockKind(OPEN)).toBe("unlocked");
    expect(lockView(OPEN)).toBe("app");
    expect(OPEN.shown).toBe(true);
  });

  it("counts the wait from the background when a pass comes while away (T23)", () => {
    const pending = run(ON, started("active", T0));
    const away = run(pending, phase("background", T0 + 10), checked(1, PASSED));
    // An interrupted check that still passed opens; the wait counts from T0 + 10.
    expect(away.open).toBe(true);
    expect(away.leftAt).toBe(T0 + 10);
    expect(run(away, phase("active", T0 + 10 + LOCK_AFTER_MS)).open).toBe(false);
  });

  it("turns the switch off with the notice on no passcode at an unlock (T24, D5)", () => {
    const state = run(ON, started(), checked(1, NO_PASSCODE));
    expect(state).toMatchObject({ on: false, open: false, notice: "no_passcode" });
    expect(lockView(state)).toBe("app");
  });

  it("keeps the lock with the line on failed, cancelled and locked, and asks nothing (T25)", () => {
    for (const result of [FAILED, CANCELLED, LOCKED]) {
      const state = run(ON, started(), checked(1, result));
      expect(state.failed).toEqual(result);
      expect(state.check).toBeNull();
      expect(lockView(state)).toBe("lock");
    }
  });

  it("shows no line for an interrupted check, and asks again in the foreground (T26)", () => {
    const left = run(ON, started(), phase("background"), checked(1, CANCELLED));
    expect(left.failed).toBeNull();
    expect(run(left, phase("active")).check).toMatchObject({ id: 2, purpose: "unlock" });
  });

  it("asks at once when the answer of an interrupted check comes after active (T26)", () => {
    const back = run(ON, started(), phase("background"), phase("active"));
    // Check 1 still runs: no second check yet.
    expect(back.check?.id).toBe(1);
    expect(run(back, checked(1, CANCELLED)).check).toMatchObject({ id: 2 });
  });

  it("turns the switch on and opens after a passed turn-on, and clears the notice (T27)", () => {
    const noticed = initialLock({ on: false, notice: "no_passcode" });
    const state = run(noticed, started(), toggle(true), checked(1, PASSED));
    expect(state).toMatchObject({ on: true, open: true, notice: null });
    expect(lockView(state)).toBe("app");
  });

  it("leaves the switch off when a turn-on does not pass (T28)", () => {
    for (const result of [FAILED, CANCELLED, LOCKED, NO_PASSCODE]) {
      const state = run(OFF, started(), toggle(true), checked(1, result));
      expect(state.on).toBe(false);
      expect(state.check).toBeNull();
      expect(state.notice).toBeNull();
    }
  });

  it("turns the switch off after a passed turn-off (T30)", () => {
    const state = run(OPEN, toggle(false), checked(2, PASSED));
    expect(state).toMatchObject({ on: false, open: false, check: null });
    expect(lockView(state)).toBe("app");
  });

  it("turns the switch off with the notice on no passcode at a turn-off (T31)", () => {
    const state = run(OPEN, toggle(false), checked(2, NO_PASSCODE));
    expect(state).toMatchObject({ on: false, notice: "no_passcode" });
  });

  it("keeps the switch on and the app open when a turn-off does not pass (T32)", () => {
    const state = run(OPEN, toggle(false), checked(2, CANCELLED));
    expect(state).toMatchObject({ on: true, open: true, check: null });
  });
});

describe("a passcode that went (T33 to T35)", () => {
  it("changes nothing for a passcode that is there, or while off (T33)", () => {
    expect(run(OPEN, passcode(true))).toBe(OPEN);
    const off = run(OFF, started());
    expect(run(off, passcode(false))).toBe(off);
  });

  it("locks an open app and its check turns the switch off (T34, D5)", () => {
    const state = run(OPEN, passcode(false));
    expect(state.open).toBe(false);
    expect(state.check).toMatchObject({ purpose: "unlock" });
    const off = run(state, checked(state.check?.id ?? 0, NO_PASSCODE));
    expect(off).toMatchObject({ on: false, notice: "no_passcode" });
  });

  it("changes nothing while locked: the unlock check finds it (T35)", () => {
    const locked = run(ON, started(), checked(1, FAILED));
    expect(run(locked, passcode(false))).toBe(locked);
  });
});

describe("the kinds and the views", () => {
  it("derives the kind and the view of each state", () => {
    const cases: [LockState, string, string][] = [
      [run(OFF, started()), "off", "app"],
      [run(ON, started()), "checking", "lock"],
      [run(ON, started(), checked(1, FAILED)), "locked", "lock"],
      [OPEN, "unlocked", "app"],
      [run(OPEN, phase("inactive")), "unlocked", "cover"],
      [run(OPEN, phase("background")), "away", "cover"],
    ];
    for (const [state, kind, view] of cases) {
      expect([lockKind(state), lockView(state)]).toEqual([kind, view]);
    }
  });

  it("has a way out of every lock: a pass, a missing passcode, or Unlock then a pass", () => {
    for (const result of [FAILED, CANCELLED, LOCKED]) {
      const locked = run(ON, started(), checked(1, result));
      expect(run(locked, ask(), checked(2, PASSED)).open).toBe(true);
      expect(run(locked, ask(), checked(2, NO_PASSCODE)).on).toBe(false);
    }
    const stuck = run(ON, started("active", T0));
    expect(run(stuck, ask(T0 + CHECK_RETRY_MS), checked(2, PASSED)).open).toBe(true);
  });
});

const arbPhase = fc.constantFrom<LockPhase>("active", "inactive", "background");
const arbResult = fc.constantFrom<OwnerCheck>(PASSED, FAILED, CANCELLED, LOCKED, NO_PASSCODE);
const arbEvent: fc.Arbitrary<(at: number, checks: number) => LockEvent> = fc.oneof(
  arbPhase.map((to) => (at: number) => phase(to, at)),
  fc.constant((at: number) => ask(at)),
  fc.boolean().map((on) => (at: number) => toggle(on, at)),
  fc.tuple(fc.nat(3), arbResult).map(
    ([back, result]) =>
      (at: number, checks: number) =>
        checked(checks - back, result, at),
  ),
  fc.boolean().map((present) => (at: number) => passcode(present, at)),
);

describe("the rules over any sequence of events", () => {
  it("opens only on a pass, switches only on an answer, and only counts checks up", () => {
    fc.assert(
      fc.property(
        fc.boolean(),
        arbPhase,
        fc.array(fc.tuple(arbEvent, fc.nat(LOCK_AFTER_MS * 2)), { maxLength: 40 }),
        (on, first, steps) => {
          let state = run(initialLock({ on, notice: null }), started(first));
          let at = T0;
          for (const [make, gap] of steps) {
            at += gap;
            const event = make(at, state.checks);
            const next = lockReduce(state, event);
            // Opening needs a passed answer to a running check.
            const opened = next.open && !state.open && next.on;
            const passed = event.type === "checked" && event.result.kind === "passed";
            expect(!opened || passed).toBe(true);
            // The switch moves only on an answer of iOS.
            expect(next.on === state.on || event.type === "checked").toBe(true);
            expect(next.checks).toBeGreaterThanOrEqual(state.checks);
            // A new check always takes the next id.
            const fresh = next.check !== null && next.check.id !== state.check?.id;
            expect(!fresh || next.check?.id === next.checks).toBe(true);
            state = next;
          }
        },
      ),
      { numRuns: 300 },
    );
  });
});

/** Lets the promises of the runner settle. */
function settle(): Promise<unknown> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/** A promise and the function that resolves it. */
function deferred(): { readonly promise: Promise<void>; readonly resolve: () => void } {
  let done: (() => void) | undefined;
  const promise = new Promise<void>((resolve) => {
    done = resolve;
  });
  return { promise, resolve: () => done?.() };
}

/** A lock with fake effects; each check waits for `answer`. */
function testLock(saved: { on: boolean; notice: null | "no_passcode" }, facts?: OwnerFacts) {
  let now = T0;
  const reasons: string[] = [];
  const answers: ((result: OwnerCheck) => void)[] = [];
  const saves: unknown[] = [];
  const switches: boolean[] = [];
  const locks: number[] = [];
  const announced: string[] = [];
  const errors: unknown[] = [];
  let present = true;
  const lock = createAppLock({
    saved,
    now: () => now,
    checkOwner: (reason) => {
      reasons.push(reason);
      return new Promise((resolve) => answers.push(resolve));
    },
    ownerFacts: () => Promise.resolve(facts ?? { passcode: true, method: "Face ID" }),
    hasPasscode: () => Promise.resolve(present),
    save: (value) => saves.push(value),
    onSwitch: (on) => switches.push(on),
    onLock: () => locks.push(now),
    announce: (line) => announced.push(line),
    onError: (error) => errors.push(error),
  });
  return {
    lock,
    reasons,
    saves,
    switches,
    locks,
    announced,
    errors,
    settle,
    answer: async (result: OwnerCheck) => {
      const next = answers.shift();
      next?.(result);
      await settle();
    },
    at: (to: number) => {
      now = to;
    },
    losePasscode: () => {
      present = false;
    },
  };
}

describe("the runner", () => {
  it("asks iOS one time with 'Unlock efr' at a cold start, and never twice at once", async () => {
    const t = testLock({ on: true, notice: null });
    t.lock.start("active");
    await t.settle();
    t.lock.unlock();
    t.lock.phase("inactive");
    t.lock.phase("active");
    await t.settle();
    expect(t.reasons).toEqual(["Unlock efr"]);
    await t.answer(PASSED);
    expect(lockView(t.lock.store.get())).toBe("app");
  });

  it("turns a rejected check into a failure and reports the error", async () => {
    const lock = createAppLock({
      saved: { on: true, notice: null },
      now: () => T0,
      checkOwner: () => Promise.reject(new Error("no LAContext")),
      ownerFacts: () => Promise.resolve({ passcode: true, method: "Face ID" }),
      hasPasscode: () => Promise.resolve(true),
      save: () => undefined,
      onSwitch: () => undefined,
      onError: vi.fn<(error: unknown) => void>(),
    });
    lock.start("active");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(lock.store.get().failed).toEqual(FAILED);
  });

  it("saves and reports the switch only when it changes, with the reason of each check", async () => {
    const t = testLock({ on: false, notice: null }, { passcode: true, method: "Touch ID" });
    t.lock.start("active");
    t.lock.setOn(true);
    await t.settle();
    expect(t.reasons).toEqual(["Turn on Touch ID unlock"]);
    await t.answer(PASSED);
    t.lock.setOn(false);
    await t.settle();
    expect(t.reasons.at(-1)).toBe("Turn off Touch ID unlock");
    await t.answer(PASSED);
    expect(t.saves).toEqual([
      { on: true, notice: null },
      { on: false, notice: null },
    ]);
    expect(t.switches).toEqual([true, false]);
  });

  it("closes the keyboard when the lock comes back, not for the cover, and says the line", async () => {
    const t = testLock({ on: true, notice: null });
    // A cold start shows the lock from the first state: no keyboard is up yet.
    t.lock.start("active");
    await t.settle();
    await t.answer(CANCELLED);
    expect(t.announced).toEqual(["You cancelled Face ID."]);
    t.lock.unlock();
    await t.settle();
    await t.answer(PASSED);
    t.lock.phase("inactive");
    expect(t.locks).toHaveLength(0);
    t.lock.phase("background");
    t.at(T0 + LOCK_AFTER_MS);
    t.lock.phase("active");
    expect(t.locks).toHaveLength(1);
  });

  it("asks for the passcode on a return inside the wait, and locks when it went", async () => {
    const t = testLock({ on: true, notice: null });
    t.lock.start("active");
    await t.settle();
    await t.answer(PASSED);
    t.at(T0 + 10);
    t.lock.phase("background");
    t.losePasscode();
    t.at(T0 + 20);
    t.lock.phase("active");
    await t.settle();
    expect(t.lock.store.get().open).toBe(false);
    await t.answer(NO_PASSCODE);
    expect(t.lock.store.get()).toMatchObject({ on: false, notice: "no_passcode" });
    expect(t.saves.at(-1)).toEqual({ on: false, notice: "no_passcode" });
  });

  it("resolves whenShown at once while the app shows, and after the unlock otherwise", async () => {
    const off = testLock({ on: false, notice: null });
    await expect(off.lock.whenShown()).resolves.toBeUndefined();
    const t = testLock({ on: true, notice: null });
    t.lock.start("active");
    let shown = false;
    void t.lock.whenShown().then(() => {
      shown = true;
      return shown;
    });
    await t.settle();
    expect(shown).toBe(false);
    await t.answer(PASSED);
    expect(shown).toBe(true);
  });
});

describe("the gate of the links", () => {
  it("holds links until it follows a lock, then lets them pass in order when the app shows", async () => {
    const gate = createLinkGate();
    const passed: string[] = [];
    void gate.wait("/pair").then((path) => passed.push(path));
    void gate.wait("/conversations").then((path) => passed.push(path));
    await Promise.resolve();
    expect(passed).toEqual([]);
    const shown = deferred();
    gate.follow({ whenShown: () => shown.promise });
    await Promise.resolve();
    expect(passed).toEqual([]);
    shown.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(passed).toEqual(["/pair", "/conversations"]);
    await expect(gate.wait("/x")).resolves.toBe("/x");
  });
});
