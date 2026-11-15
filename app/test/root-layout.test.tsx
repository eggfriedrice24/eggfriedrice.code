// The root stack as the root layout puts it together: the providers, the stack of the
// navigation feature and the machine menu of the machines feature. The last test adds
// the gate and the overlay of the Face ID unlock in the order of src/app/_layout.tsx. A feature does not
// import the components of another one, so this test lives outside src/, as the layout
// joins them.
import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { router } from "expo-router";
import { act, renderRouter, screen } from "expo-router/testing-library";
import type { ReactNode } from "react";
import { Text } from "react-native";

import { MachineMenu } from "@/features/machines/components/machine-menu";
import { AppProviders } from "@/features/navigation/components/app-providers";
import { LockGate, LockOverlay } from "@/features/navigation/components/lock-overlay";
import { RootStack } from "@/features/navigation/components/root-stack";
import {
  DESKTOP,
  conversationId,
  testServices,
  type TestServices,
} from "@/features/navigation/model/testing";

jest.mock("react-native-keyboard-controller", () =>
  jest.requireActual("react-native-keyboard-controller/jest"),
);
// FullWindowOverlay is native: here it is its children (lock-overlay.test.tsx).
jest.mock("react-native-screens", () => ({
  ...jest.requireActual<object>("react-native-screens"),
  FullWindowOverlay: (props: { readonly children: ReactNode }) => props.children,
}));

let parts: TestServices | undefined;

afterEach(() => {
  parts?.services.dispose();
  parts = undefined;
});

function machineMenu(machine: string) {
  return <MachineMenu current={machine} />;
}

async function setup(records?: Parameters<typeof testServices>[0]) {
  parts = await testServices(records);
  const { services } = parts;
  await renderRouter(
    {
      _layout: () => (
        <AppProviders services={services}>
          <RootStack machineMenu={machineMenu} />
        </AppProviders>
      ),
      index: () => <Text>root content</Text>,
      "c/[machine]/[id]": () => <Text>conversation content</Text>,
      settings: () => <Text>settings content</Text>,
      conversations: () => <Text>sheet content</Text>,
      pair: () => <Text>pair content</Text>,
    },
    { initialUrl: "/" },
  );
  return parts;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null;
}

/** The value of the prop `name` on the screen on top of the stack. */
function newestProp(name: string): unknown {
  let found: unknown;
  const visit = (node: unknown): void => {
    if (Array.isArray(node)) {
      for (const child of node) {
        visit(child);
      }
      return;
    }
    if (!isRecord(node)) {
      return;
    }
    const props = node["props"];
    if (isRecord(props) && name in props) {
      found = props[name];
    }
    visit(node["children"]);
  };
  visit(screen.toJSON());
  return found ?? [];
}

describe("the machine menu of the blank root", () => {
  it("lists each machine with its state, checks the machine of the root, and can pair", async () => {
    const test = await setup();
    await act(async () => {
      await test.connect(DESKTOP);
    });
    expect(screen.getByText("root content")).toBeOnTheScreen();
    expect(newestProp("headerLeftBarButtonItems")).toMatchObject([
      {
        type: "menu",
        accessibilityLabel: "Machine: desktop",
        icon: { type: "sfSymbol", name: "line.3.horizontal.decrease" },
        menu: {
          title: "Machine",
          items: [
            {
              type: "action",
              title: "desktop",
              subtitle: "Connected",
              state: "on",
              icon: { name: "desktopcomputer" },
            },
            {
              type: "action",
              title: "laptop",
              subtitle: "Connecting…",
              state: "off",
              icon: { name: "laptopcomputer" },
            },
            {
              type: "submenu",
              displayInline: true,
              items: [
                {
                  type: "action",
                  title: "Pair a new machine",
                  icon: { name: "qrcode.viewfinder" },
                },
              ],
            },
          ],
        },
      },
    ]);
  });

  it("is not on the root without a machine", async () => {
    await setup([]);
    expect(newestProp("headerLeftBarButtonItems")).toEqual([]);
  });

  it("is not on a conversation", async () => {
    await setup();
    await act(async () => {
      router.push(`/c/${DESKTOP}/${conversationId(1)}`);
    });
    expect(screen.getByText("conversation content")).toBeOnTheScreen();
    expect(newestProp("headerLeftBarButtonItems")).toEqual([]);
  });
});

describe("the root layout with the Face ID unlock (design test 22)", () => {
  it("shows only the lock screen for a saved switch, and the route of a link after the pass", async () => {
    parts = await testServices(undefined, { lockOn: true });
    const test = parts;
    const { services } = test;
    // The order of src/app/_layout.tsx.
    await renderRouter(
      {
        _layout: () => (
          <AppProviders services={services}>
            <LockGate>
              <RootStack machineMenu={machineMenu} />
            </LockGate>
            <LockOverlay />
          </AppProviders>
        ),
        index: () => <Text>root content</Text>,
        conversations: () => <Text>sheet content</Text>,
      },
      { initialUrl: "/conversations" },
    );
    expect(screen.getByTestId("lock-screen")).toBeOnTheScreen();
    expect(screen.queryByText("sheet content")).toBeNull();
    expect(screen.queryByText("root content")).toBeNull();
    await act(async () => {
      await test.answerCheck({ kind: "passed" });
    });
    expect(screen.queryByTestId("lock-screen")).toBeNull();
    expect(screen.getByText("sheet content")).toBeOnTheScreen();
  });
});
