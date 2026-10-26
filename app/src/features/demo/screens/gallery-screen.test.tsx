// The dev gallery in the test router of expo-router, on sandbox services of the demo
// server: it lists every board, pairs the demo machines through the real pairing flow,
// and opens the real routes. The demo server runs on the real clock with a short wait at
// the terminal; nothing reaches a network.

import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider } from "@tanstack/react-query";
import { Stack, useLocalSearchParams } from "expo-router";
import { fireEvent, renderRouter, screen, waitFor } from "expo-router/testing-library";
import { Text } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { systemClock } from "@/lib/native/clock";
import { AppServicesContext } from "@/features/navigation/use-app-services";

import { GALLERY_BOARDS } from "../model/boards";
import { DemoServer } from "../model/demo-server";
import { sandboxServices, type Sandbox } from "../model/demo-services";
import { GalleryScreen } from "./gallery-screen";

/** The demo server of the test, with no wait at the terminal. */
const mockServer = { current: undefined as DemoServer | undefined };

jest.mock("../native/demo-machines", () => ({
  get demoServer() {
    return mockServer.current;
  },
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

let sandbox: Sandbox | undefined;

afterEach(() => {
  sandbox?.services.dispose();
  sandbox = undefined;
  mockServer.current?.dispose();
});

function Params() {
  const params = useLocalSearchParams();
  return <Text testID="params">{JSON.stringify(params)}</Text>;
}

async function showGallery(): Promise<Sandbox> {
  const server = new DemoServer({ clock: systemClock, confirmMs: 0 });
  mockServer.current = server;
  const box = await sandboxServices({
    server,
    machines: [],
    schedule: (callback) => {
      callback();
      return () => undefined;
    },
  });
  sandbox = box;
  await renderRouter(
    {
      _layout: () => (
        <SafeAreaProvider initialMetrics={METRICS}>
          <QueryClientProvider client={box.services.queryClient}>
            <AppServicesContext value={box.services}>
              <Stack />
            </AppServicesContext>
          </QueryClientProvider>
        </SafeAreaProvider>
      ),
      index: () => <Text>root</Text>,
      "(dev)/gallery": GalleryScreen,
      "(dev)/board": Params,
      "(dev)/bench": () => <Text>bench</Text>,
      "c/[machine]/[id]": Params,
      settings: () => <Text>settings</Text>,
      conversations: () => <Text>sheet</Text>,
    },
    { initialUrl: "/gallery" },
  );
  return box;
}

describe("the dev gallery", () => {
  it("lists every board by its canvas name", async () => {
    await showGallery();
    for (const board of GALLERY_BOARDS) {
      expect(screen.getByLabelText(`${board.name}: ${board.title}`)).toBeOnTheScreen();
    }
    expect(screen.getByText("Decode bench (zod, jitless, hand)")).toBeOnTheScreen();
  });

  it("opens a component board on the board route with a new visit", async () => {
    await showGallery();
    await fireEvent.press(screen.getByText("S-Approvals"));
    await waitFor(() => expect(screen).toHavePathname("/board"));
    expect(screen.getByTestId("params")).toHaveTextContent(/"name":"S-Approvals","visit":"\d+"/);
  });

  it("pairs the demo machines through the pairing flow, then forgets them", async () => {
    const box = await showGallery();
    expect(screen.getByText("0 of 3")).toBeOnTheScreen();
    await fireEvent.press(screen.getByText("Pair the demo machines"));
    await waitFor(() => expect(screen.getByText("3 of 3")).toBeOnTheScreen());
    expect((await box.devices.loadAll()).map((record) => record.fqdn)).toEqual([
      "desktop.demo.invalid",
      "laptop.demo.invalid",
      "homelab.demo.invalid",
    ]);
    await fireEvent.press(screen.getByText("Forget the demo machines"));
    await waitFor(() => expect(screen.getByText("0 of 3")).toBeOnTheScreen());
  });

  it("sets the stage of a screen board and opens its conversation", async () => {
    await showGallery();
    await fireEvent.press(screen.getByText("R-Streaming"));
    await waitFor(() => expect(screen.getByTestId("params")).toBeOnTheScreen());
    const params: unknown = JSON.parse(String(screen.getByTestId("params").props.children));
    expect(params).toMatchObject({ machine: "0199d3e0-de30-7000-8000-00000000d001" });
    expect(mockServer.current?.stageNow).toBe("streaming");
  });
});
