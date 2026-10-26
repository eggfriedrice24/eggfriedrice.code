import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { router } from "expo-router";
import {
  act,
  fireEvent,
  renderRouter,
  screen,
  type RenderRouterResult,
} from "expo-router/testing-library";
import { Pressable, Text } from "react-native";

import { BLANK_ROOT, draftKey } from "@/features/conversation/model/drafts";

import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { MAX_STACKED } from "./model/history";
import type { OpenOptions } from "./model/routes";
import { AppProviders } from "./components/app-providers";
import { RootStack } from "./components/root-stack";
import { DESKTOP, LAPTOP, conversationId, testServices, type TestServices } from "./model/testing";
import { useHeaderActions } from "./use-header-actions";
import { useOpenConversation } from "./use-open-conversation";

jest.mock("react-native-keyboard-controller", () =>
  jest.requireActual("react-native-keyboard-controller/jest"),
);

/** What the next press of "open" opens. */
let next: { target: ConversationRef; options: OpenOptions } = {
  target: { machine: DESKTOP, id: conversationId(0) },
  options: {},
};

function Controls(props: { readonly name: string; readonly rootMachine?: string }) {
  const openConversation = useOpenConversation();
  const actions = useHeaderActions(props.rootMachine);
  return (
    <>
      <Text>{props.name}</Text>
      <Pressable
        testID={`open-${props.name}`}
        onPress={() => openConversation(next.target, next.options)}
      />
      <Pressable testID={`new-${props.name}`} onPress={actions.newConversation} />
      <Pressable testID={`settings-${props.name}`} onPress={actions.openSettings} />
    </>
  );
}

let parts: TestServices | undefined;
let view: RenderRouterResult | undefined;

afterEach(() => {
  parts?.services.dispose();
  parts = undefined;
});

async function setup() {
  parts = await testServices();
  const { services } = parts;
  view = await renderRouter(
    {
      _layout: () => (
        <AppProviders services={services}>
          <RootStack machineMenu={() => null} />
        </AppProviders>
      ),
      index: () => <Controls name="root" rootMachine={DESKTOP} />,
      "c/[machine]/[id]": () => <Controls name="conversation" />,
      settings: () => <Text>settings</Text>,
      conversations: () => <Controls name="sheet" />,
      pair: () => <Text>pair</Text>,
    },
    { initialUrl: "/" },
  );
  return parts;
}

interface StackRouteState {
  readonly key: string;
  readonly name: string;
  readonly params?: object | undefined;
  readonly state?: unknown;
}

function routesOf(state: unknown): readonly StackRouteState[] {
  if (typeof state !== "object" || state === null || !("routes" in state)) {
    return [];
  }
  const { routes } = state;
  return Array.isArray(routes) ? routes.filter(isRoute) : [];
}

function isRoute(value: unknown): value is StackRouteState {
  return (
    typeof value === "object" &&
    value !== null &&
    "key" in value &&
    typeof value.key === "string" &&
    "name" in value &&
    typeof value.name === "string"
  );
}

/** The routes of the root stack: the state with the root route. */
function stackRoutes(): readonly StackRouteState[] {
  let routes = routesOf(rendered().getRouterState());
  while (routes.length > 0 && !routes.some((route) => route.name === "index")) {
    routes = routesOf(routes[0]?.state);
  }
  return routes;
}

function idOf(route: StackRouteState): string {
  const params = route.params;
  return params !== undefined && "id" in params && typeof params.id === "string" ? params.id : "";
}

/** The routes of the root stack, as names with the conversation id. */
function stack(): string[] {
  return stackRoutes().map((route) =>
    route.name === "c/[machine]/[id]" ? `c:${idOf(route)}` : route.name,
  );
}

function stackKeys(): string[] {
  return stackRoutes().map((route) => route.key);
}

function rendered(): RenderRouterResult {
  if (view === undefined) {
    throw new Error("the router is not rendered");
  }
  return view;
}

async function open(target: ConversationRef, options: OpenOptions = {}, from = "root") {
  next = { target, options };
  await act(async () => {
    await fireEvent.press(screen.getByTestId(`open-${from}`));
  });
}

