import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, render, screen } from "@testing-library/react-native";
import { AccessibilityInfo, Text } from "react-native";

import { reduceMotion, useReduceMotion } from "./reduce-motion";

function Probe() {
  return <Text>{useReduceMotion() ? "reduced" : "full"}</Text>;
}

const remove = jest.fn<() => void>();

/** Sets the first answer of iOS, and catches the listener of the store. */
function ios(on: boolean) {
  jest.spyOn(AccessibilityInfo, "isReduceMotionEnabled").mockResolvedValue(on);
  // The preset of React Native already mocks AccessibilityInfo, so the spy is that mock:
  // clear the calls of the tests before.
  const spy = jest.spyOn(AccessibilityInfo, "addEventListener").mockReturnValue({ remove });
  spy.mockClear();
  return spy;
}

let listen: ReturnType<typeof ios>;

/** iOS reports that Reduce Motion is now `on`. */
async function change(on: boolean): Promise<void> {
  const handler = listen.mock.calls[0]?.[1];
  await act(async () => {
    handler?.(on);
  });
}

async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  remove.mockClear();
  listen = ios(false);
});

afterEach(() => {
  jest.restoreAllMocks();
});

describe("useReduceMotion", () => {
  it("takes the first value from iOS", async () => {
    listen = ios(true);
    await render(<Probe />);
    await settle();
    expect(screen.getByText("reduced")).toBeOnTheScreen();
    expect(reduceMotion()).toBe(true);
  });

  it("renders again when the setting changes", async () => {
    await render(<Probe />);
    await settle();
    expect(screen.getByText("full")).toBeOnTheScreen();
    expect(listen).toHaveBeenCalledWith("reduceMotionChanged", expect.any(Function));
    await change(true);
    expect(screen.getByText("reduced")).toBeOnTheScreen();
    await change(false);
    expect(screen.getByText("full")).toBeOnTheScreen();
    expect(reduceMotion()).toBe(false);
  });

  it("listens once for many readers, and stops when the last one goes", async () => {
    const { unmount } = await render(
      <>
        <Probe />
        <Probe />
      </>,
    );
    await settle();
    expect(listen).toHaveBeenCalledTimes(1);
    expect(remove).not.toHaveBeenCalled();
    await unmount();
    expect(remove).toHaveBeenCalledTimes(1);
  });
});
