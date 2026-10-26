import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider } from "@tanstack/react-query";
import { Stack, router, useLocalSearchParams } from "expo-router";
import { act, fireEvent, renderRouter, screen } from "expo-router/testing-library";
import { Text } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { machineKeys } from "@/features/machines/machine-queries";
import { FakeStreams } from "@/features/machines/model/testing";
import { createRuntime } from "@/features/machines/model/runtime";
import { RuntimeContext } from "@/features/machines/use-runtime";
import { createPreferences } from "@/features/settings/model/preferences";
import { PreferencesContext } from "@/features/settings/use-preferences";
import type { KeyValueStorage } from "@/lib/store/key-value";

import { pairRouteMachine } from "../model/pair-route-schemas";
import { IDLE_PAIRING } from "../model/pairing-session";
import {
  fakePairingSession,
  pairedMachine,
  SESSION_CODE,
  SESSION_DAEMON,
  type FakePairingSession,
} from "../model/testing";
import { PairingSessionContext } from "../use-pairing";
import { NoMachineRootScreen } from "./no-machine-root-screen";
import { PairScreen } from "./pair-screen";

jest.mock("@/features/pairing/native/native-pairing", () => ({
  nativeScanner: () => null,
  suggestedDeviceName: () => "iPhone",
}));
jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const LAPTOP = "019a9b1c-3d00-7a10-8b20-0000000000d2";

const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

/** A key-value store in memory, for the preferences. */
class MemoryStorage implements KeyValueStorage {
  private readonly values = new Map<string, string>();
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

let disposers: (() => void)[] = [];

afterEach(() => {
  for (const dispose of disposers.splice(0)) {
    dispose();
  }
});

/** The pairing route, as src/app/pair.tsx. */
function Pair() {
  return <PairScreen machine={pairRouteMachine(useLocalSearchParams())} />;
}

/** The routes of the app around the pairing screens, with a fake session. */
function app(fake: FakePairingSession, root: "blank" | "no-machine" = "blank") {
  // A runtime without pairing effects: the screens get the fake session, and the status
  // query of a paired machine fails, as it does while the machine is not connected.
  const runtime = createRuntime({
    pairing: null,
    random: () => 0.5,
    streams: new FakeStreams().factory,
    schedule: (run) => {
      run();
      return () => undefined;
    },
  });
  disposers.push(
    () => runtime.dispose(),
    () => fake.session.dispose(),
    () => fake.client.clear(),
  );
  const preferences = createPreferences({ storage: new MemoryStorage() });
  // NOTE: the fake session keeps the machine of Start itself; the preferences are here
  // because the screens read them.
  function Layout() {
    return (
      <SafeAreaProvider initialMetrics={METRICS}>
        <QueryClientProvider client={fake.client}>
          <RuntimeContext value={runtime}>
            <PreferencesContext value={preferences}>
              <PairingSessionContext value={fake.session}>
                <Stack>
                  <Stack.Screen name="pair" options={{ presentation: "fullScreenModal" }} />
                </Stack>
              </PairingSessionContext>
            </PreferencesContext>
          </RuntimeContext>
        </QueryClientProvider>
      </SafeAreaProvider>
    );
  }
  return {
    routes: {
      _layout: Layout,
      index: root === "blank" ? () => <Text>blank root</Text> : NoMachineRootScreen,
      "c/[id]": () => <Text>a conversation</Text>,
      settings: () => <Text>settings</Text>,
      pair: Pair,
    },
  };
}

async function go(path: string): Promise<void> {
  await act(async () => {
    router.push(path);
  });
}

/** Lets the promise jobs of the session run. */
async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 20; i++) {
      await Promise.resolve();
    }
  });
}

/** Presses Scan code and shows the scanner the code. */
async function scan(fake: FakePairingSession): Promise<void> {
  await fireEvent.press(screen.getByRole("button", { name: "Scan code" }));
  await flush();
  await act(async () => {
    fake.scanner.show(SESSION_CODE);
  });
  await flush();
}

