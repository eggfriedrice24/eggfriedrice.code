import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, fireEvent, render, screen, within } from "@testing-library/react-native";
import {
  type ComponentType,
  createRef,
  type ReactNode,
  type Ref,
  useImperativeHandle,
} from "react";
import { type NativeScrollEvent, type NativeSyntheticEvent, ScrollView, View } from "react-native";

import { haptic } from "@/lib/native/haptics";
import { EMPTY_STREAM } from "@/lib/stream/reducer";
import {
  approvalRequested,
  eventItem,
  messageCompleted,
  promptQueued,
  run,
  toolCallCompleted,
  toolCallStarted,
  turnCompleted,
  turnStarted,
} from "@/lib/stream/testing";

import { Transcript, transcriptItems } from "./transcript";
import type { TranscriptListHandle } from "./transcript-list";
import { testScope } from "./transcript-testing";

const mockScrollToEnd = jest.fn<(params: unknown) => void>();
const mockScrollToIndex = jest.fn<(params: unknown) => Promise<void>>(() => Promise.resolve());

/** The scroll methods of FlashList that the transcript calls. */
interface MockListRef {
  readonly scrollToEnd: (params: unknown) => void;
  readonly scrollToIndex: (params: unknown) => Promise<void>;
}

/** The props of FlashList that the fake list reads. */
interface MockFlashListProps {
  readonly data: readonly string[];
  readonly renderItem: (info: { item: string; index: number }) => ReactNode;
  readonly keyExtractor: (item: string) => string;
  readonly onScroll: (event: NativeSyntheticEvent<NativeScrollEvent>) => void;
  readonly ItemSeparatorComponent: ComponentType<{ leadingItem: string; trailingItem: string }>;
  readonly inverted?: boolean;
  readonly maintainVisibleContentPosition?: unknown;
  readonly onStartReached?: () => void;
  readonly ref?: Ref<MockListRef>;
}

/**
 * The list libraries are native. The fake list draws every row in the order it gets
 * them, oldest first, with the separator after each row but the last, as FlashList
 * does. It exposes the scroll handler, the scroll methods and the chat props.
 */
function MockFlashList({ ref, ...props }: MockFlashListProps) {
  useImperativeHandle(ref, () => ({
    scrollToEnd: mockScrollToEnd,
    scrollToIndex: mockScrollToIndex,
  }));
  return (
    <ScrollView
      testID="flash-list"
      onScroll={props.onScroll}
      onScrollToTop={props.onStartReached}
      accessibilityHint={JSON.stringify({
        inverted: props.inverted ?? false,
        position: props.maintainVisibleContentPosition,
      })}
    >
      {props.data.map((item, index) => {
        const next = props.data[index + 1];
        const Separator = props.ItemSeparatorComponent;
        return (
          <View key={props.keyExtractor(item)} testID={`cell-${item}`}>
            {props.renderItem({ item, index })}
            {next === undefined ? null : <Separator leadingItem={item} trailingItem={next} />}
          </View>
        );
      })}
    </ScrollView>
  );
}

jest.mock("@shopify/flash-list", () => ({ FlashList: MockFlashList }));
jest.mock("react-native-keyboard-controller", () => ({ KeyboardChatScrollView: () => null }));
jest.mock("./fade-in", () => ({
  FadeIn: (props: { children: unknown }) => props.children,
}));
jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";
const NEXT = "019a9b1c-3d00-7a10-8b20-000000000003";
const CALL = "019a9b1c-3d00-7a10-8b20-000000000004";
const ASK = "019a9b1c-3d00-7a10-8b20-000000000005";
const INSETS = { top: 106, bottom: 124 };

function finishedTurn() {
  return run([
    eventItem(1, promptQueued(TURN, "Fix the steer race.")),
    eventItem(2, turnStarted(TURN)),
    eventItem(3, toolCallStarted(TURN, CALL, "shell", { command: "cargo test" })),
    eventItem(4, toolCallCompleted(TURN, CALL, "ok")),
    eventItem(5, messageCompleted(TURN, 0, "Found it.")),
    eventItem(6, turnCompleted(TURN)),
  ]);
}

