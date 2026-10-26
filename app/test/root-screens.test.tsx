// The real root and conversation screens inside the root stack, as the routes of src/app
// put them together: the header of the navigation feature around the screens of the
// conversation and pairing features. A feature does not import the components of another
// one, so this test lives outside src/, as the root layout joins them.
import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { router } from "expo-router";
import { act, renderRouter, screen } from "expo-router/testing-library";
import { Text } from "react-native";

import { BlankRootScreen } from "@/features/conversation/screens/blank-root-screen";
import { ConversationScreen } from "@/features/conversation/screens/conversation-screen";
import { MachineMenu } from "@/features/machines/components/machine-menu";
import { AppProviders } from "@/features/navigation/components/app-providers";
import { RootStack } from "@/features/navigation/components/root-stack";
import {
  DESKTOP,
  conversationId,
  testServices,
  type TestServices,
} from "@/features/navigation/model/testing";
import { NoMachineRootScreen } from "@/features/pairing/screens/no-machine-root-screen";

jest.mock("@shopify/flash-list", () => ({ FlashList: () => null }));
jest.mock("react-native-keyboard-controller", () =>
  jest.requireActual("react-native-keyboard-controller/jest"),
);
jest.mock("@/features/pairing/native/native-pairing", () => ({
  nativeScanner: () => null,
  suggestedDeviceName: () => "iPhone",
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn(), setHapticsEnabled: jest.fn() }));

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
      index: () => <BlankRootScreen noMachine={<NoMachineRootScreen />} />,
      "c/[machine]/[id]": () => (
        <ConversationScreen conversation={{ machine: DESKTOP, id: conversationId(1) }} />
      ),
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

/** Every value of the prop `name` in the rendered tree, in order. */
function propValues(name: string): unknown[] {
  const found: unknown[] = [];
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
      found.push(props[name]);
    }
    visit(node["children"]);
  };
  visit(screen.toJSON());
  return found;
}

/** The accessibility labels of the bar buttons on the right of the screen on top. */
function rightButtons(): unknown[] {
  const items = propValues("headerRightBarButtonItems").at(-1);
  return Array.isArray(items)
    ? items.map((item: unknown) => (isRecord(item) ? item["accessibilityLabel"] : undefined))
    : [];
}

describe("the root screens in the root stack", () => {
  it("shows the pairing intro with Settings only while no machine is paired", async () => {
    await setup([]);
    expect(screen.getByText("no machine paired")).toBeOnTheScreen();
    expect(rightButtons()).toEqual(["Settings"]);
  });

  it("shows the blank root with Settings and + on the machine of the root", async () => {
    await setup();
    expect(screen.getByLabelText("desktop, in ~, cautious")).toBeOnTheScreen();
    expect(rightButtons()).toHaveLength(2);
    expect(rightButtons()).toEqual(expect.arrayContaining(["Settings", "New conversation"]));
  });

  it("pushes the conversation screen under the header of a conversation", async () => {
    await setup();
    await act(async () => {
      router.push(`/c/${DESKTOP}/${conversationId(1)}`);
    });
    expect(screen.getByLabelText("loading conversation from desktop")).toBeOnTheScreen();
    expect(rightButtons()).toHaveLength(2);
    expect(rightButtons()).toEqual(expect.arrayContaining(["Settings", "New conversation"]));
  });
});
