import { beforeEach, describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import { eventItem, promptQueued, run, turnStarted } from "@/lib/stream/testing";

import { InScope, testScope } from "./transcript-testing";
import { PendingPromptRow, PromptRow, YourMessageView } from "./your-message";

jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const HIDDEN = { includeHiddenElements: true } as const;
const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";
const OTHER = "019a9b1c-3d00-7a10-8b20-000000000003";

beforeEach(() => {
  jest.mocked(copyText).mockClear();
  jest.mocked(haptic).mockClear();
});

describe("your message", () => {
  it("shows the yolk prompt glyph and the text in bright", async () => {
    await render(
      <YourMessageView
        text="Run the tests again."
        state={{ kind: "sent" }}
        onEditAndResend={jest.fn()}
      />,
    );
    expect(screen.getByText("❯", HIDDEN).props.className).toContain("text-accent-primary");
    expect(screen.getByText("Run the tests again.").props.className).toContain("text-text-bright");
    expect(screen.getByLabelText("You: Run the tests again.")).toBeOnTheScreen();
  });

  it("puts each paragraph in its own text", async () => {
    await render(
      <YourMessageView
        text={"Two things before you push.\n\nFirst, keep the reader."}
        state={{ kind: "sent" }}
        onEditAndResend={jest.fn()}
      />,
    );
    expect(screen.getByText("Two things before you push.")).toBeOnTheScreen();
    expect(screen.getByText("First, keep the reader.")).toBeOnTheScreen();
  });

  it("tags a steer with its time", async () => {
    const at = new Date(2026, 9, 26, 14, 20).getTime();
    await render(
      <YourMessageView
        text="Use TurnState, not a new enum."
        state={{ kind: "steered", at }}
        onEditAndResend={jest.fn()}
      />,
    );
    expect(screen.getByText("steered · 14:20")).toBeOnTheScreen();
  });

  it("dims a queued message and offers withdraw", async () => {
    const onWithdraw = jest.fn<() => void>();
    await render(
      <YourMessageView
        text="Also add a regression test."
        state={{ kind: "queued", onWithdraw }}
        onEditAndResend={jest.fn()}
      />,
    );
    expect(screen.getByText("❯", HIDDEN).props.className).toContain("text-text-line-number");
    expect(screen.getByText("Also add a regression test.").props.className).toContain(
      "text-text-muted",
    );
    expect(screen.getByText("queued · sends when this turn ends ·")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Withdraw the queued message" }));
    expect(onWithdraw).toHaveBeenCalledTimes(1);
  });

  it("shows a prompt on its way, and one that did not go", async () => {
    const onResend = jest.fn<() => void>();
    await render(
      <YourMessageView text="Hi" state={{ kind: "sending" }} onEditAndResend={jest.fn()} />,
    );
    expect(screen.getByText("sending")).toBeOnTheScreen();
    await render(
      <YourMessageView
        text="Hi"
        state={{ kind: "failed", onResend }}
        onEditAndResend={jest.fn()}
      />,
    );
    await fireEvent.press(screen.getByRole("button", { name: "Send it again" }));
    expect(onResend).toHaveBeenCalledTimes(1);
  });

  it("opens Copy and Edit and resend on a long press", async () => {
    const onEditAndResend = jest.fn<(text: string) => void>();
    await render(
      <YourMessageView text="Fix it." state={{ kind: "sent" }} onEditAndResend={onEditAndResend} />,
    );
    const menu = screen.getByTestId("your-message-menu");
    expect(menu.props.shouldOpenOnLongPress).toBe(true);
    await fireEvent(menu, "pressAction", { nativeEvent: { event: "copy" } });
    expect(copyText).toHaveBeenCalledWith("Fix it.");
    expect(haptic).toHaveBeenCalledWith("light");
    await fireEvent(menu, "pressAction", { nativeEvent: { event: "edit" } });
    expect(onEditAndResend).toHaveBeenCalledWith("Fix it.");
  });
});

describe("the prompt row", () => {
  it("reads the prompt of its turn, and withdraws it while it waits", async () => {
    const state = run([
      eventItem(1, promptQueued(OTHER, "Find the race.")),
      eventItem(2, turnStarted(OTHER)),
      eventItem(3, promptQueued(TURN, "Also add a test.")),
    ]);
    const { scope, calls } = testScope(state);
    await render(
      <InScope scope={scope}>
        <PromptRow turnId={TURN} />
      </InScope>,
    );
    await fireEvent.press(screen.getByRole("button", { name: "Withdraw the queued message" }));
    expect(calls).toEqual([{ action: "withdraw", turnId: TURN }]);
  });

  it("shows a running prompt without a tag", async () => {
    const state = run([eventItem(1, promptQueued(TURN, "Go.")), eventItem(2, turnStarted(TURN))]);
    const { scope } = testScope(state);
    await render(
      <InScope scope={scope}>
        <PromptRow turnId={TURN} />
      </InScope>,
    );
    expect(screen.getByLabelText("You: Go.")).toBeOnTheScreen();
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("the pending prompt row", () => {
  it("sends a failed prompt again by its key", async () => {
    const { scope, calls } = testScope(run([]));
    await render(
      <InScope scope={scope}>
        <PendingPromptRow prompt={{ key: "cmd-1", text: "Hi", failed: true }} />
      </InScope>,
    );
    await fireEvent.press(screen.getByRole("button", { name: "Send it again" }));
    expect(calls).toEqual([{ action: "resend", key: "cmd-1" }]);
  });
});
