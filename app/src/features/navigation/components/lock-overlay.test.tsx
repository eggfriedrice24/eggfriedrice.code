// The Face ID unlock over the window (S-Lock): the gate of the stack, the lock screen and
// the cover, on the real services with a fake iPhone. `FullWindowOverlay` is a plain view
// here; that it lies over a form sheet is a check on the iPhone (README, check 36).

import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import type { ReactElement, ReactNode } from "react";
import { Text } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { testServices, type TestServices } from "../model/testing";
import { AppServicesContext } from "../use-app-services";
import { LockGate, LockOverlay } from "./lock-overlay";
import { LockCover, LockScreen } from "./lock-screen";

jest.mock("react-native-screens", () => ({
  ...jest.requireActual<object>("react-native-screens"),
  FullWindowOverlay: (props: { readonly children: ReactNode }) => props.children,
}));

const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

let parts: TestServices | undefined;

afterEach(() => {
  // NOTE: clear the queries, or the gc timer of 5 minutes keeps jest waiting.
  parts?.services.queryClient.clear();
  parts?.services.dispose();
  parts = undefined;
});

async function draw(lockOn: boolean): Promise<TestServices> {
  parts = await testServices(undefined, { lockOn });
  const test = parts;
  await render(
    <SafeAreaProvider initialMetrics={METRICS}>
      <QueryClientProvider client={test.services.queryClient}>
        <AppServicesContext value={test.services}>
          <LockGate>
            <Text testID="app">a conversation about the deploy key</Text>
          </LockGate>
          <LockOverlay />
        </AppServicesContext>
      </QueryClientProvider>
    </SafeAreaProvider>,
  );
  return test;
}

/** The classes of the place of the mark. */
function place(): unknown {
  return screen.getByTestId("lock-mark-place", { includeHiddenElements: true }).props.className;
}

function inSafeArea(node: ReactNode): ReactElement {
  return <SafeAreaProvider initialMetrics={METRICS}>{node}</SafeAreaProvider>;
}

describe("the Face ID unlock over the window", () => {
  it("shows the app and no overlay while the switch is off", async () => {
    await draw(false);
    expect(screen.getByTestId("app")).toBeOnTheScreen();
    expect(screen.queryByTestId("lock-screen")).toBeNull();
    expect(screen.queryByTestId("lock-cover", { includeHiddenElements: true })).toBeNull();
  });

  it("shows only the lock screen at a cold start, and the app after the pass", async () => {
    const test = await draw(true);
    expect(screen.getByTestId("lock-screen")).toBeOnTheScreen();
    expect(screen.queryByTestId("app")).toBeNull();
    // Nothing of a conversation on the lock screen.
    expect(screen.queryByText(/deploy key/)).toBeNull();
    await act(async () => {
      await test.answerCheck({ kind: "passed" });
    });
    expect(screen.getByTestId("app")).toBeOnTheScreen();
    expect(screen.queryByTestId("lock-screen")).toBeNull();
  });

  it("covers the app while it is inactive, and takes the cover away when it is back", async () => {
    const test = await draw(true);
    await act(async () => {
      await test.answerCheck({ kind: "passed" });
    });
    await act(async () => {
      test.setPhase("inactive");
    });
    expect(screen.getByTestId("lock-cover", { includeHiddenElements: true })).toBeOnTheScreen();
    // The app stays mounted under the cover.
    expect(screen.getByTestId("app", { includeHiddenElements: true })).toBeOnTheScreen();
    await act(async () => {
      test.setPhase("active");
    });
    expect(screen.queryByTestId("lock-cover", { includeHiddenElements: true })).toBeNull();
  });

  it("says why after a cancel, and asks again on a tap on Unlock", async () => {
    const test = await draw(true);
    await act(async () => {
      await test.answerCheck({ kind: "cancelled", method: "Face ID" });
    });
    expect(screen.getByTestId("lock-line")).toHaveTextContent("You cancelled Face ID.");
    expect(test.checkReasons).toEqual(["Unlock efr"]);
    await fireEvent.press(screen.getByRole("button", { name: "Unlock" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(test.checkReasons).toEqual(["Unlock efr", "Unlock efr"]);
  });

  it("keeps the mark at the place of the cover, with and without the line (S-Lock 7, 8a, 11)", async () => {
    const view = await render(inSafeArea(<LockCover />));
    const cover = place();
    expect(cover).toContain("absolute inset-0");
    await view.rerender(inSafeArea(<LockScreen line={undefined} hint="" onUnlock={() => {}} />));
    expect(place()).toBe(cover);
    await view.rerender(
      inSafeArea(<LockScreen line="You cancelled Face ID." hint="" onUnlock={() => {}} />),
    );
    expect(place()).toBe(cover);
    // The line lies outside the flow of the mark, so it does not move it.
    expect(screen.getByTestId("lock-line").props.className).toContain("absolute");
  });

  it("says that Face ID is locked, and gives VoiceOver the words of the board", async () => {
    const test = await draw(true);
    await act(async () => {
      await test.answerCheck({ kind: "locked", method: "Face ID" });
    });
    expect(screen.getByTestId("lock-line")).toHaveTextContent(
      "Face ID is locked. Unlock asks for the passcode.",
    );
    expect(screen.getByRole("header", { name: "efr is locked." })).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Unlock" }).props.accessibilityHint).toBe(
      "Asks for the passcode of this iPhone.",
    );
  });
});
