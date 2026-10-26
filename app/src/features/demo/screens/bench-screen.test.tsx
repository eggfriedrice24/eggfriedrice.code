// The decode bench screen: the facts of the engine, a run in each mode, and the report.
// The test runs one round of one small recording, not the plan's 100 rounds of all of
// them, on the fake timers of jest: the run takes no wall clock time to wait for.

import { describe, expect, it, jest } from "@jest/globals";
import { act, fireEvent, render, screen } from "@testing-library/react-native";

import { DRAFT_ROW } from "@/lib/dev/decode-bench";
import { RECORDINGS } from "@/lib/dev/recordings";
import { BenchScreen } from "./bench-screen";

jest.mock("@/lib/native/clipboard", () => ({ copyText: jest.fn(() => Promise.resolve(true)) }));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));
jest.mock("expo-router", () => ({
  Redirect: () => null,
  Stack: { Screen: () => null },
}));

describe("the decode bench screen", () => {
  it("shows the engine and runs the three modes over the recordings", async () => {
    jest.useFakeTimers();
    const recording = RECORDINGS.find((one) => one.name === "approval_in_the_middle");
    if (recording === undefined) {
      throw new Error("no recording approval_in_the_middle");
    }
    await render(<BenchScreen rounds={1} recordings={[recording]} />);
    expect(screen.getByText("another engine")).toBeOnTheScreen();
    expect(screen.getByText("zod compiles its parsers")).toBeOnTheScreen();
    await fireEvent.press(screen.getByText("Run the bench"));
    await act(async () => {
      await jest.runAllTimersAsync();
    });
    expect(screen.getByText("Run again")).toBeOnTheScreen();
    expect(
      screen.getByText(`${String(recording.lines.length)} items x 1 rounds`),
    ).toBeOnTheScreen();
    for (const mode of ["zod", "jitless", "hand"]) {
      expect(screen.getByText(new RegExp(`^${mode}: draft p95 `))).toBeOnTheScreen();
    }
    expect(screen.getAllByLabelText(DRAFT_ROW)).toHaveLength(3);
    expect(screen.getByLabelText("Copy the report")).toBeOnTheScreen();
    jest.useRealTimers();
  });
});
