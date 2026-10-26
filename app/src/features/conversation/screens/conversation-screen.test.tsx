// The conversation screen on the services of the app over fake sockets (boards
// R-Streaming, R-Approval, C-Failed and C-Offline, and the undrawn loading state): the
// subscription follows the focus, the transcript shows the stream and the outbox, the
// screen scrolls to an approval on request, and the focused screen gives the VoiceOver
// announcements and the error haptic of a failed turn.

import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { router } from "expo-router";
import {
  act,
  fireEvent,
  renderRouter,
  screen,
  type RenderRouterResult,
} from "expo-router/testing-library";
import { type ReactNode, type Ref, useImperativeHandle } from "react";
import {
  AccessibilityInfo,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
  ScrollView,
  View,
} from "react-native";

import { haptic } from "@/lib/native/haptics";
import { flush } from "@/lib/net/testing";
import type { Event } from "@/lib/protocol/wire";
import {
  approvalRequested,
  envelope,
  eventItem,
  promptQueued as queued,
  snapshotItem,
  turnCompleted,
  turnStarted,
} from "@/lib/stream/testing";
import {
  DESKTOP,
  LAPTOP,
  conversationId,
  setRecent,
  testServices,
  type TestServices,
} from "@/features/navigation/model/testing";

import { screenRoutes, socketOf, stream } from "./screen-testing";

const mockScrollToIndex = jest.fn<(params: unknown) => Promise<void>>(() => Promise.resolve());

interface MockListRef {
  readonly scrollToEnd: (params: unknown) => void;
  readonly scrollToIndex: (params: unknown) => Promise<void>;
}

interface MockFlashListProps {
  readonly data: readonly string[];
  readonly renderItem: (info: { item: string; index: number }) => ReactNode;
  readonly keyExtractor: (item: string) => string;
  readonly onScroll: (event: NativeSyntheticEvent<NativeScrollEvent>) => void;
  readonly onStartReached?: () => void;
  readonly ref?: Ref<MockListRef>;
}

