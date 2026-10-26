// The Settings screen on the real runtime of the machines, with fake sockets, a fake clock
// and fake key stores, inside the test router of expo-router. No test reaches a network.

import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { Stack, router, useLocalSearchParams } from "expo-router";
import { act, fireEvent, renderRouter, screen } from "expo-router/testing-library";
import type { ReactNode } from "react";
import { ActionSheetIOS, Alert, Text } from "react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import {
  FakeClock,
  FakeSockets,
  completeHandshake,
  flush,
  remoteHello,
  type FakeSocket,
} from "@/lib/net/testing";
import { createQueryClient } from "@/lib/query/query-client";
import type { KeyValueStorage } from "@/lib/store/key-value";
import { createRuntime, type Runtime } from "@/features/machines/model/runtime";
import {
  DESKTOP,
  FakeStreams,
  LAPTOP,
  machineRecord,
  pairedStores,
} from "@/features/machines/model/testing";
import type { PairingDeps } from "@/features/pairing/model/pair";
import type { PairedDevice } from "@/features/pairing/model/paired-device";
import { createPreferences, type PreferencesStore } from "../model/preferences";
import { SettingsScreen } from "./settings-screen";
import { SettingsServicesContext, type SettingsServices } from "./settings-services";

jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));
jest.mock("expo-constants", () => ({
  __esModule: true,
  default: { expoConfig: { version: "0.1.0", ios: { buildNumber: "14" } } },
}));

const DESKTOP_URL = "ws://100.64.0.1:7419/";
const CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000000001";

/** A key-value store in memory, for the preferences. */
class MemoryStorage implements KeyValueStorage {
  readonly items = new Map<string, string>();

  getItemSync(key: string): string | null {
    return this.items.get(key) ?? null;
  }

  getItem(key: string): Promise<string | null> {
    return Promise.resolve(this.items.get(key) ?? null);
  }

  setItem(key: string, value: string): Promise<void> {
    this.items.set(key, value);
    return Promise.resolve();
  }

  removeItem(key: string): Promise<void> {
    this.items.delete(key);
    return Promise.resolve();
  }
}

interface Setup {
  readonly runtime: Runtime;
  readonly preferences: PreferencesStore;
  readonly sockets: FakeSockets;
  readonly client: QueryClient;
  readonly devices: { list(): Promise<readonly PairedDevice[]> };
}

let opened: Runtime[] = [];

async function setup(records: readonly PairedDevice[]): Promise<Setup> {
  const stores = await pairedStores(records);
  const sockets = new FakeSockets();
  const streams = new FakeStreams();
  const pairing: PairingDeps = {
    sockets: sockets.factory,
    clock: new FakeClock(Date.now()),
    keys: stores.keys,
    devices: stores.devices,
    sha256: () => Promise.resolve(Uint8Array.from({ length: 32 }, (_, i) => i)),
    newKeyAlias: () => "device-new",
    now: () => new Date(),
    client: "efr-app test",
  };
  const runtime = createRuntime({
    pairing,
    random: () => 0,
    streams: streams.factory,
    schedule: (callback) => {
      callback();
      return () => {};
    },
  });
  opened.push(runtime);
  await runtime.load();
  const preferences = createPreferences({ storage: new MemoryStorage() });
  return {
    runtime,
    preferences,
    sockets,
    client: createQueryClient(),
    devices: { list: () => stores.devices.loadAll() },
  };
}

function lastOf(sockets: FakeSockets, url: string): FakeSocket {
  const all = sockets.made.filter((one) => one.url === url);
  const socket = all.at(-1);
  if (socket === undefined) {
    throw new Error(`no socket to ${url}`);
  }
  return socket;
}

async function connect(parts: Setup, url: string, daemonId: string): Promise<void> {
  await act(async () => {
    await completeHandshake(
      lastOf(parts.sockets, url),
      remoteHello({ daemon_id: daemonId, version: "0.0.5" }),
    );
  });
}

function Pair() {
  const params = useLocalSearchParams<{ machine?: string }>();
  return <Text>{`pair ${params.machine ?? "new"}`}</Text>;
}

/** The routes of the app, with Settings and its services. */
function routes(services: SettingsServices | null, client: QueryClient) {
  function Layout(): ReactNode {
    return (
      <QueryClientProvider client={client}>
        <SettingsServicesContext value={services}>
          <Stack />
        </SettingsServicesContext>
      </QueryClientProvider>
    );
  }
  return {
    _layout: Layout,
    index: () => <Text>root</Text>,
    "c/[machine]/[id]": () => <Text>conversation</Text>,
    settings: SettingsScreen,
    pair: Pair,
  };
}

