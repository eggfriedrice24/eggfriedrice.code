import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, render, screen } from "@testing-library/react-native";

import {
  approvalRequested,
  approvalResolved,
  draftItem,
  eventItem,
  promptQueued,
  run,
  toolCallStarted,
  turnStarted,
} from "@/lib/stream/testing";

import { LoadingLine } from "./loading-line";
import { RunningLineView, RunningRow } from "./running-line";
import { InScope, testScope } from "./transcript-testing";

jest.mock("./fade-in", () => ({
  FadeIn: (props: { children: unknown }) => props.children,
}));

const HIDDEN = { includeHiddenElements: true } as const;
const NOW = new Date(2026, 9, 26, 14, 0, 0).getTime();
const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";

async function wait(ms: number): Promise<void> {
  await act(async () => {
    jest.advanceTimersByTime(ms);
  });
}

beforeEach(() => {
  jest.useFakeTimers({ now: NOW });
});

afterEach(() => {
  jest.useRealTimers();
});

describe("the running line", () => {
  it("says running in yolk, with the time from 2 s on", async () => {
    await render(<RunningLineView startedAt={NOW} compacting={false} />);
    expect(screen.getByText("running").props.className).toContain("text-accent-primary");
    expect(screen.getByLabelText("running")).toBeOnTheScreen();
    await wait(72_000);
    expect(screen.getByText("· 1m 12s")).toBeOnTheScreen();
    expect(screen.getByLabelText("running, 1m 12s")).toBeOnTheScreen();
  });

  it("says compacting while efrd compacts the context", async () => {
    await render(<RunningLineView startedAt={NOW} compacting />);
    expect(screen.getByText("compacting")).toBeOnTheScreen();
  });

  it("puts the writing blocks of the turn above the line", async () => {
    const state = run([
      eventItem(1, promptQueued(TURN, "Go.")),
      eventItem(2, turnStarted(TURN)),
      draftItem(TURN, 2, { kind: "tool_input", call: 0, tool: "apply_patch", bytes: 2_345 }),
    ]);
    const { scope } = testScope(state);
    await render(
      <InScope scope={scope}>
        <RunningRow turnId={TURN} />
      </InScope>,
    );
    expect(screen.getByLabelText("writing patch, 2.3 KB")).toBeOnTheScreen();
    expect(screen.getAllByTestId("braille-cell", HIDDEN)).toHaveLength(2);
  });

  it("goes away while an approval of the turn waits, and comes back after the answer", async () => {
    const asked = [
      eventItem(1, promptQueued(TURN, "Go.")),
      eventItem(2, turnStarted(TURN)),
      eventItem(3, toolCallStarted(TURN, "call-1", "shell", { command: "git push" })),
      eventItem(4, approvalRequested(TURN, "call-1")),
    ];
    const waiting = testScope(run(asked)).scope;
    const view = await render(
      <InScope scope={waiting}>
        <RunningRow turnId={TURN} />
      </InScope>,
    );
    expect(screen.queryByText("running")).toBeNull();
    await view.unmount();
    const answered = testScope(
      run([...asked, eventItem(5, approvalResolved(TURN, "call-1", "allow"))]),
    ).scope;
    await render(
      <InScope scope={answered}>
        <RunningRow turnId={TURN} />
      </InScope>,
    );
    expect(screen.getByText("running")).toBeOnTheScreen();
  });
});

describe("the loading line", () => {
  it("names the machine and waits in the grey of the network", async () => {
    await render(<LoadingLine machineName="desktop" />);
    expect(screen.getByLabelText("loading conversation from desktop")).toBeOnTheScreen();
    expect(screen.getByLabelText("Loading")).toBeOnTheScreen();
    await wait(3_000);
    expect(screen.getByText("loading conversation from desktop · 3s")).toBeOnTheScreen();
  });
});
