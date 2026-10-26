// The decode bench screen: the facts of the engine, a run over every recording in each
// mode, and the report. The test runs one round, not the plan's 100.

import { describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen, waitFor } from "@testing-library/react-native";

import { DRAFT_ROW } from "@/lib/dev/decode-bench";
import { BenchScreen } from "./bench-screen";

jest.mock("@/lib/native/clipboard", () => ({ copyText: jest.fn(() => Promise.resolve(true)) }));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));
jest.mock("expo-router", () => ({
  Redirect: () => null,
  Stack: { Screen: () => null },
}));

describe("the decode bench screen", () => {
  it("shows the engine and runs the three modes over every recording", async () => {
    await render(<BenchScreen rounds={1} />);
    expect(screen.getByText("another engine")).toBeOnTheScreen();
    expect(screen.getByText("zod compiles its parsers")).toBeOnTheScreen();
    await fireEvent.press(screen.getByText("Run the bench"));
    await waitFor(() => expect(screen.getByText("Run again")).toBeOnTheScreen(), {
      timeout: 20_000,
    });
    for (const mode of ["zod", "jitless", "hand"]) {
      expect(screen.getByText(new RegExp(`^${mode}: draft p95 `))).toBeOnTheScreen();
    }
    expect(screen.getAllByLabelText(DRAFT_ROW)).toHaveLength(3);
    expect(screen.getByLabelText("Copy the report")).toBeOnTheScreen();
  });
});
