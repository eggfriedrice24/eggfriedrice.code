import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { createAtom } from "@tanstack/store";
import { Stack, router, useLocalSearchParams } from "expo-router";
import { act, renderRouter, screen } from "expo-router/testing-library";
import { fireEvent } from "@testing-library/react-native";
import { Text } from "react-native";

import { machineOf } from "@/features/machines/model/machine";
import { loadedRegistry, type RegistryState } from "@/features/machines/model/machine-registry";
import { DESKTOP as MACHINE, machineRecord } from "@/features/machines/model/testing";
import {
  conversationId,
  testServices,
  type TestServices,
} from "@/features/navigation/model/testing";
import { AppServicesContext } from "@/features/navigation/use-app-services";
import type { PairedDevice } from "@/features/pairing/model/paired-device";
import type {
  ConversationsSearch,
  ConversationsSearchResult,
  SearchRow,
} from "@/lib/protocol/generated";
import type { ConversationAddress } from "@/lib/query/keys";
import { createQueryClient } from "@/lib/query/query-client";
import { FakeCaller } from "@/lib/query/testing";

import { conversationsKeys } from "../conversations-queries";
import { october } from "../model/testing";
import { sheetParams } from "../model/sheet-schemas";
import { ConversationsScreen } from "./conversations-screen";
import { SheetDepsContext, type SheetDeps } from "./use-sheet-deps";

jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const NOW = october(26, 14);
const DESKTOP = "019a9b1c-3d00-7a10-8b20-00000000000d";
const LAPTOP = "019a9b1c-3d00-7a10-8b20-00000000000e";

function device(daemonId: string, fqdn: string): PairedDevice {
  return {
    daemonId,
    deviceId: `phone-${daemonId.slice(-1)}`,
    host: "100.64.0.1",
    port: 7681,
    fqdn,
    name: "iPhone 17 Pro",
    keyAlias: `key-${daemonId.slice(-1)}`,
    scopes: ["read", "prompt"],
    pairedAt: "2026-10-20T10:00:00Z",
  };
}

const REGISTRY: RegistryState = loadedRegistry([
  machineOf(device(DESKTOP, "desktop.tail3f2a.ts.net"), false),
  machineOf(device(LAPTOP, "laptop.tail3f2a.ts.net"), false),
]);

/** The conversation of the pick on the services of the app. */
const PICKED = conversationId(1);

/** One search row, updated one minute before NOW. */
function row(id: string, title: string): SearchRow {
  return {
    conversation: {
      summary: {
        id,
        title,
        status: "idle",
        created_at: new Date(NOW - 120_000).toISOString(),
        updated_at: new Date(NOW - 60_000).toISOString(),
        last_seq: 3,
      },
      route: { kind: "outside" },
      turns: 1,
    },
    short_id: id.slice(0, 8),
  };
}

function result(machine: string): ConversationsSearchResult {
  const id = machine === DESKTOP ? "fan" : "restic";
  const title = machine === DESKTOP ? "Why is the fan loud" : "Set up restic backups";
  return {
    rows: [
      {
        conversation: {
          summary: {
            id,
            title,
            status: "idle",
            created_at: new Date(NOW - 120_000).toISOString(),
            updated_at: new Date(NOW - 60_000).toISOString(),
            last_seq: 3,
          },
          route: { kind: "outside" },
          turns: 1,
        },
        short_id: id,
      },
    ],
  };
}

let client: QueryClient;
let caller: FakeCaller;
let opened: ConversationAddress[];
let deps: SheetDeps;

function Layout() {
  return (
    <QueryClientProvider client={client}>
      <SheetDepsContext value={deps}>
        <Stack />
      </SheetDepsContext>
    </QueryClientProvider>
  );
}

// The same as src/app/conversations.tsx (a test cannot import a route).
function ConversationsRoute() {
  const params = sheetParams(useLocalSearchParams());
  return <ConversationsScreen {...params} />;
}

