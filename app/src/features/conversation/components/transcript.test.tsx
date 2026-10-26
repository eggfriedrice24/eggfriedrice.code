import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { createRef, type ReactNode, type Ref, useImperativeHandle } from "react";
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

const mockScrollToOffset = jest.fn<(params: unknown) => void>();
const mockScrollToIndex = jest.fn<(params: unknown) => Promise<void>>(() => Promise.resolve());

/** The scroll methods of FlashList that the transcript calls. */
interface MockListRef {
  readonly scrollToOffset: (params: unknown) => void;
  readonly scrollToIndex: (params: unknown) => Promise<void>;
}

/** The props of FlashList that the fake list reads. */
interface MockFlashListProps {
  readonly data: readonly string[];
  readonly renderItem: (info: { item: string; index: number }) => ReactNode;
  readonly keyExtractor: (item: string) => string;
  readonly onScroll: (event: NativeSyntheticEvent<NativeScrollEvent>) => void;
  readonly ref?: Ref<MockListRef>;
}

/**
 * The list libraries are native. The fake list draws every row in the order it gets
 * them, newest first, and exposes the scroll handler and the scroll methods.
 */
function MockFlashList({ ref, ...props }: MockFlashListProps) {
  useImperativeHandle(ref, () => ({
    scrollToOffset: mockScrollToOffset,
    scrollToIndex: mockScrollToIndex,
  }));
  return (
    <ScrollView testID="flash-list" onScroll={props.onScroll}>
      {props.data.map((item, index) => (
        <View key={props.keyExtractor(item)} testID={`cell-${item}`}>
          {props.renderItem({ item, index })}
        </View>
      ))}
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

/** Scrolls the fake list to `offset` from the end, in a screen of 800 pt. */
async function scrollTo(offset: number): Promise<void> {
  await fireEvent.scroll(screen.getByTestId("flash-list"), {
    nativeEvent: {
      contentOffset: { y: offset, x: 0 },
      layoutMeasurement: { height: 800, width: 402 },
    },
  });
}

beforeEach(() => {
  jest.useFakeTimers();
  mockScrollToOffset.mockClear();
  mockScrollToIndex.mockClear();
  jest.mocked(haptic).mockClear();
});

afterEach(() => {
  jest.useRealTimers();
});

describe("the transcript", () => {
  it("draws each row of the stream, newest first in the inverted list", async () => {
    const { scope } = testScope(finishedTurn());
    await render(<Transcript scope={scope} insets={INSETS} />);
    expect(screen.getByLabelText("You: Fix the steer race.")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: /Command cargo test, done/ })).toBeOnTheScreen();
    expect(screen.getByLabelText(/^Turn done/)).toBeOnTheScreen();
    const cells = screen.getAllByTestId(/^cell-/).map((cell) => String(cell.props.testID));
    expect(cells).toEqual([
      `cell-e:${TURN}`,
      `cell-m:${TURN}:0`,
      `cell-t:${TURN}:${CALL}`,
      `cell-p:${TURN}`,
    ]);
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
    expect(screen.getAllByTestId(/^cell-/)[0]?.props.testID).toBe("cell-o:cmd-9");
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
    expect(mockScrollToOffset).toHaveBeenCalledWith({ offset: 0, animated: true });
    expect(haptic).toHaveBeenCalledWith("light");
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
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
  it("scrolls to a row by its key, in the reversed list", async () => {
    const { scope } = testScope(finishedTurn());
    const ref = createRef<TranscriptListHandle>();
    await render(<Transcript scope={scope} insets={INSETS} ref={ref} />);
    await act(async () => {
      ref.current?.scrollToKey(`p:${TURN}`, false);
    });
    expect(mockScrollToIndex).toHaveBeenCalledWith({
      index: 3,
      animated: false,
      viewPosition: 0.5,
    });
    await act(async () => {
      ref.current?.scrollToEnd(true);
    });
    expect(mockScrollToOffset).toHaveBeenCalledWith({ offset: 0, animated: true });
  });
});

describe("transcriptItems", () => {
  it("keys the prompts of the outbox apart from the rows", () => {
    expect(transcriptItems([], [{ key: "a", text: "x", failed: true }])).toEqual([
      { kind: "pending", key: "o:a", prompt: { key: "a", text: "x", failed: true } },
    ]);
  });
});