describe("the pairing modal", () => {
  it("pairs a new machine, then starts a blank root on it", async () => {
    const fake = fakePairingSession();
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go("/c/one");
    await go("/pair");
    expect(screen.getByRole("header", { name: "Pair a machine" })).toBeOnTheScreen();

    await scan(fake);
    expect(fake.scanner.launched).toBe(1);
    expect(screen.getByRole("header", { name: "Connecting to desktop" })).toBeOnTheScreen();

    await act(async () => {
      fake.pairs[0]?.variables.onStep?.({
        step: "confirm",
        name: "iPhone 17 Pro",
        fingerprint: "SHA256:0ed3a6ab957ff6f5",
      });
    });
    expect(screen.getByRole("header", { name: "Confirm on desktop" })).toBeOnTheScreen();
    expect(screen.getByLabelText("key, SHA256:0ed3a6ab957ff6f5")).toBeOnTheScreen();

    await act(async () => {
      fake.pairs[0]?.succeed(pairedMachine(SESSION_DAEMON, "desktop"));
    });
    await flush();
    expect(screen.getByText("✓ paired")).toBeOnTheScreen();

    await fireEvent.press(screen.getByRole("button", { name: "Start" }));
    expect(screen).toHavePathname("/");
    expect(screen.getByText("blank root")).toBeOnTheScreen();
    expect(fake.selected).toEqual([SESSION_DAEMON]);
    expect(fake.session.state.get()).toEqual(IDLE_PAIRING);
  });

  it("shows the efrd line once the machine is connected", async () => {
    const fake = fakePairingSession();
    fake.client.setQueryData(machineKeys.status(SESSION_DAEMON), {
      version: "0.0.5",
      protocol: 1,
      scopes: [],
      seenAt: 0,
    });
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go("/pair");
    await scan(fake);
    await act(async () => {
      fake.pairs[0]?.succeed(pairedMachine(SESSION_DAEMON, "desktop"));
    });
    await flush();
    expect(screen.getByText("efrd 0.0.5 · protocol 1")).toBeOnTheScreen();
  });

  it("stops the pairing on Close and leaves to where the user was", async () => {
    const fake = fakePairingSession();
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go("/c/one");
    await go("/pair");
    await scan(fake);
    const signal = fake.pairs[0]?.variables.signal;
    await fireEvent.press(screen.getByRole("button", { name: "Close" }));
    expect(signal?.cancelled).toBe(true);
    expect(screen).toHavePathname("/c/one");
    expect(fake.session.state.get()).toEqual(IDLE_PAIRING);
  });

  it("goes back to the intro on Cancel", async () => {
    const fake = fakePairingSession();
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go("/pair");
    await scan(fake);
    await fireEvent.press(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Scan code" })).toBeOnTheScreen();
    expect(screen).toHavePathname("/pair");
  });

  it("re-pairs a machine and goes back to where the user was", async () => {
    const fake = fakePairingSession({ machines: [pairedMachine(LAPTOP, "laptop")] });
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go("/c/one");
    await go(`/pair?machine=${LAPTOP}`);
    expect(screen.getByRole("header", { name: "Re-pair laptop" })).toBeOnTheScreen();
    await scan(fake);
    expect(fake.pairs[0]?.variables.repair).toBe(LAPTOP);
    await act(async () => {
      fake.pairs[0]?.succeed(pairedMachine(LAPTOP, "laptop"));
    });
    await flush();
    expect(screen.getByText("✓ re-paired")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Start" }));
    expect(screen).toHavePathname("/c/one");
    expect(fake.selected).toEqual([]);
  });

  it("names both machines when a re-pair scans the code of another one", async () => {
    const fake = fakePairingSession({ machines: [pairedMachine(LAPTOP, "laptop")] });
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go(`/pair?machine=${LAPTOP}`);
    await scan(fake);
    await act(async () => {
      fake.pairs[0]?.fail({
        kind: "other_machine",
        daemonId: SESSION_DAEMON,
        host: "100.64.0.3",
        fqdn: "homelab.tail3f2a.ts.net",
      });
    });
    await flush();
    expect(screen.getByRole("header", { name: "This code is from homelab" })).toBeOnTheScreen();
    expect(
      screen.getByText(
        "You're re-pairing laptop, but this code came from homelab. " +
          "Run the command on laptop and scan that code.",
      ),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Scan again" }));
    await flush();
    expect(fake.scanner.launched).toBe(2);
  });

  it("closes on Done when efrd is too old", async () => {
    const fake = fakePairingSession();
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go("/c/one");
    await go("/pair");
    await scan(fake);
    await act(async () => {
      fake.pairs[0]?.fail({ kind: "protocol_mismatch", protocol: 0, version: "0.0.3" });
    });
    await flush();
    expect(screen.getByRole("header", { name: "efr on desktop is too old" })).toBeOnTheScreen();
    expect(fake.announced).toEqual(["efr on desktop is too old"]);
    await fireEvent.press(screen.getByRole("button", { name: "Done" }));
    expect(screen).toHavePathname("/c/one");
  });

  it("resets to the root on Close after a new machine paired, without a switch", async () => {
    const fake = fakePairingSession();
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go("/c/one");
    await go("/pair");
    await scan(fake);
    await act(async () => {
      fake.pairs[0]?.succeed(pairedMachine(SESSION_DAEMON, "desktop"));
    });
    await flush();
    await fireEvent.press(screen.getByRole("button", { name: "Close" }));
    expect(screen).toHavePathname("/");
    expect(fake.selected).toEqual([]);
  });

  it("opens Tailscale from the tailnet error", async () => {
    const fake = fakePairingSession();
    const { routes } = app(fake);
    await renderRouter(routes, { initialUrl: "/" });
    await go("/pair");
    await scan(fake);
    await act(async () => {
      fake.pairs[0]?.fail({ kind: "unreachable" });
    });
    await flush();
    await fireEvent.press(screen.getByRole("button", { name: "Open Tailscale" }));
    expect(fake.opened).toEqual(["tailscale"]);
  });
});

describe("the no-machine root", () => {
  it("scans on the root, then shows the pairing in the modal", async () => {
    const fake = fakePairingSession();
    const { routes } = app(fake, "no-machine");
    await renderRouter(routes, { initialUrl: "/" });
    expect(screen.getByText("no machine paired")).toBeOnTheScreen();
    await scan(fake);
    expect(screen).toHavePathname("/pair");
    expect(screen.getByRole("header", { name: "Connecting to desktop" })).toBeOnTheScreen();
  });

  it("opens the modal with the camera error when the camera is off", async () => {
    const fake = fakePairingSession();
    fake.scanner.granted = false;
    const { routes } = app(fake, "no-machine");
    await renderRouter(routes, { initialUrl: "/" });
    await fireEvent.press(screen.getByRole("button", { name: "Scan code" }));
    await flush();
    expect(screen).toHavePathname("/pair");
    expect(screen.getByRole("header", { name: "efr cannot use the camera" })).toBeOnTheScreen();
  });

  it("stays on the root when the scan is cancelled", async () => {
    const fake = fakePairingSession();
    const { routes } = app(fake, "no-machine");
    await renderRouter(routes, { initialUrl: "/" });
    await fireEvent.press(screen.getByRole("button", { name: "Scan code" }));
    await flush();
    await act(async () => {
      fake.session.cancel();
    });
    await flush();
    expect(screen).toHavePathname("/");
  });
});