const ROUTES = {
  _layout: Layout,
  index: () => <Text>root</Text>,
  conversations: ConversationsRoute,
};

async function flush(): Promise<void> {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(0);
  });
  await act(async () => {
    await jest.advanceTimersByTimeAsync(0);
  });
}

beforeEach(() => {
  jest.useFakeTimers();
  client = createQueryClient();
  caller = new FakeCaller({
    "conversations.search": (_params: ConversationsSearch, machine: string) => result(machine),
  });
  opened = [];
  deps = {
    caller,
    registry: createAtom(REGISTRY),
    open: (ref) => opened.push(ref),
    now: () => NOW,
  };
});

afterEach(() => {
  client.clear();
  jest.useRealTimers();
  jest.restoreAllMocks();
});

describe("the conversations sheet route", () => {
  it("lists the conversations of every paired machine, by machine name", async () => {
    await renderRouter(ROUTES, { initialUrl: "/" });
    await act(async () => {
      router.push("/conversations");
    });
    await flush();
    expect(screen).toHavePathname("/conversations");
    expect(screen.getByLabelText("Why is the fan loud, desktop · 1m")).toBeOnTheScreen();
    expect(screen.getByLabelText("Set up restic backups, laptop · 1m")).toBeOnTheScreen();
    expect(caller.callsOf("conversations.search").map((call) => call.machine)).toEqual([
      DESKTOP,
      LAPTOP,
    ]);
  });

  it("asks the machine of the route param first", async () => {
    await renderRouter(ROUTES, { initialUrl: "/" });
    await act(async () => {
      router.push(`/conversations?machine=${LAPTOP}`);
    });
    await flush();
    expect(caller.callsOf("conversations.search").map((call) => call.machine)).toEqual([
      LAPTOP,
      DESKTOP,
    ]);
  });

  it("opens the picked conversation with its machine", async () => {
    await renderRouter(ROUTES, { initialUrl: "/" });
    await act(async () => {
      router.push("/conversations");
    });
    await flush();
    await fireEvent.press(screen.getByLabelText("Set up restic backups, laptop · 1m"));
    await flush();
    expect(opened).toEqual([{ machine: LAPTOP, id: "restic" }]);
  });

  it("needs the services of the root layout", async () => {
    jest.spyOn(console, "error").mockImplementation(() => undefined);
    await expect(
      renderRouter(
        {
          _layout: () => (
            <QueryClientProvider client={client}>
              <Stack />
            </QueryClientProvider>
          ),
          index: ConversationsRoute,
        },
        { initialUrl: "/" },
      ),
    ).rejects.toThrow("The conversations sheet has no runtime");
  });
});

describe("the conversations sheet on the services of the app", () => {
  let parts: TestServices;

  afterEach(() => {
    parts.services.dispose();
  });

  function wiredRoutes() {
    return {
      _layout: () => (
        <QueryClientProvider client={parts.services.queryClient}>
          <AppServicesContext value={parts.services}>
            <Stack />
          </AppServicesContext>
        </QueryClientProvider>
      ),
      index: () => <Text>root</Text>,
      conversations: ConversationsRoute,
      "c/[machine]/[id]": () => <Text>conversation</Text>,
    };
  }

  it("dismisses itself, then pushes the picked conversation over the root", async () => {
    parts = await testServices([machineRecord(MACHINE)]);
    parts.services.queryClient.setQueryData(conversationsKeys.search(MACHINE, ""), {
      rows: [row(PICKED, "Why is the fan loud")],
    });
    await renderRouter(wiredRoutes(), { initialUrl: "/" });
    await act(async () => {
      router.push("/conversations");
    });
    await flush();
    await fireEvent.press(screen.getByLabelText(/^Why is the fan loud, desktop/));
    await flush();
    expect(screen).toHavePathname(`/c/${MACHINE}/${PICKED}`);
    await act(async () => {
      router.back();
    });
    expect(screen).toHavePathname("/");
  });
});
