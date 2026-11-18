// The Settings screen on the real services of the app, with fake sockets, a fake clock and
// fake key stores, inside the test router of expo-router. No test reaches a network.

import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { Stack, router, useLocalSearchParams } from "expo-router";
import { act, fireEvent, renderRouter, screen, waitFor } from "expo-router/testing-library";
import type { ReactNode } from "react";
import { ActionSheetIOS, Text } from "react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import {
  FakeSockets,
  completeHandshake,
  flush,
  remoteHello,
  type FakeSocket,
} from "@/lib/net/testing";
import { createQueryClient } from "@/lib/query/query-client";
import { createConversationStream } from "@/features/conversation/model/conversation-stream";
import { createRuntime } from "@/features/machines/model/runtime";
import { DESKTOP, LAPTOP, machineRecord } from "@/features/machines/model/testing";
import type { AppServices } from "@/features/navigation/model/app-services";
import {
  TEST_NOW,
  testServices,
  type TestServices,
  type TestServicesOptions,
} from "@/features/navigation/model/testing";
import { draftKey } from "@/features/conversation/model/drafts";
import { AppServicesContext } from "@/features/navigation/use-app-services";
import type { PairedDevice } from "@/features/pairing/model/paired-device";
import { conversationKeys } from "@/features/conversation/conversation-queries";
import { machineKeys } from "@/features/machines/machine-queries";
import {
  accountParamsSchema,
  settingsParamsSchema,
} from "@/features/navigation/model/route-schemas";
import { FakePushNative, expoToken } from "../model/testing";
import { AccountScreen } from "./account-screen";
import { SettingsScreen, UNLOCK_MARK_MS } from "./settings-screen";

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

interface Setup {
  readonly services: AppServices;
  readonly sockets: FakeSockets;
  readonly client: QueryClient;
  readonly devices: { list(): Promise<readonly PairedDevice[]> };
}

let opened: { dispose(): void }[] = [];