/** Opens Settings over the root and, when `conversation` is given, over that conversation. */
async function openSettings(parts: Setup, conversation?: string): Promise<void> {
  await renderRouter(routes(parts, parts.client), { initialUrl: "/" });
  if (conversation !== undefined) {
    await act(async () => {
      router.push(conversation);
    });
  }
  await act(async () => {
    router.push("/settings");
  });
}

/** Makes the next action sheet answer with the option at `index`. */
function answerSheet(index: number) {
  return jest
    .spyOn(ActionSheetIOS, "showActionSheetWithOptions")
    .mockImplementation((_options, callback) => callback(index));
}

async function pickInMenu(name: string, id: string): Promise<void> {
  await fireEvent(screen.getByTestId(`machine-menu-${name}`), "pressAction", {
    nativeEvent: { event: id },
  });
  await act(async () => {
    await flush();
  });
}

beforeEach(() => {
  jest.mocked(copyText).mockClear();
  jest.mocked(haptic).mockClear();
});

afterEach(() => {
  jest.restoreAllMocks();
  for (const runtime of opened) {
    runtime.dispose();
  }
  opened = [];
});

describe("the machine rows", () => {
  it("show each state, the node name and the efrd version", async () => {
    const parts = await setup([machineRecord(DESKTOP), machineRecord(LAPTOP)]);
    await openSettings(parts);
    expect(screen).toHavePathname("/settings");
    expect(screen.getByRole("header", { name: "Machines" })).toBeOnTheScreen();
    expect(screen.getByText("desktop")).toBeOnTheScreen();
    expect(screen.getByText("laptop")).toBeOnTheScreen();
    expect(screen.getAllByText("… connecting")).toHaveLength(2);

    await connect(parts, DESKTOP_URL, DESKTOP);
    expect(screen.getByText("✓ connected").props.className).toContain("text-state-success");
    expect(screen.getByText(/· desktop\.tail3f2a\.ts\.net$/)).toBeOnTheScreen();
    expect(screen.getByText(/· laptop\.tail3f2a\.ts\.net$/)).toBeOnTheScreen();
    // The status query runs again on its own; here the test asks for it.
    await act(async () => {
      await parts.client.refetchQueries();
    });
    // Query tells its observers on a timer of its own.
    expect(await screen.findByText("efrd 0.0.5 · protocol 1")).toBeOnTheScreen();
  });

  it("show an offline machine dim, with when it was seen, and keep its version", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts);
    await connect(parts, DESKTOP_URL, DESKTOP);
    await act(async () => {
      await parts.client.refetchQueries();
    });
    expect(await screen.findByText("efrd 0.0.5 · protocol 1")).toBeOnTheScreen();
    await act(async () => {
      lastOf(parts.sockets, DESKTOP_URL).drop();
      await flush();
    });
    expect(screen.getByText(/^offline · seen \d\d:\d\d$/)).toBeOnTheScreen();
    expect(screen.queryByText(/desktop\.tail3f2a\.ts\.net/)).toBeNull();
    expect(screen.getByText("desktop").props.className).toContain("text-text-muted");
    expect(screen.getByText("efrd 0.0.5 · protocol 1")).toBeOnTheScreen();
  });

  it("have an actions button with the machine name", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts);
    expect(screen.getByRole("button", { name: "desktop actions" })).toBeOnTheScreen();
    const menu = screen.getByTestId("machine-menu-desktop");
    expect(menu.props.title).toBe("desktop");
    expect(menu.props.actions.map((action: { id: string }) => action.id)).toEqual([
      "copy",
      "repair",
      "forget",
    ]);
  });
});

describe("the row menu", () => {
  it("copies the node name with the light haptic", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts);
    await pickInMenu("desktop", "copy");
    expect(copyText).toHaveBeenCalledWith("desktop.tail3f2a.ts.net");
    expect(haptic).toHaveBeenCalledWith("light");
  });

  it("opens Pairing for this machine on Re-pair", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts);
    await pickInMenu("desktop", "repair");
    expect(screen).toHavePathname("/pair");
    expect(screen.getByText(`pair ${DESKTOP}`)).toBeOnTheScreen();
  });
});