function ref(n: number, machine = DESKTOP): ConversationRef {
  return { machine, id: conversationId(n) };
}

describe("opening a conversation", () => {
  it("pushes a conversation that is not in the stack", async () => {
    await setup();
    await open(ref(1));
    expect(stack()).toEqual(["index", `c:${conversationId(1)}`]);
    expect(screen).toHavePathname(`/c/${DESKTOP}/${conversationId(1)}`);
  });

  it("pushes the first send with enter=instant", async () => {
    await setup();
    await open(ref(1), { firstSend: true });
    expect(rendered().getSearchParams()).toMatchObject({ enter: "instant" });
  });

  it("pops back to a conversation in the stack and drops the ones above it", async () => {
    await setup();
    await open(ref(1));
    await open(ref(2), {}, "conversation");
    await open(ref(3), {}, "conversation");
    const keys = stackKeys();
    await open(ref(1), {}, "conversation");
    expect(stack()).toEqual(["index", `c:${conversationId(1)}`]);
    expect(stackKeys()).toEqual(keys.slice(0, 2));
  });

  it("gives the focus to a conversation that it pops back to", async () => {
    await setup();
    await open(ref(1));
    await open(ref(2), {}, "conversation");
    await open(ref(1), { focus: "approval" }, "conversation");
    expect(stack()).toEqual(["index", `c:${conversationId(1)}`]);
    expect(rendered().getSearchParams()).toMatchObject({ focus: "approval" });
  });

  it("keeps a conversation of another machine with the same id apart", async () => {
    await setup();
    await open(ref(1));
    await open({ machine: LAPTOP, id: conversationId(1) }, {}, "conversation");
    expect(stack()).toHaveLength(3);
  });

  it("drops the oldest conversation on the ninth push and keeps the other screens", async () => {
    await setup();
    await open(ref(0));
    for (let n = 1; n < MAX_STACKED; n += 1) {
      await open(ref(n), {}, "conversation");
    }
    expect(stack()).toHaveLength(MAX_STACKED + 1);
    const keys = stackKeys();
    await open(ref(MAX_STACKED), {}, "conversation");
    expect(stack()).toEqual([
      "index",
      ...Array.from({ length: MAX_STACKED - 1 }, (_, n) => `c:${conversationId(n + 1)}`),
      `c:${conversationId(MAX_STACKED)}`,
    ]);
    // The kept routes keep their keys, so no screen mounts again.
    expect(stackKeys().slice(0, MAX_STACKED)).toEqual([keys[0], ...keys.slice(2)]);
  });

  it("dismisses the sheet before it pushes a pick of the sheet", async () => {
    await setup();
    await open(ref(1));
    await act(async () => {
      router.push("/conversations");
    });
    expect(stack().at(-1)).toBe("conversations");
    await open(ref(2), {}, "sheet");
    expect(stack()).toEqual(["index", `c:${conversationId(1)}`, `c:${conversationId(2)}`]);
  });

  it("pops back from the sheet to a pick that is in the stack", async () => {
    await setup();
    await open(ref(1));
    await act(async () => {
      router.push("/conversations");
    });
    await open(ref(1), {}, "sheet");
    expect(stack()).toEqual(["index", `c:${conversationId(1)}`]);
  });
});

describe("the header actions", () => {
  it("+ pops to the blank root", async () => {
    await setup();
    await open(ref(1));
    await open(ref(2), {}, "conversation");
    await act(async () => {
      await fireEvent.press(screen.getByTestId("new-conversation"));
    });
    expect(stack()).toEqual(["index"]);
  });

  it("+ on the blank root clears its draft", async () => {
    const { services } = await setup();
    const key = draftKey(DESKTOP, BLANK_ROOT);
    services.drafts.set(key, "half a prompt");
    await act(async () => {
      await fireEvent.press(screen.getByTestId("new-root"));
    });
    expect(services.drafts.text(key)).toBe("");
    expect(stack()).toEqual(["index"]);
  });

  it("Settings pushes Settings", async () => {
    await setup();
    await act(async () => {
      await fireEvent.press(screen.getByTestId("settings-root"));
    });
    expect(screen).toHavePathname("/settings");
  });
});
