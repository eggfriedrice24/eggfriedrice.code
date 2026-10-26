import { beforeEach, describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import { run, eventItem, promptQueued, turnStarted, at } from "@/lib/stream/testing";

import { InScope, testScope } from "./transcript-testing";
import { TurnEndLineView, TurnEndRow } from "./turn-end-line";
import { turnEndView } from "../model/turn-end";

jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";
const START = new Date(2026, 9, 26, 14, 29, 24).getTime();

beforeEach(() => {
  jest.mocked(copyText).mockClear();
  jest.mocked(haptic).mockClear();
});

describe("the turn end line", () => {
  it("shows the time, the duration and the tokens on one plain line", async () => {
    const view = turnEndView(
      {
        outcome: "completed",
        at: START + 188_000,
        context: undefined,
        error: undefined,
        stop: undefined,
        usage: { input_tokens: 15_000, output_tokens: 3_400 },
      },
      START,
      undefined,
      "desktop",
    );
    await render(<TurnEndLineView view={view} onRetry={undefined} />);
    expect(screen.getByText("14:32 · 3m 08s · 18k tokens")).toBeOnTheScreen();
    expect(screen.queryAllByTestId("hairline", { includeHiddenElements: true })).toHaveLength(0);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("marks an interrupt with ■", async () => {
    const view = turnEndView(
      {
        outcome: "interrupted",
        at: START + 62_000,
        context: undefined,
        error: undefined,
        stop: undefined,
        usage: undefined,
      },
      START,
      "phone",
      "desktop",
    );
    await render(<TurnEndLineView view={view} onRetry={undefined} />);
    expect(screen.getByText("■ ").props.className).toContain("text-text-muted");
    expect(screen.getByLabelText("Turn interrupted by you · 14:30 · 1m 02s")).toBeOnTheScreen();
  });

  it("gives a failed turn its reason, retry and copy error", async () => {
    const onRetry = jest.fn<() => void>();
    const view = turnEndView(
      {
        outcome: "failed",
        at: START + 22_000,
        context: undefined,
        error: "model error: overloaded, the provider asked to retry later",
        stop: undefined,
        usage: undefined,
      },
      START,
      undefined,
      "desktop",
    );
    await render(<TurnEndLineView view={view} onRetry={onRetry} />);
    expect(screen.getByText("✗ ").props.className).toContain("text-state-error");
    expect(screen.getByText("turn failed ")).toBeOnTheScreen();
    expect(
      screen.getByText("model error: overloaded, the provider asked to retry later"),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Retry the turn" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
    await fireEvent.press(screen.getByRole("button", { name: "copy error" }));
    expect(copyText).toHaveBeenCalledWith(
      "model error: overloaded, the provider asked to retry later",
    );
    expect(haptic).toHaveBeenCalledWith("light");
  });
});

describe("the turn end row", () => {
  it("reads its turn and sends the prompt again on retry", async () => {
    const state = run([
      eventItem(1, promptQueued(TURN, "Upgrade to Expo SDK 58.")),
      eventItem(2, turnStarted(TURN), at(0)),
      eventItem(
        3,
        { kind: "turn_failed", turn_id: TURN, error: { code: "internal", message: "model error" } },
        at(22),
      ),
    ]);
    const { scope, calls } = testScope(state);
    await render(
      <InScope scope={scope}>
        <TurnEndRow turnId={TURN} />
      </InScope>,
    );
    expect(screen.getByText("model error")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Retry the turn" }));
    expect(calls).toEqual([{ action: "retry", turnId: TURN, text: "Upgrade to Expo SDK 58." }]);
  });

  it("shows nothing before the turn ends", async () => {
    const { scope } = testScope(run([eventItem(1, promptQueued(TURN, "Hi"))]));
    await render(
      <InScope scope={scope}>
        <TurnEndRow turnId={TURN} />
      </InScope>,
    );
    expect(screen.toJSON()).toBeNull();
  });
});