async function setup(records: readonly PairedDevice[]): Promise<Setup> {
  const parts = await testServices(records);
  opened.push(parts.services);
  return {
    services: parts.services,
    sockets: parts.sockets,
    client: parts.services.queryClient,
    devices: { list: () => parts.devices.loadAll() },
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
function routes(services: AppServices | null, client: QueryClient) {
  function Layout(): ReactNode {
    return (
      <QueryClientProvider client={client}>
        <AppServicesContext value={services}>
          <Stack />
        </AppServicesContext>
      </QueryClientProvider>
    );
  }
  return {
    _layout: Layout,
    index: () => <Text>root</Text>,
    "c/[machine]/[id]": () => <Text>conversation</Text>,
    settings: SettingsRoute,
    account: AccountRoute,
    pair: Pair,
  };
}

/**
 * True for the status query of a machine. A test refetches only those: the model list of
 * the accounts would wait for an answer that the fake socket never sends.
 */
function isStatus(query: { readonly queryKey: readonly unknown[] }): boolean {
  return (
    JSON.stringify(query.queryKey) === JSON.stringify(machineKeys.status(DESKTOP)) ||
    JSON.stringify(query.queryKey) === JSON.stringify(machineKeys.status(LAPTOP))
  );
}

/** The settings route, as src/app/settings.tsx renders it. */
function SettingsRoute() {
  const params = settingsParamsSchema.safeParse(useLocalSearchParams());
  return <SettingsScreen focus={params.success ? params.data.focus : undefined} />;
}

/** The account route, as src/app/account.tsx renders it. */
function AccountRoute() {
  const params = accountParamsSchema.safeParse(useLocalSearchParams());
  return <AccountScreen target={params.success ? params.data : undefined} />;
}

/** Opens Settings over the root and, when `conversation` is given, over that conversation. */
async function openSettings(
  parts: Setup,
  conversation?: string,
  href = "/settings",
): Promise<void> {
  await renderRouter(routes(parts.services, parts.client), { initialUrl: "/" });
  if (conversation !== undefined) {
    await act(async () => {
      router.push(conversation);
    });
  }
  await act(async () => {
    router.push(href);
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
  for (const one of opened) {
    one.dispose();
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
      await parts.client.refetchQueries({ predicate: isStatus });
    });
    // Query tells its observers on a timer of its own.
    expect(await screen.findByText("efrd 0.0.5 · protocol 1")).toBeOnTheScreen();
  });

  it("show an offline machine dim, with when it was seen, and keep its version", async () => {
    // The row counts from the clock of the phone, and the machine was seen at the time
    // of the tests: on another day the row says "seen yesterday".
    jest.useFakeTimers({ now: TEST_NOW });
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts);
    await connect(parts, DESKTOP_URL, DESKTOP);
    await act(async () => {
      await parts.client.refetchQueries({ predicate: isStatus });
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
    // No title on top: the row already names the machine.
    expect(menu.props.title).toBe("");
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
    jest
      .spyOn(parts.services.runtime, "forget")
      .mockRejectedValue(new Error("the keychain failed"));
    await openSettings(parts);
    const sheet = answerSheet(0);
    await pickInMenu("desktop", "forget");
    // An action sheet and not an alert: an alert of React Native lies over the lock screen.
    expect(sheet).toHaveBeenLastCalledWith(
      {
        title: "Could not forget desktop",
        message:
          "The pairing key of desktop is still on this iPhone. Forget desktop again after the next start of the app.",
        options: ["OK"],
        cancelButtonIndex: 0,
        destructiveButtonIndex: [],
        userInterfaceStyle: "dark",
      },
      expect.any(Function),
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
    expect(parts.services.preferences.store.get().haptics).toBe(false);
    expect(screen.getByLabelText("Haptics").props.value).toBe(false);
    expect(screen.getByText("Haptics for taps, copies, scans and errors.")).toBeOnTheScreen();
  });

  it("switch Ask before a costly model switch, on by default", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    await openSettings(parts);
    expect(screen.getByRole("header", { name: "Models" })).toBeOnTheScreen();
    const toggle = screen.getByLabelText("Ask before a costly model switch");
    expect(toggle.props.value).toBe(true);
    await fireEvent(toggle, "valueChange", false);
    expect(parts.services.preferences.store.get().askCostlySwitch).toBe(false);
  });

  it("list the accounts of a machine with their fullest limit, and push the limits of one", async () => {
    const parts = await setup([machineRecord(DESKTOP)]);
    const now = Date.now();
    const iso = (offset: number): string => new Date(now + offset).toISOString();
    parts.client.setQueryData(conversationKeys.models(DESKTOP), {
      models: [],
      catalogs: [
        { provider: "anthropic-api.work", origin: "backend" },
        { provider: "anthropic-api.personal", origin: "cache", signed_out: true },
        {
          provider: "openai-subscription",
          origin: "backend",
          limits: {
            observed_at: iso(-60_000),
            windows: [{ minutes: 300, used_percent: 92, resets_at: iso(3_600_000) }],
          },
        },
        {
          provider: "openai-subscription.b",
          origin: "backend",
          limits: {
            observed_at: iso(-60_000),
            windows: [
              { minutes: 300, used_percent: 100, resets_at: iso(600_000) },
              { minutes: 10_080, used_percent: 63, resets_at: iso(86_400_000) },
            ],
          },
        },
      ],
    });
    await openSettings(parts);
    expect(screen.getByRole("header", { name: "Accounts on desktop" })).toBeOnTheScreen();
    expect(screen.getByText("signed out on desktop")).toBeOnTheScreen();
    expect(screen.getByText("92%")).toBeOnTheScreen();
    // AC-Limits 4: the value in mono 13.
    expect(screen.getByText("limit reached").props.className).toBe(
      "text-mono-shell text-state-error",
    );
    await fireEvent.press(screen.getByRole("button", { name: "ChatGPT plan · b, limit reached" }));
    expect(screen).toHavePathname("/account");
    expect(screen.getByText("On desktop")).toBeOnTheScreen();
    expect(screen.getByText("5-hour")).toBeOnTheScreen();
    expect(screen.getByText("weekly")).toBeOnTheScreen();
    // AC-Limits 5 and 6: only "limit reached" in state_error, the reset quiet after it.
    const reached = screen.getByText(/^limit reached · resets /u);
    expect(reached.props.className).toBe("text-mono-shell text-state-error");
    expect(screen.getByText(/^63% · resets /u).props.className).toBe(
      "text-mono-shell text-text-muted",
    );
    const resets = screen.getAllByText(/^· resets /u);
    expect(resets).toHaveLength(2);
    expect(resets[0]?.props.className).toBe("text-mono-shell text-text-subtle");
    expect(screen.getByLabelText(/^5-hour, limit reached, resets /u)).toBeOnTheScreen();
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
      streams: createConversationStream,
      schedule: () => () => {},
    });
    opened.push(unloaded);
    await renderRouter(routes({ ...parts.services, runtime: unloaded }, parts.client), {
      initialUrl: "/settings",
    });
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

/** Settings with the Face ID unlock of a fake iPhone. */
async function openWithLock(
  options: TestServicesOptions = {},
  href = "/settings",
): Promise<TestServices> {
  const parts = await testServices([machineRecord(DESKTOP)], options);
  // NOTE: clear the queries at the end, or the gc timer of 5 minutes keeps jest waiting.
  opened.push({
    dispose: () => {
      parts.services.queryClient.clear();
      parts.services.dispose();
    },
  });
  await openSettings(
    {
      services: parts.services,
      sockets: parts.sockets,
      client: parts.services.queryClient,
      devices: { list: () => parts.devices.loadAll() },
    },
    undefined,
    href,
  );
  await act(async () => {
    await flush();
  });
  return parts;
}

/** The row "Face ID unlock". */
function row() {
  return screen.getByTestId("unlock-row");
}

describe("the Face ID unlock row (Settings round 9)", () => {
  it("is off by default in the group Privacy, with its line", async () => {
    await openWithLock();
    expect(screen.getByRole("header", { name: "Privacy" })).toBeOnTheScreen();
    expect(row()).toHaveAccessibleName("Face ID unlock");
    expect(row().props.accessibilityState).toMatchObject({ checked: false });
    expect(screen.getByTestId("unlock-line")).toHaveTextContent(
      "Opens efr with Face ID. Auto from this phone needs it.",
    );
  });

  it("asks iOS before it turns on, and moves only after the pass", async () => {
    const parts = await openWithLock();
    await fireEvent.press(row());
    await act(async () => {
      await flush();
    });
    expect(parts.checkReasons).toEqual(["Turn on Face ID unlock"]);
    expect(row().props.accessibilityState).toMatchObject({ checked: false, busy: true });
    await act(async () => {
      await parts.answerCheck({ kind: "passed" });
    });
    expect(row().props.accessibilityState).toMatchObject({ checked: true, busy: false });
    expect(parts.services.preferences.store.get().faceIdUnlock).toBe(true);
  });

  it("stays off after a cancel", async () => {
    const parts = await openWithLock();
    await fireEvent.press(row());
    await act(async () => {
      await flush();
      await parts.answerCheck({ kind: "cancelled", method: "Face ID" });
    });
    expect(row().props.accessibilityState).toMatchObject({ checked: false });
  });

  it("asks before it turns off, and then sets each conversation back to cautious", async () => {
    const parts = await openWithLock({ lockOn: true });
    await act(async () => {
      await parts.answerCheck({ kind: "passed" });
    });
    const key = draftKey(DESKTOP, CONVERSATION);
    parts.services.modelChoices.setMode(key, "auto");
    await fireEvent.press(row());
    await act(async () => {
      await flush();
    });
    expect(parts.checkReasons.at(-1)).toBe("Turn off Face ID unlock");
    await act(async () => {
      await parts.answerCheck({ kind: "passed" });
    });
    expect(row().props.accessibilityState).toMatchObject({ checked: false });
    expect(parts.services.modelChoices.mode(key)).toBe("cautious");
  });

  it("scrolls to the row and marks it for about 1 s with focus=unlock (S-ComposerR7 r9 12b)", async () => {
    await openWithLock({}, "/settings?focus=unlock");
    expect(row().props.className).toContain(" bg-selection-bg");
    jest.useFakeTimers();
    try {
      await fireEvent(screen.getByTestId("settings-privacy"), "layout", {
        nativeEvent: { layout: { x: 0, y: 900, width: 390, height: 80 } },
      });
      await fireEvent(row(), "layout", {
        nativeEvent: { layout: { x: 0, y: 0, width: 390, height: 66 } },
      });
      await act(async () => {
        jest.advanceTimersByTime(UNLOCK_MARK_MS);
      });
    } finally {
      jest.useRealTimers();
    }
    expect(row().props.className).not.toContain(" bg-selection-bg");
  });

  it("does not mark the row without the focus", async () => {
    await openWithLock();
    expect(row().props.className).not.toContain(" bg-selection-bg");
  });

  it("greys the row on an iPhone with no passcode", async () => {
    await openWithLock({ facts: { passcode: false, method: "the passcode" } });
    await waitFor(() => {
      expect(row().props.accessibilityState).toMatchObject({ disabled: true });
    });
    expect(screen.getByTestId("unlock-line")).toHaveTextContent("Needs a passcode on this iPhone.");
  });

  it("names the row after what the iPhone checks first", async () => {
    await openWithLock({ facts: { passcode: true, method: "Touch ID" } });
    await waitFor(() => {
      expect(row()).toHaveAccessibleName("Touch ID unlock");
    });
  });

  it("says why efr turned it off while the passcode is gone, then goes back to frame 1", async () => {
    const parts = await openWithLock({ lockOn: true });
    // The facts of a Face ID iPhone whose passcode went (lib/native/owner-check.ts).
    parts.setOwnerFacts({ passcode: false, method: "Face ID" });
    await act(async () => {
      await parts.answerCheck({ kind: "no_passcode" });
    });
    await act(async () => {
      await parts.services.queryClient.invalidateQueries({ queryKey: ["device", "owner"] });
    });
    await waitFor(() => {
      expect(row().props.accessibilityState).toMatchObject({ checked: false, disabled: true });
    });
    expect(row()).toHaveAccessibleName("Face ID unlock");
    expect(screen.getByTestId("unlock-line")).toHaveTextContent(
      "Face ID unlock is off: this iPhone has no passcode.",
    );

    parts.setOwnerFacts({ passcode: true, method: "Face ID" });
    await act(async () => {
      await parts.services.queryClient.invalidateQueries({ queryKey: ["device", "owner"] });
    });
    await waitFor(() => {
      expect(row().props.accessibilityState).toMatchObject({ checked: false, disabled: false });
    });
    expect(screen.getByTestId("unlock-line")).toHaveTextContent(
      "Opens efr with Face ID. Auto from this phone needs it.",
    );
  });
});

/** Settings with push on a fake iPhone, with desktop and laptop paired. */
async function openWithPush(native: FakePushNative): Promise<TestServices> {
  const parts = await testServices([machineRecord(DESKTOP), machineRecord(LAPTOP)], {
    push: native,
  });
  opened.push({
    dispose: () => {
      parts.services.queryClient.clear();
      parts.services.dispose();
    },
  });
  await openSettings({
    services: parts.services,
    sockets: parts.sockets,
    client: parts.services.queryClient,
    devices: { list: () => parts.devices.loadAll() },
  });
  await act(async () => {
    await flush();
  });
  return parts;
}

/** The row "Notifications". */
function notifications() {
  return screen.getByTestId("notifications-row");
}

describe("the row Notifications (Settings 0.0.13)", () => {
  it("is not there in a build without expo-notifications", async () => {
    await openWithLock();
    expect(screen.queryByRole("header", { name: "When you are away" })).toBeNull();
    expect(screen.queryByTestId("notifications-row")).toBeNull();
  });

  it("is off by default, lets iOS ask on a tap and moves after the answer", async () => {
    const native = new FakePushNative();
    const parts = await openWithPush(native);
    expect(screen.getByRole("header", { name: "When you are away" })).toBeOnTheScreen();
    expect(notifications()).toHaveAccessibleName("Notifications");
    expect(notifications().props.accessibilityState).toMatchObject({ checked: false });
    expect(screen.getByTestId("notifications-line")).toHaveTextContent(
      "A notice when something needs you and you are not at the terminal.",
    );
    await act(async () => {
      await parts.connect(DESKTOP, { capabilities: { device_prove: true, push: true } });
    });
    await fireEvent.press(notifications());
    await act(async () => {
      await flush();
    });
    expect(native.questions).toBe(1);
    expect(notifications().props.accessibilityState).toMatchObject({ checked: true });
    expect(lastOf(parts.sockets, DESKTOP_URL).requests("device.push_set").at(-1)).toMatchObject({
      params: { token: expoToken(1) },
    });
  });

  it("names a machine whose efrd takes no push token", async () => {
    const parts = await openWithPush(new FakePushNative());
    await act(async () => {
      await parts.connect(LAPTOP);
    });
    expect(screen.getByTestId("notifications-note")).toHaveTextContent("Not available on laptop");
    expect(screen.getByTestId("notifications-note").props.className).toContain("text-text-muted");
  });

  it("says so when iOS denies the notices, and a tap opens the iOS settings", async () => {
    const native = new FakePushNative();
    native.permissionNow = "denied";
    await openWithPush(native);
    expect(notifications()).toHaveAccessibleName("Notifications, off in iOS Settings");
    expect(notifications().props.accessibilityRole).toBe("button");
    const line = screen.getByTestId("notifications-line");
    expect(line).toHaveTextContent(
      "Notifications are off for efr in iOS Settings. Tap to open them.",
    );
    expect(line.props.className).toContain("text-state-warning");
    await fireEvent.press(notifications());
    expect(native.settingsOpened).toBe(1);
    expect(native.questions).toBe(0);
  });

  it("sends no token to a machine before Forget", async () => {
    const parts = await openWithPush(new FakePushNative());
    await act(async () => {
      await parts.connect(DESKTOP, { capabilities: { device_prove: true, push: true } });
    });
    const socket = lastOf(parts.sockets, DESKTOP_URL);
    const before = socket.requests("device.push_set").length;
    answerSheet(0);
    await pickInMenu("desktop", "forget");
    expect(socket.requests("device.push_set").slice(before)).toEqual([
      expect.objectContaining({ params: {} }),
    ]);
  });
});