/** The height of the content of the fake list. */
const CONTENT = 5_000;

/** Scrolls the fake list to `distance` from the end, in a screen of 800 pt. */
async function scrollTo(distance: number): Promise<void> {
  await fireEvent.scroll(screen.getByTestId("flash-list"), {
    nativeEvent: {
      contentOffset: { y: CONTENT - 800 - distance, x: 0 },
      contentSize: { height: CONTENT, width: 402 },
      layoutMeasurement: { height: 800, width: 402 },
    },
  });
}

/** The glass of the jump button. */
function jumpGlass() {
  return within(screen.getByTestId("jump-to-latest")).getByTestId("material-glass", {
    includeHiddenElements: true,
  });
}

beforeEach(() => {
  jest.useFakeTimers();
  mockScrollToEnd.mockClear();
  mockScrollToIndex.mockClear();
  jest.mocked(haptic).mockClear();
});

afterEach(() => {
  jest.useRealTimers();
});

describe("the transcript", () => {
  it("draws each row of the stream, oldest first in a list that is not inverted", async () => {
    const { scope } = testScope(finishedTurn());
    await render(<Transcript scope={scope} insets={INSETS} />);
    expect(screen.getByLabelText("You: Fix the steer race.")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: /Command cargo test, done/ })).toBeOnTheScreen();
    expect(screen.getByLabelText(/^Turn done/)).toBeOnTheScreen();
    const cells = screen.getAllByTestId(/^cell-/).map((cell) => String(cell.props.testID));
    expect(cells).toEqual([
      `cell-p:${TURN}`,
      `cell-t:${TURN}:${CALL}`,
      `cell-m:${TURN}:0`,
      `cell-e:${TURN}`,
    ]);
    // The fake list gives its chat props in its hint.
    expect(JSON.parse(String(screen.getByTestId("flash-list").props.accessibilityHint))).toEqual({
      inverted: false,
      position: { startRenderingFromBottom: true, autoscrollToBottomThreshold: 200 },
    });
  });

  it("asks for older history when the reader gets near the start", async () => {
    const { scope } = testScope(finishedTurn());
    const onReachTop = jest.fn<() => void>();
    await render(<Transcript scope={scope} insets={INSETS} onReachTop={onReachTop} />);
    await fireEvent(screen.getByTestId("flash-list"), "scrollToTop");
    expect(onReachTop).toHaveBeenCalledTimes(1);
  });

  it("puts 16 pt under your prompt and 12 pt between the other rows", async () => {
    const { scope } = testScope(finishedTurn());
    await render(<Transcript scope={scope} insets={INSETS} />);
    const gaps = screen
      .getAllByTestId("row-gap", { includeHiddenElements: true })
      .map((gap) => String(gap.props.className));
    // Oldest first: prompt over tool, tool over message, message over end.
    expect(gaps).toEqual(["h-after-prompt", "h-item", "h-item"]);
  });

  it("shows the whole output of a tool after a tap on show all", async () => {
    const lines = Array.from({ length: 8 }, (_, i) => `line ${String(i + 1)}`).join("\n");
    const { scope } = testScope(
      run([
        eventItem(1, promptQueued(TURN, "Run the tests.")),
        eventItem(2, turnStarted(TURN)),
        eventItem(3, toolCallStarted(TURN, CALL, "shell", { command: "cargo test" })),
        eventItem(4, toolCallCompleted(TURN, CALL, lines)),
        eventItem(5, turnCompleted(TURN)),
      ]),
    );
    await render(<Transcript scope={scope} insets={INSETS} />);
    expect(screen.queryByText("line 1")).toBeNull();
    expect(screen.getByText("line 8")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "show all 8 lines" }));
    expect(screen.getByText("line 1")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "collapse" })).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "collapse" }));
    expect(screen.queryByText("line 1")).toBeNull();
  });

  it("shows the loading line until the stream has events", async () => {
    const { scope } = testScope(EMPTY_STREAM);
    await render(<Transcript scope={scope} insets={INSETS} />);
    expect(screen.getByLabelText("loading conversation from desktop")).toBeOnTheScreen();
  });

  it("puts the prompts of the outbox after the rows of the stream", async () => {
    const { scope } = testScope(finishedTurn());
    await render(
      <Transcript
        scope={scope}
        insets={INSETS}
        pending={[{ key: "cmd-9", text: "Push it.", failed: false }]}
      />,
    );
    expect(screen.getByText("sending")).toBeOnTheScreen();
    expect(screen.getAllByTestId(/^cell-/).at(-1)?.props.testID).toBe("cell-o:cmd-9");
  });

  it("draws an approval with the view that the screen gives", async () => {
    const state = run(
      [
        eventItem(7, promptQueued(NEXT, "Push it.")),
        eventItem(8, turnStarted(NEXT)),
        eventItem(9, approvalRequested(NEXT, ASK, "git push")),
      ],
      finishedTurn(),
    );
    const { scope } = testScope(state);
    await render(<Transcript scope={scope} insets={INSETS} />);
    expect(screen.getByText(`approval ${ASK}`)).toBeOnTheScreen();
  });
});

