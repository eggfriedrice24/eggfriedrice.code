// The component boards of the gallery in the test router of expo-router: each one shows
// the real screen or component in its state. The transcript list and the keyboard are
// native, so they get the fakes of the transcript tests. No test reaches a network.

import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider } from "@tanstack/react-query";
import { Stack, useLocalSearchParams } from "expo-router";
import { fireEvent, renderRouter, screen, waitFor } from "expo-router/testing-library";
import type { ReactNode } from "react";
import { ScrollView, View } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { testServices, type TestServices } from "@/features/navigation/model/testing";
import { AppServicesContext } from "@/features/navigation/use-app-services";
import { PAIRING_BOARDS } from "@/features/pairing/model/pairing-boards";

import { boardParams } from "../model/board-schemas";
import { approvalCases } from "../model/demo-conversations";
import { pairingBoardName } from "../model/boards";
import { BoardScreen } from "./board-screen";

interface MockListProps {
  readonly data: readonly string[];
  readonly renderItem: (info: { item: string; index: number }) => ReactNode;
  readonly keyExtractor: (item: string) => string;
}

/** The list library is native: the fake list draws every row. */
function MockFlashList(props: MockListProps) {
  return (
    <ScrollView testID="flash-list">
      {props.data.map((item, index) => (
        <View key={props.keyExtractor(item)} testID={`cell-${item}`}>
          {props.renderItem({ item, index })}
        </View>
      ))}
    </ScrollView>
  );
}

jest.mock("@shopify/flash-list", () => ({ FlashList: MockFlashList }));
jest.mock("react-native-keyboard-controller", () => {
  const actual: Record<string, unknown> = jest.requireActual(
    "react-native-keyboard-controller/jest",
  );
  return { ...actual, KeyboardChatScrollView: () => null };
});
jest.mock("@/features/conversation/components/fade-in", () => ({
  FadeIn: (props: { children: unknown }) => props.children,
}));
jest.mock("@/features/pairing/native/native-pairing", () => ({
  nativeScanner: () => null,
  suggestedDeviceName: () => "iPhone",
}));
jest.mock("@/lib/native/clipboard", () => ({ copyText: jest.fn(() => Promise.resolve(true)) }));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

let parts: TestServices | undefined;

afterEach(() => {
  parts?.services.dispose();
  parts = undefined;
});

function BoardRoute() {
  return <BoardScreen {...boardParams(useLocalSearchParams())} />;
}

/** Renders the board `name` in a stack with the services of a test. */
async function showBoard(name: string, extra: Record<string, string> = {}) {
  parts = await testServices([]);
  const { services } = parts;
  const query = new URLSearchParams({ name, ...extra }).toString();
  await renderRouter(
    {
      _layout: () => (
        <SafeAreaProvider initialMetrics={METRICS}>
          <QueryClientProvider client={services.queryClient}>
            <AppServicesContext value={services}>
              <Stack />
            </AppServicesContext>
          </QueryClientProvider>
        </SafeAreaProvider>
      ),
      "(dev)/board": BoardRoute,
    },
    { initialUrl: `/board?${query}` },
  );
}

describe("the component boards of the gallery", () => {
  it("shows every state of the approval block", async () => {
    await showBoard("S-Approvals");
    for (const one of approvalCases(() => "0199d3e0-de3a-7000-8000-000000000001")) {
      expect(screen.getByText(one.name)).toBeOnTheScreen();
    }
    expect(screen.getAllByTestId("approval-block").length).toBeGreaterThan(5);
  });

  it("gives the docked composer the state that the user picks", async () => {
    await showBoard("S-Composer");
    expect(screen.getByTestId("composer")).toBeOnTheScreen();
    await fireEvent.press(screen.getByText("disconnected: offline"));
    expect(screen.getByPlaceholderText("desktop is offline")).toBeOnTheScreen();
    await fireEvent.press(screen.getByText("disconnected: efrd is starting"));
    expect(screen.getByPlaceholderText("efrd is starting")).toBeOnTheScreen();
  });

  it.each(["S-Messages", "S-Markdown", "S-Tools", "S-Diff"])(
    "draws the rows of the transcript sheet %s",
    async (name) => {
      await showBoard(name);
      expect(screen.getAllByTestId(/^cell-/).length).toBeGreaterThan(1);
    },
  );

  it("draws the loaders", async () => {
    await showBoard("S-Loaders");
    expect(screen.getByText("running line")).toBeOnTheScreen();
    expect(screen.getByText("loading line")).toBeOnTheScreen();
  });

  it("draws the icon", async () => {
    await showBoard("Icon");
    expect(screen.getByText("settings, 29 pt")).toBeOnTheScreen();
  });

  it.each(PAIRING_BOARDS.map((board) => [board.name, board.title]))(
    "opens the pairing board %s in the pairing modal",
    async (name, title) => {
      await showBoard(pairingBoardName(name));
      expect(screen.getByText(title)).toBeOnTheScreen();
    },
  );

  it("shows the root without a machine", async () => {
    await showBoard("R-NoMachine");
    expect(screen.getByText("no machine paired")).toBeOnTheScreen();
  });

  it("shows Settings without a machine", async () => {
    await showBoard("Settings · no machine");
    expect(screen.getByText("Pair a new machine")).toBeOnTheScreen();
  });

  it("shows Settings with one demo machine in a sandbox", async () => {
    await showBoard("Settings-ForgetLast", { visit: "1" });
    await waitFor(() => expect(screen.getByText("desktop")).toBeOnTheScreen());
  });

  it("names a board that it does not know", async () => {
    await showBoard("S-Nothing");
    expect(screen.getByText("no board S-Nothing")).toBeOnTheScreen();
  });
});
