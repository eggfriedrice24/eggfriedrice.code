// The blank root (board Main, design 3.2) on the services of the app over fake sockets:
// the shell block of the machine of the root, the switch of the machine, the swap to the
// pairing intro when no machine is paired, and the first send, which pushes the new
// conversation and leaves a blank root under it.

import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { router } from "expo-router";
import {
  act,
  fireEvent,
  renderRouter,
  screen,
  type RenderRouterResult,
} from "expo-router/testing-library";
import type { ReactNode } from "react";
import { View } from "react-native";

import { flush } from "@/lib/net/testing";
import {
  DESKTOP,
  LAPTOP,
  testServices,
  type TestServices,
} from "@/features/navigation/model/testing";
import { BLANK_ROOT, draftKey } from "../model/drafts";
import { screenRoutes, socketOf } from "./screen-testing";

interface MockFlashListProps {
  readonly data: readonly string[];
  readonly renderItem: (info: { item: string; index: number }) => ReactNode;
  readonly keyExtractor: (item: string) => string;
}

/** FlashList is native: the fake one draws every row. */
function MockFlashList(props: MockFlashListProps) {
  return (
    <View>
      {props.data.map((item, index) => (
        <View key={props.keyExtractor(item)}>{props.renderItem({ item, index })}</View>
      ))}
    </View>
  );
}

jest.mock("@shopify/flash-list", () => ({ FlashList: MockFlashList }));
jest.mock("react-native-keyboard-controller", () =>
  jest.requireActual("react-native-keyboard-controller/jest"),
);
jest.mock("../components/fade-in", () => ({
  FadeIn: (props: { children: unknown }) => props.children,
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn(), setHapticsEnabled: jest.fn() }));

const CONVERSATION = "019a9b1c-3d00-7a10-8b20-0000000000c1";
const TURN = "019a9b1c-3d00-7a10-8b20-0000000000c2";

let test: TestServices | undefined;
let view: RenderRouterResult | undefined;

afterEach(() => {
  test?.services.dispose();
  test = undefined;
  view = undefined;
});

async function open(records?: Parameters<typeof testServices>[0]): Promise<TestServices> {
  test = await testServices(records);
  const current = test;
  view = await renderRouter(screenRoutes(current), { initialUrl: "/" });
  return current;
}

function rendered(): RenderRouterResult {
  if (view === undefined) {
    throw new Error("the router is not rendered");
  }
  return view;
}

/** Types `text` in the composer on top and taps Send. */
async function sendFromRoot(text: string): Promise<void> {
  const inputs = screen.getAllByTestId("composer-input");
  const input = inputs[inputs.length - 1];
  if (input === undefined) {
    throw new Error("no composer");
  }
  await fireEvent.changeText(input, text);
  const sends = screen.getAllByRole("button", { name: "Send" });
  const send = sends[sends.length - 1];
  if (send === undefined) {
    throw new Error("no Send button");
  }
  await fireEvent.press(send);
  await act(flush);
}

describe("the blank root", () => {
  it("shows the shell block of the machine of the root", async () => {
    await open();
    expect(screen.getByLabelText("desktop, in ~, cautious")).toBeOnTheScreen();
    // VoiceOver skips the prompt line; the label of the first line says it all.
    expect(screen.getByText(/❯/, { includeHiddenElements: true })).toBeOnTheScreen();
    expect(screen.getByTestId("block-cursor", { includeHiddenElements: true })).toBeOnTheScreen();
    expect(screen.getByTestId("composer-input").props.placeholder).toBe("Message efr");
  });

  it("follows a pick of the machine menu", async () => {
    const { services } = await open();
    await act(async () => {
      services.preferences.setLastMachine(LAPTOP);
    });
    expect(screen.getByLabelText("laptop, in ~, cautious")).toBeOnTheScreen();
  });

  it("is the pairing intro while no machine is paired", async () => {
    await open([]);
    expect(screen.getByText("no machine root")).toBeOnTheScreen();
    expect(screen.queryByTestId("composer")).toBeNull();
  });

  it("pushes a new conversation on the first send, then follows it when efrd made it", async () => {
    const parts = await open();
    await act(async () => {
      await parts.connect(DESKTOP);
    });
    await sendFromRoot("Why is the fan loud?");

    // The new conversation is on top with its local id, and shows the prompt on its way.
    const localId = String(rendered().getSearchParams()["id"] ?? "");
    expect(screen).toHavePathname(`/c/${DESKTOP}/${localId}`);
    expect(rendered().getSearchParams()).toMatchObject({ enter: "instant" });
    expect(screen.getByText("sending")).toBeOnTheScreen();
    expect(parts.services.drafts.text(draftKey(DESKTOP, BLANK_ROOT))).toBe("");

    // efrd makes the conversation: the route gets its id, and the screen subscribes.
    const socket = socketOf(parts, DESKTOP);
    expect(socket.requests("prompt.send").at(-1)?.["params"]).toMatchObject({
      new_conversation: true,
      text: "Why is the fan loud?",
    });
    await act(async () => {
      socket.answer("prompt.send", {
        conversation_id: CONVERSATION,
        queued: false,
        seq: 1,
        turn_id: TURN,
      });
      await flush();
    });
    expect(screen).toHavePathname(`/c/${DESKTOP}/${CONVERSATION}`);
    // The pop of this screen has the normal animation again.
    expect(rendered().getSearchParams()["enter"]).toBeUndefined();
    expect(socket.requests("conversation.subscribe").at(-1)?.["params"]).toMatchObject({
      conversation_id: CONVERSATION,
    });

    // The root under it is blank, and its next send makes another new conversation.
    await act(async () => {
      router.back();
    });
    expect(screen).toHavePathname("/");
    await sendFromRoot("And the disk?");
    const second = String(rendered().getSearchParams()["id"] ?? "");
    expect(second).not.toBe(localId);
    expect(second).not.toBe(CONVERSATION);
    expect(socket.requests("prompt.send").at(-1)?.["params"]).toMatchObject({
      new_conversation: true,
      text: "And the disk?",
    });
  });
});