describe("jump to latest", () => {
  it("shows when the reader left the end and new content came", async () => {
    const { scope, store } = testScope(finishedTurn());
    await render(<Transcript scope={scope} insets={INSETS} />);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
    await scrollTo(2_000);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
    await act(async () => {
      store.setState((state) => run([eventItem(7, promptQueued(NEXT, "More."))], state));
    });
    const jump = screen.getByRole("button", { name: "Jump to latest" });
    await fireEvent.press(jump);
    expect(mockScrollToEnd).toHaveBeenCalledWith({ animated: true });
    expect(haptic).toHaveBeenCalledWith("light");
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  });

  it("shows and hides its Liquid Glass with the system animation, not a fade", async () => {
    const { scope, store } = testScope(finishedTurn());
    await render(<Transcript scope={scope} insets={INSETS} />);
    expect(jumpGlass().props.glassEffectStyle).toEqual({ style: "none", animate: true });
    await scrollTo(2_000);
    await act(async () => {
      store.setState((state) => run([eventItem(7, promptQueued(NEXT, "More."))], state));
    });
    expect(jumpGlass().props.glassEffectStyle).toEqual({ style: "regular", animate: true });
    expect(screen.getByRole("button", { name: "Jump to latest" }).props.pointerEvents).toBe("auto");
  });

  it("hides when the reader is back within a screen of the end", async () => {
    const { scope, store } = testScope(finishedTurn());
    await render(<Transcript scope={scope} insets={INSETS} />);
    await scrollTo(2_000);
    await act(async () => {
      store.setState((state) => run([eventItem(7, promptQueued(NEXT, "More."))], state));
    });
    expect(screen.getByRole("button", { name: "Jump to latest" })).toBeOnTheScreen();
    await scrollTo(100);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  });
});

describe("the handle of the transcript", () => {
  it("scrolls to a row by its key, at its index from the oldest row", async () => {
    const { scope } = testScope(finishedTurn());
    const ref = createRef<TranscriptListHandle>();
    await render(<Transcript scope={scope} insets={INSETS} ref={ref} />);
    await act(async () => {
      ref.current?.scrollToKey(`p:${TURN}`, false);
    });
    expect(mockScrollToIndex).toHaveBeenCalledWith({
      index: 0,
      animated: false,
      viewPosition: 0.5,
    });
    await act(async () => {
      ref.current?.scrollToEnd(true);
    });
    expect(mockScrollToEnd).toHaveBeenCalledWith({ animated: true });
  });
});

describe("transcriptItems", () => {
  it("keys the prompts of the outbox apart from the rows", () => {
    expect(transcriptItems([], [{ key: "a", text: "x", failed: true }])).toEqual([
      { kind: "pending", key: "o:a", prompt: { key: "a", text: "x", failed: true } },
    ]);
  });
});
