import { describe, expect, it } from "vitest";

import {
  IDLE_PAIRING,
  isBusy,
  phaseFor,
  type PairingPhase,
  type RepairTarget,
} from "./pairing-session";
import {
  fakePairingSession,
  pairedMachine,
  SESSION_CODE,
  SESSION_DAEMON,
  settle,
  type FakePairingSession,
} from "./testing";

const LAPTOP = "019a9b1c-3d00-7a10-8b20-0000000000d2";
const REPAIR = { id: LAPTOP, name: "laptop" };

function phase(fake: FakePairingSession): PairingPhase {
  return fake.session.state.get().phase;
}

/** Starts a scan and shows the scanner the code. */
async function scanCode(fake: FakePairingSession, repair: RepairTarget | null = null) {
  const outcome = fake.session.scan(repair);
  await settle();
  fake.scanner.show(SESSION_CODE);
  return outcome;
}

describe("the pairing session", () => {
  it("starts at the intro, for a new machine", () => {
    const fake = fakePairingSession();
    expect(fake.session.state.get()).toEqual(IDLE_PAIRING);
  });

  it("scans, pairs with the first code and shows each step", async () => {
    const fake = fakePairingSession();
    const outcome = fake.session.scan(null);
    await settle();
    expect(phase(fake)).toEqual({ kind: "scanning" });
    expect(fake.scanner.launched).toBe(1);
    fake.scanner.show("https://example.com/menu");
    fake.scanner.show(SESSION_CODE);
    expect(await outcome).toBe("pairing");
    expect(fake.haptics).toEqual(["success"]);
    expect(phase(fake)).toEqual({ kind: "connecting", machine: "desktop" });

    await settle();
    const pending = fake.pairs[0];
    expect(pending?.variables).toMatchObject({ code: SESSION_CODE, name: "iPhone 17 Pro" });
    expect(pending?.variables.repair).toBeUndefined();
    pending?.variables.onStep?.({
      step: "confirm",
      name: "iPhone 17 Pro",
      fingerprint: "SHA256:0ed3a6ab957ff6f5",
    });
    expect(phase(fake)).toEqual({
      kind: "confirm",
      machine: "desktop",
      device: "iPhone 17 Pro",
      fingerprint: "SHA256:0ed3a6ab957ff6f5",
      saving: false,
    });
    pending?.variables.onStep?.({ step: "saving" });
    expect(phase(fake)).toMatchObject({ kind: "confirm", saving: true });

    const machine = pairedMachine(SESSION_DAEMON, "desktop");
    pending?.succeed(machine);
    await settle();
    expect(phase(fake)).toEqual({ kind: "paired", machine, repaired: false });
    expect(fake.haptics).toEqual(["success", "success"]);
  });

  it("runs the pairing as a mutation of the query client", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, null);
    await settle();
    const mutations = fake.client.getMutationCache().getAll();
    expect(mutations).toHaveLength(1);
    expect(mutations[0]?.options.mutationKey).toEqual(["machine", "pair"]);
    expect(mutations[0]?.state.status).toBe("pending");
  });

  it("pairs a machine again with its target and says re-paired", async () => {
    const fake = fakePairingSession();
    await scanCode(fake, REPAIR);
    await settle();
    expect(fake.session.state.get().repair).toBe(LAPTOP);
    expect(fake.pairs[0]?.variables.repair).toBe(LAPTOP);
    const machine = pairedMachine(LAPTOP, "laptop");
    fake.pairs[0]?.succeed(machine);
    await settle();
    expect(phase(fake)).toEqual({ kind: "paired", machine, repaired: true });
  });

  it("shows an error with the error haptic and the title for VoiceOver", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, null);
    await settle();
    fake.pairs[0]?.fail({ kind: "refused", message: "the terminal refused the device" });
    await settle();
    expect(phase(fake)).toEqual({
      kind: "failed",
      problem: { kind: "refused", machine: "desktop" },
    });
    expect(fake.haptics).toEqual(["error"]);
    expect(fake.announced).toEqual(["Refused on desktop"]);
  });

  it("names the machine of the re-pair in a wrong machine error", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, REPAIR);
    await settle();
    fake.pairs[0]?.fail({
      kind: "other_machine",
      daemonId: SESSION_DAEMON,
      host: "100.101.102.103",
      fqdn: "homelab.tail3f2a.ts.net",
    });
    await settle();
    expect(phase(fake)).toEqual({
      kind: "failed",
      problem: { kind: "other_machine", code: "homelab", repairing: "laptop" },
    });
  });

  it("goes back to the intro when the flow reports a cancel", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, null);
    await settle();
    fake.pairs[0]?.fail({ kind: "cancelled" });
    await settle();
    expect(phase(fake)).toEqual({ kind: "intro" });
    expect(fake.haptics).toEqual([]);
  });

  it("cancels the request of the pairing, and ignores its late result", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, REPAIR);
    await settle();
    const signal = fake.pairs[0]?.variables.signal;
    expect(signal?.cancelled).toBe(false);
    fake.session.cancel();
    expect(signal?.cancelled).toBe(true);
    expect(fake.session.state.get()).toEqual({ repair: LAPTOP, phase: { kind: "intro" } });
    fake.pairs[0]?.succeed(pairedMachine(LAPTOP, "laptop"));
    await settle();
    expect(phase(fake)).toEqual({ kind: "intro" });
    expect(fake.haptics).toEqual([]);
  });

  it("cancels an open scan", async () => {
    const fake = fakePairingSession();
    const outcome = fake.session.scan(null);
    await settle();
    fake.session.cancel();
    expect(await outcome).toBe("cancelled");
    expect(phase(fake)).toEqual({ kind: "intro" });
    expect(fake.scanner.dismissed).toBe(1);
    expect(fake.scanner.listening).toBe(0);
  });

  it("forgets the target on close", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, REPAIR);
    await settle();
    fake.session.close();
    expect(fake.session.state.get()).toEqual(IDLE_PAIRING);
    expect(fake.pairs[0]?.variables.signal?.cancelled).toBe(true);
  });

  it("starts nothing new while a scan or a pairing runs", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, null);
    expect(await fake.session.scan(null)).toBe("busy");
    expect(fake.scanner.launched).toBe(0);
  });

  it("shows the camera error when the user refused the camera", async () => {
    const fake = fakePairingSession();
    fake.scanner.granted = false;
    expect(await fake.session.scan(null)).toBe("failed");
    expect(phase(fake)).toEqual({ kind: "failed", problem: { kind: "camera" } });
    expect(fake.haptics).toEqual(["error"]);
    fake.session.openSettings();
    expect(fake.opened).toEqual(["settings"]);
  });

  it("shows the no scanner error on a phone without the system scanner", async () => {
    const without = fakePairingSession({ scanner: false });
    expect(await without.session.scan(null)).toBe("failed");
    expect(phase(without)).toEqual({ kind: "failed", problem: { kind: "no_scanner" } });

    const unavailable = fakePairingSession();
    unavailable.scanner.available = false;
    expect(await unavailable.session.scan(null)).toBe("failed");
    expect(phase(unavailable)).toEqual({ kind: "failed", problem: { kind: "no_scanner" } });
  });

  it("tries again with the same code when the app comes back from Tailscale", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, null);
    await settle();
    fake.pairs[0]?.fail({ kind: "unreachable" });
    await settle();
    expect(phase(fake)).toEqual({
      kind: "failed",
      problem: { kind: "tailnet", machine: "desktop" },
    });
    fake.session.openTailscale();
    expect(fake.opened).toEqual(["tailscale"]);
    expect(fake.appState.size).toBe(1);
    for (const listener of fake.appState) {
      listener(false);
    }
    expect(fake.pairs).toHaveLength(1);
    for (const listener of fake.appState) {
      listener(true);
    }
    await settle();
    expect(fake.pairs).toHaveLength(2);
    expect(fake.pairs[1]?.variables.code).toBe(SESSION_CODE);
    expect(phase(fake)).toEqual({ kind: "connecting", machine: "desktop" });
    expect(fake.appState.size).toBe(0);
  });

  it("stops listening to the app state when the user leaves the error", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, null);
    await settle();
    fake.pairs[0]?.fail({ kind: "unreachable" });
    await settle();
    fake.session.close();
    expect(fake.appState.size).toBe(0);
  });

  it("goes back to the intro on Try again", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, null);
    await settle();
    fake.pairs[0]?.fail({ kind: "refused", message: "no" });
    await settle();
    fake.session.tryAgain();
    expect(phase(fake)).toEqual({ kind: "intro" });
  });

  it("makes the new machine the machine of the blank root on Start", async () => {
    const fake = fakePairingSession();
    fake.session.pair(SESSION_CODE, null);
    await settle();
    fake.pairs[0]?.succeed(pairedMachine(SESSION_DAEMON, "desktop"));
    await settle();
    fake.session.start(SESSION_DAEMON);
    expect(fake.selected).toEqual([SESSION_DAEMON]);
    expect(fake.session.state.get()).toEqual(IDLE_PAIRING);
  });

  it("names the machine of a damaged code as the machine", async () => {
    const fake = fakePairingSession();
    fake.session.pair("efr://pair?v=1&d=nope", null);
    expect(phase(fake)).toEqual({ kind: "connecting", machine: "the machine" });
  });
});

describe("phaseFor", () => {
  it("shows the session's phase only to the screen of the same target", () => {
    const state = { repair: LAPTOP, phase: { kind: "scanning" } as const };
    expect(phaseFor(state, LAPTOP)).toEqual({ kind: "scanning" });
    expect(phaseFor(state, null)).toEqual({ kind: "intro" });
  });

  it("knows which phases run", () => {
    expect(isBusy({ kind: "scanning" })).toBe(true);
    expect(isBusy({ kind: "connecting", machine: "a" })).toBe(true);
    expect(isBusy({ kind: "intro" })).toBe(false);
    expect(isBusy({ kind: "failed", problem: { kind: "camera" } })).toBe(false);
  });
});
