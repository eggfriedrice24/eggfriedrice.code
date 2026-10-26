import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { router } from "expo-router";
import {
  act,
  fireEvent,
  renderRouter,
  screen,
  type RenderRouterResult,
} from "expo-router/testing-library";
import { Text } from "react-native";

import { OFFLINE_GRACE_MS } from "../model/header-slot";
import {
  DESKTOP,
  LAPTOP,
  TEST_NOW,
  conversationId,
  lastSocket,
  setRecent,
  testServices,
  type TestServices,
} from "../model/testing";
import { AppProviders } from "./app-providers";
import { RootStack } from "./root-stack";

jest.mock("react-native-keyboard-controller", () =>
  jest.requireActual("react-native-keyboard-controller/jest"),
);

let parts: TestServices | undefined;
let view: RenderRouterResult | undefined;

afterEach(() => {
  parts?.services.dispose();
  parts = undefined;
});

interface SetupOptions {
  readonly records?: Parameters<typeof testServices>[0];
}

/** The machine menu of these tests; test/root-layout.test.tsx renders the real one. */
function machineMenu(machine: string) {
  return <Text>{`machine menu ${machine}`}</Text>;
}

async function setup(options: SetupOptions = {}) {
  parts = await testServices(options.records);
  const { services } = parts;
  view = await renderRouter(
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

/**
 * Runs `change`, then lets Query tell its observers: it tells them in a timer, and
 * renderRouter turns on the fake timers of jest.
 */
async function settled(change: () => void) {
  await act(async () => {
    change();
    jest.advanceTimersByTime(1);
  });
}

/** Connects the machines, inside act, because each state of a client renders. */
async function online(test: TestServices, ...machines: string[]) {
  for (const machine of machines) {
    await act(async () => {
      await test.connect(machine);
    });
  }
}

function rendered(): RenderRouterResult {
  if (view === undefined) {
    throw new Error("the router is not rendered");
  }
  return view;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null;
}

/** The values of the prop `name` in the rendered tree, oldest screen first. */
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

/** The value of the prop `name` on the screen on top of the stack. */
function newestProp(name: string): unknown {
  return propValues(name).at(-1);
}

/** The native bar items on one side of the screen on top of the stack. */
function barItems(side: "headerLeftBarButtonItems" | "headerRightBarButtonItems"): unknown {
  return newestProp(side) ?? [];
}

/** The newest element with `testID`: the screen on top of the stack. */
function newest(testID: string) {
  const found = screen.getAllByTestId(testID);
  const element = found[found.length - 1];
  if (element === undefined) {
    throw new Error(`no element ${testID}`);
  }
  return element;
}

async function push(href: string) {
  await act(async () => {
    router.push(href);
  });
}

describe("the header of the blank root", () => {
  it("shows the machine menu and the machine name, which opens the sheet", async () => {
    await setup();
    expect(screen.getByText(`machine menu ${DESKTOP}`)).toBeOnTheScreen();
    const title = screen.getByTestId("title-button");
    expect(title.props.accessibilityLabel).toBe("desktop, all conversations");
    expect(screen.queryByTestId("title-menu")).toBeNull();
    expect(screen.queryByTestId("title-dot")).toBeNull();
  });

  it("starts on the last machine of the preferences", async () => {
    const { services } = await setup();
    await act(async () => {
      services.preferences.setLastMachine(LAPTOP);
    });
    expect(screen.getByText(`machine menu ${LAPTOP}`)).toBeOnTheScreen();
    expect(screen.getByTestId("title-button").props.accessibilityLabel).toBe(
      "laptop, all conversations",
    );
  });

  it("opens the conversations sheet on a tap, with the machine of the screen", async () => {
    await setup();
    await act(async () => {
      await fireEvent.press(screen.getByTestId("title-button"));
    });
    expect(screen).toHavePathname("/conversations");
    expect(rendered().getSearchParams()).toMatchObject({ machine: DESKTOP });
  });

  it("shows the dot while a conversation waits, on any machine", async () => {
    const test = await setup();
    await online(test, LAPTOP);
    await settled(() => {
      setRecent(test.services.queryClient, LAPTOP, [
        { id: conversationId(2), title: "Fix steer race", status: "awaiting_approval" },
      ]);
    });
    expect(screen.getByTestId("title-dot")).toBeOnTheScreen();
    expect(screen.getByTestId("title-button").props.accessibilityLabel).toBe(
      "desktop, all conversations, 1 approval waiting",
    );
  });

  it("shows no machine menu and no title menu without a machine", async () => {
    await setup({ records: [] });
    expect(screen.getByText("root content")).toBeOnTheScreen();
    expect(screen.queryByTestId("title-button")).toBeNull();
    expect(screen.queryByText(/machine menu/)).toBeNull();
  });
});

describe("the bar buttons", () => {
  it("has Settings and + on the right, in the dark style", async () => {
    await setup();
    // iOS lays the right items out from the right edge: + is the last one on the screen.
    expect(barItems("headerRightBarButtonItems")).toMatchObject([
      { type: "button", accessibilityLabel: "New conversation", icon: { name: "plus" } },
      { type: "button", accessibilityLabel: "Settings", icon: { name: "gearshape" } },
    ]);
    expect(newestProp("experimental_userInterfaceStyle")).toBe("dark");
  });

  it("has Settings only on the root without a machine", async () => {
    await setup({ records: [] });
    expect(barItems("headerRightBarButtonItems")).toMatchObject([
      { type: "button", accessibilityLabel: "Settings" },
    ]);
    expect(barItems("headerRightBarButtonItems")).toHaveLength(1);
    expect(barItems("headerLeftBarButtonItems")).toEqual([]);
  });

  it("has Settings and + on a conversation, and no machine menu", async () => {
    await setup();
    await push(`/c/${DESKTOP}/${conversationId(1)}`);
    expect(barItems("headerRightBarButtonItems")).toHaveLength(2);
    expect(barItems("headerLeftBarButtonItems")).toEqual([]);
  });
});

describe("the header of a conversation", () => {
  it("shows the title of the recent list", async () => {
    const { services } = await setup();
    await settled(() => {
      setRecent(services.queryClient, DESKTOP, [
        { id: conversationId(1), title: "Why is the fan loud" },
      ]);
    });
    await push(`/c/${DESKTOP}/${conversationId(1)}`);
    expect(newest("title-button").props.accessibilityLabel).toBe(
      "Why is the fan loud, all conversations",
    );
  });

  it("reads Untitled for a conversation without a title yet", async () => {
    await setup();
    await push(`/c/${DESKTOP}/${conversationId(5)}`);
    expect(newest("title-button").props.accessibilityLabel).toBe("Untitled, all conversations");
  });

  it("shows the strip and the dot for one approval elsewhere, and opens it", async () => {
    const test = await setup();
    const { services } = test;
    await online(test, DESKTOP);
    await settled(() => {
      setRecent(services.queryClient, DESKTOP, [
        { id: conversationId(1), title: "Fan" },
        { id: conversationId(2), title: "Fix steer race", status: "awaiting_approval" },
      ]);
    });
    await push(`/c/${DESKTOP}/${conversationId(1)}`);
    const strip = screen.getByRole("button", {
      name: "Approval waiting, desktop, Fix steer race",
    });
    expect(screen.getAllByTestId("title-dot").length).toBeGreaterThan(0);
    await act(async () => {
      await fireEvent.press(strip);
    });
    expect(screen).toHavePathname(`/c/${DESKTOP}/${conversationId(2)}`);
    expect(rendered().getSearchParams()).toMatchObject({ focus: "approval" });
  });

  it("does not count the approval of the conversation on screen", async () => {
    const test = await setup();
    const { services } = test;
    await online(test, DESKTOP);
    await settled(() => {
      setRecent(services.queryClient, DESKTOP, [
        { id: conversationId(2), title: "Fix steer race", status: "awaiting_approval" },
      ]);
    });
    await push(`/c/${DESKTOP}/${conversationId(2)}`);
    expect(screen.queryByTestId("header-strip")).toBeNull();
  });

  it("leaves out the old list of a machine that is not connected", async () => {
    const { services } = await setup();
    await settled(() => {
      setRecent(services.queryClient, LAPTOP, [
        { id: conversationId(2), title: "Old wait", status: "awaiting_approval" },
      ]);
    });
    await push(`/c/${DESKTOP}/${conversationId(1)}`);
    expect(screen.queryByText(/Approval waiting/)).toBeNull();
  });

  it("shows the plural strip, which opens the sheet with the waiting section", async () => {
    const test = await setup();
    const { services } = test;
    await online(test, DESKTOP, LAPTOP);
    await settled(() => {
      setRecent(services.queryClient, DESKTOP, [
        { id: conversationId(2), title: "Fix steer race", status: "awaiting_approval" },
      ]);
      setRecent(services.queryClient, LAPTOP, [
        { id: conversationId(3), title: "Backups", status: "awaiting_approval" },
      ]);
    });
    await push(`/c/${DESKTOP}/${conversationId(1)}`);
    const strip = screen.getByRole("button", { name: "2 approvals waiting" });
    await act(async () => {
      await fireEvent.press(strip);
    });
    expect(screen).toHavePathname("/conversations");
    expect(rendered().getSearchParams()).toMatchObject({ section: "waiting" });
  });

  it("shows the offline line when the machine of the screen does not answer", async () => {
    const test = await setup();
    // The clock of the machines and the clock of the screens show the same time.
    jest.setSystemTime(TEST_NOW);
    await online(test, DESKTOP);
    await push(`/c/${DESKTOP}/${conversationId(1)}`);
    expect(screen.queryByTestId("header-strip")).toBeNull();
    await act(async () => {
      lastSocket(test.sockets, "ws://100.64.0.1:7419/").drop();
    });
    // At first the client only connects again, and the composer says "reconnecting".
    expect(screen.queryByTestId("header-strip")).toBeNull();
    await act(async () => {
      jest.advanceTimersByTime(OFFLINE_GRACE_MS);
    });
    const strip = screen.getByTestId("header-strip");
    // A glass capsule 12 pt from the sides and 8 pt under the bar.
    expect(strip.props.className).toContain(
      "right-12 left-12 h-strip overflow-hidden rounded-full",
    );
    expect(strip).toContainElement(
      screen.getByTestId("material-glass", { includeHiddenElements: true }),
    );
    expect(screen.queryByTestId("hairline", { includeHiddenElements: true })).toBeNull();
    expect(
      screen.getByRole("text", { name: /^desktop offline · seen \d\d:\d\d/ }),
    ).toBeOnTheScreen();
    expect(screen.queryByRole("button", { name: /Approval waiting/ })).toBeNull();
  });
});