describe("Forget", () => {
  it("asks first, and Cancel changes nothing", async () => {
    const parts = await setup([machineRecord(DESKTOP), machineRecord(LAPTOP)]);
    await openSettings(parts);
    const sheet = answerSheet(1);
    await pickInMenu("desktop", "forget");
    expect(sheet.mock.calls[0]?.[0]).toMatchObject({
      title: "Forget desktop?",
      options: ["Forget desktop", "Cancel"],
      destructiveButtonIndex: [0],
    });
    expect(screen.getByText("desktop")).toBeOnTheScreen();
    expect(await parts.devices.list()).toHaveLength(2);
  });

  it("removes the row and the record, and keeps Settings open", async () => {
    const parts = await setup([machineRecord(DESKTOP), machineRecord(LAPTOP)]);
    await openSettings(parts);
    answerSheet(0);
    await pickInMenu("desktop", "forget");
    expect(screen.queryByText("desktop")).toBeNull();
    expect(screen.getByText("laptop")).toBeOnTheScreen();
    expect(screen).toHavePathname("/settings");
    expect((await parts.devices.list()).map((device) => device.daemonId)).toEqual([LAPTOP]);
  });

  it("goes back to the blank root when a conversation of the machine is in the stack", async () => {
    const parts = await setup([machineRecord(DESKTOP), machineRecord(LAPTOP)]);
    await openSettings(parts, `/c/${DESKTOP}/${CONVERSATION}`);
    answerSheet(0);
    await pickInMenu("desktop", "forget");
    expect(screen).toHavePathname("/settings");
    await act(async () => {
      router.back();
    });
    expect(screen).toHavePathname("/");
  });

  it("names the last machine and resets the stack to the root", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts, `/c/${DESKTOP}/${CONVERSATION}`);
    const sheet = answerSheet(0);
    await pickInMenu("desktop", "forget");
    expect(sheet.mock.calls[0]?.[0]).toMatchObject({
      message:
        "desktop is the last machine on this iPhone. Its pairing key is deleted and the app goes back to pairing. Nothing changes on desktop.",
    });
    expect(screen).toHavePathname("/");
    expect(screen.getByText("root")).toBeOnTheScreen();
    expect(router.canGoBack()).toBe(false);
    expect(await parts.devices.list()).toEqual([]);
  });

  it("says so when the key stays on the phone", async () => {
    const parts = await setup([machineRecord(DESKTOP), machineRecord(LAPTOP)]);
    jest.spyOn(parts.runtime, "forget").mockRejectedValue(new Error("the keychain failed"));
    const alert = jest.spyOn(Alert, "alert").mockImplementation(() => undefined);
    await openSettings(parts);
    answerSheet(0);
    await pickInMenu("desktop", "forget");
    expect(alert).toHaveBeenCalledWith(
      "Could not forget desktop",
      "The pairing key of desktop is still on this iPhone. Forget desktop again after the next start of the app.",
    );
  });
});

describe("the other groups", () => {
  it("open Pairing from Pair a new machine", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts);
    await fireEvent.press(screen.getByRole("button", { name: "Pair a new machine" }));
    expect(screen).toHavePathname("/pair");
    expect(screen.getByText("pair new")).toBeOnTheScreen();
  });

  it("hold only Pair a new machine when no machine is paired", async () => {
    const parts = await setup([]);
    await openSettings(parts);
    expect(screen.getByRole("button", { name: "Pair a new machine" })).toBeOnTheScreen();
    expect(screen.queryByRole("button", { name: /actions$/ })).toBeNull();
    expect(screen.getByText("efr device add")).toBeOnTheScreen();
  });

  it("switch the haptics preference", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts);
    const toggle = screen.getByLabelText("Haptics");
    expect(toggle.props.value).toBe(true);
    await fireEvent(toggle, "valueChange", false);
    expect(parts.preferences.store.get().haptics).toBe(false);
    expect(screen.getByLabelText("Haptics").props.value).toBe(false);
    expect(screen.getByText("Haptics for taps, copies, scans and errors.")).toBeOnTheScreen();
  });

  it("show the version of the app", async () => {
    const parts = await setup([]);
    await openSettings(parts);
    expect(screen.getByRole("header", { name: "About" })).toBeOnTheScreen();
    expect(screen.getByText("Version")).toBeOnTheScreen();
    expect(screen.getByText("0.1.0 (14)").props.className).toContain("text-mono-shell");
  });

  it("show the loading row until the runtime read the records", async () => {
    const parts = await setup([]);
    const unloaded = createRuntime({
      pairing: null,
      random: () => 0,
      streams: new FakeStreams().factory,
      schedule: () => () => {},
    });
    opened.push(unloaded);
    await renderRouter(
      routes({ runtime: unloaded, preferences: parts.preferences }, parts.client),
      {
        initialUrl: "/settings",
      },
    );
    expect(screen.getByText("loading the machines")).toBeOnTheScreen();
  });
});

describe("without the services of the layout", () => {
  it("shows Settings with no machine", async () => {
    await renderRouter(routes(null, createQueryClient()), { initialUrl: "/settings" });
    expect(screen.getByRole("button", { name: "Pair a new machine" })).toBeOnTheScreen();
    expect(screen.getByLabelText("Haptics").props.value).toBe(true);
  });
});