/** FlashList is native: the fake one draws every row, oldest first. */
function MockFlashList({ ref, ...props }: MockFlashListProps) {
  useImperativeHandle(ref, () => ({
    scrollToEnd: () => undefined,
    scrollToIndex: mockScrollToIndex,
  }));
  return (
    // The test gets to the top (the start of the list) with a `scrollToTop`.
    <ScrollView testID="flash-list" onScroll={props.onScroll} onScrollToTop={props.onStartReached}>
      {props.data.map((item, index) => (
        <View key={props.keyExtractor(item)} testID={`cell-${item}`}>
          {props.renderItem({ item, index })}
        </View>
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
jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn(), setHapticsEnabled: jest.fn() }));

const CONVERSATION = conversationId(1);
const TURN = "019a9b1c-3d00-7a10-8b20-0000000000f1";
const NEXT = "019a9b1c-3d00-7a10-8b20-0000000000f2";
const CALL = "019a9b1c-3d00-7a10-8b20-0000000000f3";

let commands = 0;

/** A `prompt_queued` event with a command id that the decoder takes (a UUID). */
function promptQueued(turnId: string, text: string): Event {
  commands += 1;
  return {
    ...queued(turnId, text),
    command_id: `019a9b1c-3d00-7a10-8b20-${String(commands).padStart(12, "0")}`,
  };
}

let test: TestServices | undefined;
let view: RenderRouterResult | undefined;
const announce = jest.spyOn(AccessibilityInfo, "announceForAccessibilityWithOptions");

beforeEach(() => {
  seq = 3;
  mockScrollToIndex.mockClear();
  announce.mockClear();
  jest.mocked(haptic).mockClear();
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

/** The app with desktop and laptop paired and desktop connected, on `url`. */
async function open(url: string, connect = true): Promise<TestServices> {
  test = await testServices();
  const current = test;
  if (connect) {
    await current.connect(DESKTOP);
  }
  view = await renderRouter(screenRoutes(current), { initialUrl: "/" });
  await act(async () => {
    router.push(url);
  });
  await act(flush);
  return current;
}

/** The params of each `conversation.subscribe` that the app sent to desktop. */
function subscribes(): unknown[] {
  return socketOf(services(), DESKTOP)
    .requests("conversation.subscribe")
    .map((frame) => frame["params"]);
}

/** A snapshot of CONVERSATION with one finished turn, up to seq 3. */
function finished() {
  return snapshotItem(
    CONVERSATION,
    "idle",
    [
      envelope(1, promptQueued(TURN, "Why is the fan loud?")),
      envelope(2, turnStarted(TURN)),
      envelope(3, turnCompleted(TURN)),
    ],
    3,
  );
}

/** A snapshot of CONVERSATION with one finished turn and NEXT running, up to seq 5. */
function running() {
  return snapshotItem(
    CONVERSATION,
    "running",
    [
      envelope(1, promptQueued(TURN, "Why is the fan loud?")),
      envelope(2, turnStarted(TURN)),
      envelope(3, turnCompleted(TURN)),
      envelope(4, promptQueued(NEXT, "Check the logs.")),
      envelope(5, turnStarted(NEXT)),
    ],
    5,
  );
}

/** Types `text` in the composer and taps Send. */
async function typeAndSend(text: string): Promise<void> {
  await fireEvent.changeText(screen.getByTestId("composer-input"), text);
  await fireEvent.press(screen.getByRole("button", { name: "Send" }));
  await act(flush);
}

/** The prompts in the outbox, and the params of the last prompt.send on the socket. */
function sentPrompts() {
  return {
    outbox: services()
      .services.outbox.store.get()
      .entries.map((entry) => [entry.target, entry.text]),
    params: socketOf(services(), DESKTOP).requests("prompt.send").at(-1)?.["params"],
  };
}

/** What `sentPrompts` gives after one prompt `text` for CONVERSATION. */
function onePrompt(text: string) {
  return {
    outbox: [[{ kind: "conversation", conversationId: CONVERSATION }, text]],
    params: { conversation_id: CONVERSATION, text },
  };
}

/** The sequence number of the last event that efrd sent; the snapshot ends at 3. */
let seq = 3;

async function send(...events: Event[]): Promise<void> {
  await act(async () => {
    await stream(
      services(),
      DESKTOP,
      CONVERSATION,
      events.map((body) => eventItem(++seq, body)),
    );
  });
}

/** Lets the display frames run. renderRouter turns on the fake timers of jest. */
async function frames(): Promise<void> {
  await act(async () => {
    jest.advanceTimersByTime(50);
    await flush();
  });
}

describe("the conversation screen", () => {
  it("shows the loading line until the stream has events, then the transcript", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    expect(screen.getByLabelText("loading conversation from desktop")).toBeOnTheScreen();
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [finished()]);
    });
    expect(screen.getByLabelText("You: Why is the fan loud?")).toBeOnTheScreen();
    expect(screen.queryByLabelText("loading conversation from desktop")).toBeNull();
  });

  it("subscribes while it has the focus, and again with after_seq when it gets it back", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    expect(subscribes()).toEqual([{ conversation_id: CONVERSATION, drafts: true }]);
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [finished()]);
    });
    const socket = socketOf(services(), DESKTOP);
    const first = socket.lastId("conversation.subscribe");
    await act(async () => {
      router.push("/settings");
    });
    await act(flush);
    expect(socket.sent.some((frame) => frame["cancel"] === first)).toBe(true);
    await act(async () => {
      router.back();
    });
    await act(flush);
    expect(subscribes().at(-1)).toEqual({
      conversation_id: CONVERSATION,
      drafts: true,
      after_seq: 3,
    });
    // The stream stayed in memory, so the transcript shows at once.
    expect(screen.getByLabelText("You: Why is the fan loud?")).toBeOnTheScreen();
  });

  it("announces a question at once and the end of a turn in the queue", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [finished()]);
    });
    expect(announce).not.toHaveBeenCalled();
    await send(
      promptQueued(NEXT, "Push it."),
      turnStarted(NEXT),
      approvalRequested(NEXT, CALL, "git push"),
    );
    expect(announce).toHaveBeenLastCalledWith("Approval needed. git push", { queue: false });
    await send(turnCompleted(NEXT));
    expect(announce).toHaveBeenLastCalledWith("Reply done.", { queue: true });
    // The renders of the screen kept the one subscription.
    expect(subscribes()).toHaveLength(1);
  });

  it("plays the error haptic once when a turn fails on screen", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [finished()]);
    });
    await send(promptQueued(NEXT, "Push it."), turnStarted(NEXT), {
      kind: "turn_failed",
      turn_id: NEXT,
      error: { code: "internal", message: "model error: overloaded" },
    });
    expect(jest.mocked(haptic).mock.calls.filter(([kind]) => kind === "error")).toHaveLength(1);
    expect(screen.getByText("model error: overloaded")).toBeOnTheScreen();
  });

  it("sends the prompt of a failed turn again as a new prompt on retry", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [finished()]);
    });
    await send(promptQueued(NEXT, "Push it."), turnStarted(NEXT), {
      kind: "turn_failed",
      turn_id: NEXT,
      error: { code: "internal", message: "model error: overloaded" },
    });
    await fireEvent.press(screen.getByRole("button", { name: "Retry the turn" }));
    await act(flush);
    const sent = socketOf(services(), DESKTOP).requests("prompt.send").at(-1);
    expect(sent?.["params"]).toMatchObject({ conversation_id: CONVERSATION, text: "Push it." });
    expect(screen.getByText("sending")).toBeOnTheScreen();
  });

  it("scrolls to the approval that waits, then clears the request", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}?focus=approval`);
    expect(view?.getSearchParams()).toMatchObject({ focus: "approval" });
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [
        snapshotItem(
          CONVERSATION,
          "awaiting_approval",
          [
            envelope(1, promptQueued(NEXT, "Push it.")),
            envelope(2, turnStarted(NEXT)),
            envelope(3, approvalRequested(NEXT, CALL, "git push")),
          ],
          3,
        ),
      ]);
    });
    await frames();
    expect(mockScrollToIndex).toHaveBeenCalledTimes(1);
    const cells = screen.getAllByTestId(/^cell-/).map((cell) => String(cell.props.testID));
    expect(mockScrollToIndex).toHaveBeenCalledWith({
      index: cells.indexOf(`cell-a:${NEXT}:${CALL}`),
      animated: true,
      viewPosition: 0.5,
    });
    expect(cells.indexOf(`cell-a:${NEXT}:${CALL}`)).toBeGreaterThanOrEqual(0);
    expect(view?.getSearchParams()).not.toHaveProperty("focus");
  });

  it("fetches the older history when the reader gets to the top", async () => {
    const OLDER = "019a9b1c-3d00-7a10-8b20-0000000000f4";
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    const window = snapshotItem(
      CONVERSATION,
      "idle",
      [envelope(4, promptQueued(TURN, "Why is the fan loud?")), envelope(5, turnStarted(TURN))],
      5,
    );
    if (window.kind !== "snapshot") {
      throw new Error("not a snapshot");
    }
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [{ ...window, history_cursor: "before-4" }]);
    });
    const socket = socketOf(services(), DESKTOP);
    expect(socket.requests("conversation.history")).toHaveLength(0);
    await act(async () => {
      await fireEvent(screen.getByTestId("flash-list"), "scrollToTop");
      await flush();
    });
    expect(socket.requests("conversation.history").map((frame) => frame["params"])).toEqual([
      { conversation_id: CONVERSATION, cursor: "before-4" },
    ]);
    await act(async () => {
      socket.answer("conversation.history", {
        events: [
          envelope(1, promptQueued(OLDER, "What changed?")),
          envelope(2, turnStarted(OLDER)),
          envelope(3, turnCompleted(OLDER)),
        ],
      });
      await flush();
    });
    await frames();
    const cells = screen.getAllByTestId(/^cell-/).map((cell) => String(cell.props.testID));
    // The fake list draws the oldest row first: the older turn comes before.
    expect(cells.indexOf(`cell-p:${OLDER}`)).toBeLessThan(cells.indexOf(`cell-p:${TURN}`));
    expect(screen.getByLabelText("You: What changed?")).toBeOnTheScreen();
    // The page reached the first event: the top asks for nothing more.
    await act(async () => {
      await fireEvent(screen.getByTestId("flash-list"), "scrollToTop");
      await flush();
    });
    expect(socket.requests("conversation.history")).toHaveLength(1);
  });

  it("pages back from the newest event when the stream saw no snapshot", async () => {
    const OLDER = "019a9b1c-3d00-7a10-8b20-0000000000f4";
    const held = [
      envelope(4, promptQueued(TURN, "Why is the fan loud?")),
      envelope(5, turnStarted(TURN)),
    ];
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    // A resume: events, and no snapshot with a cursor.
    await act(async () => {
      await stream(
        services(),
        DESKTOP,
        CONVERSATION,
        held.map((item) => eventItem(item.seq, item.event)),
      );
    });
    const socket = socketOf(services(), DESKTOP);
    await act(async () => {
      await fireEvent(screen.getByTestId("flash-list"), "scrollToTop");
      await flush();
    });
    await act(async () => {
      // The newest page holds only what the stream has: the next page follows at once.
      socket.answer("conversation.history", { events: held, next_cursor: "before-4" });
      await flush();
    });
    expect(socket.requests("conversation.history").map((frame) => frame["params"])).toEqual([
      { conversation_id: CONVERSATION, cursor: null },
      { conversation_id: CONVERSATION, cursor: "before-4" },
    ]);
    await act(async () => {
      socket.answer("conversation.history", {
        events: [
          envelope(1, promptQueued(OLDER, "What changed?")),
          envelope(2, turnStarted(OLDER)),
        ],
      });
      await flush();
    });
    await frames();
    expect(screen.getByLabelText("You: What changed?")).toBeOnTheScreen();
    expect(screen.getByLabelText("You: Why is the fan loud?")).toBeOnTheScreen();
  });

  it("keeps the transcript and says why it cannot send when the machine is offline", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [finished()]);
    });
    await act(async () => {
      socketOf(services(), DESKTOP).drop();
      await flush();
    });
    expect(screen.getByLabelText("You: Why is the fan loud?")).toBeOnTheScreen();
    const input = screen.getByTestId("composer-input");
    expect(input.props.placeholder).toBe("desktop is offline");
    expect(input.props.editable).toBe(false);
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("sends a follow-up prompt with Send in a conversation whose last turn ended", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [finished()]);
    });
    await typeAndSend("And the disk?");
    expect(sentPrompts()).toMatchObject(onePrompt("And the disk?"));
    expect(screen.getByTestId("composer-input").props.value).toBe("And the disk?");
    expect(screen.getByRole("button", { name: "Sending" })).toBeOnTheScreen();
  });

  it("sends a follow-up in a conversation of a terminal, opened from the recent list", async () => {
    test = await testServices();
    const current = test;
    await current.connect(DESKTOP);
    setRecent(current.services.queryClient, DESKTOP, [{ id: CONVERSATION, title: "fan" }]);
    view = await renderRouter(screenRoutes(current), { initialUrl: "/" });
    await act(async () => {
      router.push(`/c/${DESKTOP}/${CONVERSATION}`);
    });
    await act(flush);
    await act(async () => {
      await stream(services(), DESKTOP, CONVERSATION, [
        snapshotItem(
          CONVERSATION,
          "idle",
          [
            envelope(1, {
              kind: "conversation_created",
              origin: "shell",
              route: { kind: "outside" },
              tty: "/dev/pts/3",
            }),
            envelope(2, {
              kind: "terminal_attached",
              how: "picked_up",
              route: { kind: "outside" },
              tty: "/dev/pts/3",
            }),
            envelope(3, promptQueued(TURN, "Why is the fan loud?")),
            envelope(4, turnStarted(TURN)),
            envelope(5, turnCompleted(TURN)),
          ],
          5,
        ),
      ]);
    });
    await typeAndSend("And the disk?");
    expect(sentPrompts()).toMatchObject(onePrompt("And the disk?"));
  });

  it("queues a prompt from the capsule while a turn runs", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    await act(async () => {
      services().services.preferences.setSendMode("queue");
      await stream(services(), DESKTOP, CONVERSATION, [running()]);
    });
    await fireEvent.changeText(screen.getByTestId("composer-input"), "Then the tests.");
    await fireEvent.press(screen.getByRole("button", { name: "Queue, more options" }));
    await act(flush);
    expect(sentPrompts()).toMatchObject(onePrompt("Then the tests."));
  });

  it("steers the running turn from the capsule", async () => {
    await open(`/c/${DESKTOP}/${CONVERSATION}`);
    await act(async () => {
      services().services.preferences.setSendMode("steer");
      await stream(services(), DESKTOP, CONVERSATION, [running()]);
    });
    await fireEvent.changeText(screen.getByTestId("composer-input"), "Use the tests.");
    await fireEvent.press(screen.getByRole("button", { name: "Steer, more options" }));
    await frames();
    expect(socketOf(services(), DESKTOP).requests("turn.steer").at(-1)?.["params"]).toMatchObject({
      conversation_id: CONVERSATION,
      text: "Use the tests.",
      turn_id: NEXT,
    });
  });

  it("shows a quiet line for a machine that is not paired, and for a bad link", async () => {
    await open(`/c/019a9b1c-3d00-7a10-8b20-0000000000dd/${CONVERSATION}`);
    expect(screen.getByText("This machine is not paired with this iPhone.")).toBeOnTheScreen();
    await act(async () => {
      router.push(`/c/${LAPTOP}/not-a-conversation`);
    });
    expect(screen.getByText("This link names no conversation.")).toBeOnTheScreen();
  });
});
