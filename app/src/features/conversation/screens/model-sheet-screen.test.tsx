// The model sheet on the services of the app over fake sockets (S-ModelSheet, MS-*,
// AC-Out 5): the model button opens it, a pick shows the confirm, the confirm stores the
// choice and closes the sheet, and the next prompt carries the model and the account.

import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { notifyManager } from "@tanstack/react-query";
import { router } from "expo-router";
import {
  act,
  fireEvent,
  renderRouter,
  screen,
  type RenderRouterResult,
} from "expo-router/testing-library";
import { ScrollView, View } from "react-native";
import type { ReactNode } from "react";

import { flush } from "@/lib/net/testing";
import type { Event } from "@/lib/protocol/wire";
import type { ModelsListResult } from "@/lib/protocol/generated";
import {
  envelope,
  promptQueued as queued,
  snapshotItem,
  turnCompleted,
} from "@/lib/stream/testing";
import {
  DESKTOP,
  conversationId,
  testServices,
  type TestServices,
} from "@/features/navigation/model/testing";
import { draftKey } from "../model/drafts";

import { screenRoutes, socketOf, stream } from "./screen-testing";

interface MockFlashListProps {
  readonly data: readonly string[];
  readonly renderItem: (info: { item: string; index: number }) => ReactNode;
  readonly keyExtractor: (item: string) => string;
}

/** FlashList is native: the fake one draws every row, oldest first. */
function MockFlashList(props: MockFlashListProps) {
  return (
    <ScrollView testID="flash-list">
      {props.data.map((item, index) => (
        <View key={props.keyExtractor(item)}>{props.renderItem({ item, index })}</View>
      ))}
    </ScrollView>
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

const CONVERSATION = conversationId(1);
const TURN = "019a9b1c-3d00-7a10-8b20-0000000000f1";
const PLAN = "openai-subscription";
const PLAN_B = "openai-subscription.b";
const SOL = { efforts: ["low", "medium", "high"], default_effort: "medium" };

const MODELS: ModelsListResult = {
  models: [
    { id: "gpt-6.1-sol", provider: PLAN, source: "builtin", default: true, ...SOL },
    { id: "gpt-6.1-sol", provider: PLAN_B, source: "builtin", ...SOL },
    { id: "gpt-5.5", provider: PLAN, source: "builtin", context_window: 400_000 },
  ],
  catalogs: [
    { provider: PLAN, origin: "backend" },
    { provider: PLAN_B, origin: "backend" },
  ],
};

let test: TestServices | undefined;
let view: RenderRouterResult | undefined;

beforeEach(() => {
  // Query tells its observers at once, so a test sees an answer after one flush.
  notifyManager.setScheduler((callback) => callback());
});

afterEach(() => {
  test?.services.dispose();
  test = undefined;
  view = undefined;
});

function services(): TestServices {
  if (test === undefined) {
    throw new Error("no services");
  }
  return test;
}

/** A snapshot of CONVERSATION with one turn that ran gpt-6.1-sol on the ChatGPT plan. */
function finished() {
  const started: Event = {
    kind: "turn_started",
    turn_id: TURN,
    cwd: "/home/me",
    scope: { kind: "machine" },
    settings: { mode: "cautious", model: "gpt-6.1-sol", provider: PLAN, effort: "medium" },
  };
  return snapshotItem(
    CONVERSATION,
    "idle",
    [
      envelope(1, {
        ...queued(TURN, "Fix the race."),
        command_id: "019a9b1c-3d00-7a10-8b20-000000000001",
      }),
      envelope(2, started),
      envelope(3, turnCompleted(TURN)),
    ],
    3,
  );
}

/** The conversation, its stream, and the answer of `models.list`. */
async function openConversation(): Promise<void> {
  test = await testServices();
  await test.connect(DESKTOP);
  view = await renderRouter(screenRoutes(test), { initialUrl: "/" });
  await act(async () => {
    router.push(`/c/${DESKTOP}/${CONVERSATION}`);
  });
  await act(flush);
  await act(async () => {
    await stream(services(), DESKTOP, CONVERSATION, [finished()]);
  });
  await act(async () => {
    socketOf(services(), DESKTOP).answer("models.list", MODELS);
    await flush();
  });
}

describe("the model sheet", () => {
  it("opens from the model button, and a switch of account alone confirms with the account", async () => {
    await openConversation();
    const button = screen.getByTestId("composer-model-button");
    expect(button).toHaveTextContent("GPT-6.1 Sol · ChatGPT plan · medium");
    await fireEvent.press(button);
    await act(flush);
    expect(view?.getPathname()).toBe("/model");
    expect(view?.getSearchParams()).toMatchObject({ machine: DESKTOP, id: CONVERSATION });
    expect(screen.getByTestId("model-sheet")).toBeOnTheScreen();
    expect(screen.getByText("ChatGPT plan · b")).toBeOnTheScreen();
    // Nothing changed: no confirm.
    expect(screen.queryByRole("button", { name: /^Use / })).toBeNull();

    await fireEvent.press(screen.getByTestId(`model-row-${PLAN_B}/gpt-6.1-sol`));
    await act(flush);
    expect(screen.getByTestId("model-quiet-line")).toHaveTextContent(
      "GPT-6.1 Sol · on ChatGPT plan · b",
    );
    // The phone asks efrd what the switch costs; a cold cache stays silent.
    const socket = socketOf(services(), DESKTOP);
    expect(socket.requests("conversation.switch_preview").at(-1)?.["params"]).toEqual({
      conversation_id: CONVERSATION,
      model: "gpt-6.1-sol",
      provider: PLAN_B,
    });
    await act(async () => {
      socket.answer("conversation.switch_preview", { cost: "cold_cache", provider: PLAN_B });
      await flush();
    });
    expect(screen.queryByTestId("model-cost-line")).toBeNull();
    // One slider for the selected model: its steps, the default marked. A build without
    // the native slider (as the tests) has the steps as buttons.
    expect(screen.getByRole("button", { name: "medium effort, default" })).toBeSelected();
    await fireEvent.press(screen.getByRole("button", { name: "high effort" }));
    expect(screen.getByRole("button", { name: "Use GPT-6.1 Sol · b · high" })).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "medium effort, default" }));
    await fireEvent.press(screen.getByRole("button", { name: "Use ChatGPT plan · b" }));
    await act(flush);
    expect(view?.getPathname()).toBe(`/c/${DESKTOP}/${CONVERSATION}`);
    expect(services().services.modelChoices.choice(draftKey(DESKTOP, CONVERSATION))).toEqual({
      model: "gpt-6.1-sol",
      provider: PLAN_B,
      effort: "medium",
    });
    expect(screen.getByTestId("composer-model-button")).toHaveTextContent(
      "GPT-6.1 Sol · b · medium",
    );

    // The next prompt carries the model and the account.
    await fireEvent.changeText(screen.getByTestId("composer-input"), "Review it.");
    await fireEvent.press(screen.getByRole("button", { name: "Send" }));
    await act(flush);
    expect(socket.requests("prompt.send").at(-1)?.["params"]).toMatchObject({
      conversation_id: CONVERSATION,
      text: "Review it.",
      settings: { model: "gpt-6.1-sol", provider: PLAN_B, effort: "medium" },
    });
  });

  it("warns of a compaction, and keeps 'Don't ask again here' for the conversation", async () => {
    await openConversation();
    await act(async () => {
      router.push(`/model?machine=${DESKTOP}&id=${CONVERSATION}`);
    });
    await act(flush);
    await fireEvent.press(screen.getByTestId(`model-row-${PLAN}/gpt-5.5`));
    await act(flush);
    // While efrd answers, the confirm waits.
    expect(screen.getByRole("button", { name: "Asking what the switch costs" })).toBeOnTheScreen();
    await act(async () => {
      socketOf(services(), DESKTOP).answer("conversation.switch_preview", { cost: "compacts" });
      await flush();
    });
    expect(screen.getByTestId("model-cost-line")).toHaveTextContent(
      "Compacts this conversation and starts a cold cache.",
    );
    expect(screen.getByTestId("model-quiet-line")).toHaveTextContent(
      "GPT-5.5 · 400k context · no effort setting",
    );
    await fireEvent.press(
      screen.getByRole("switch", { name: "Don't ask again in this conversation" }),
    );
    expect(services().services.modelChoices.quiet(draftKey(DESKTOP, CONVERSATION))).toBe(true);
    await fireEvent.press(screen.getByRole("button", { name: "Switch and compact" }));
    await act(flush);
    expect(services().services.modelChoices.choice(draftKey(DESKTOP, CONVERSATION))).toEqual({
      model: "gpt-5.5",
      provider: PLAN,
    });
  });

  it("switches at once on a tap of a row while Settings asks before no switch", async () => {
    await openConversation();
    services().services.preferences.setAskCostlySwitch(false);
    await act(async () => {
      router.push(`/model?machine=${DESKTOP}&id=${CONVERSATION}`);
    });
    await act(flush);
    await fireEvent.press(screen.getByTestId(`model-row-${PLAN}/gpt-5.5`));
    await act(flush);
    expect(view?.getPathname()).toBe(`/c/${DESKTOP}/${CONVERSATION}`);
    expect(socketOf(services(), DESKTOP).requests("conversation.switch_preview")).toEqual([]);
    expect(services().services.modelChoices.choice(draftKey(DESKTOP, CONVERSATION))).toEqual({
      model: "gpt-5.5",
      provider: PLAN,
    });
  });

  it("opens with the row of a switch offer selected", async () => {
    await openConversation();
    await act(async () => {
      router.push(
        `/model?machine=${DESKTOP}&id=${CONVERSATION}&model=gpt-6.1-sol&provider=${PLAN_B}`,
      );
    });
    await act(flush);
    expect(screen.getByTestId(`model-row-${PLAN_B}/gpt-6.1-sol`)).toBeSelected();
    expect(screen.getByRole("button", { name: "Asking what the switch costs" })).toBeOnTheScreen();
  });
});
