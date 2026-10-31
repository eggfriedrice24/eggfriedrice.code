import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, fireEvent, render, screen, within } from "@testing-library/react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type { ToolCallView } from "@/lib/stream/reducer";

import { ToolBlockView, WritingBlockView } from "./tool-block";

jest.mock("./fade-in", () => ({
  FadeIn: (props: { children: unknown }) => props.children,
}));
jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const HIDDEN = { includeHiddenElements: true } as const;
const NOW = new Date(2026, 9, 26, 14, 0, 0).getTime();

function call(overrides: Partial<ToolCallView> = {}): ToolCallView {
  return {
    callId: "c1",
    tool: "shell",
    summary: "cargo test -p efr-daemon",
    tail: "",
    bytes: 0,
    done: false,
    isError: false,
    exitCode: undefined,
    output: undefined,
    refusal: undefined,
    diff: undefined,
    leftRunning: false,
    startedAt: NOW,
    endedAt: undefined,
    ...overrides,
  };
}

const TAIL = Array.from({ length: 14 }, (_, i) => `test case_${String(i + 1)} ... ok`).join("\n");

beforeEach(() => {
  jest.useFakeTimers({ now: NOW });
  jest.mocked(copyText).mockClear();
  jest.mocked(haptic).mockClear();
});

afterEach(() => {
  jest.useRealTimers();
});

describe("a tool block whose approval waits", () => {
  it("shows the yolk dot and the words, no loader and no time, an accent outline and a link", async () => {
    await render(
      <ToolBlockView
        call={call()}
        turnEnded={false}
        approval={{ pending: true, closedAt: undefined }}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("waiting for approval")).toBeOnTheScreen();
    expect(screen.queryByLabelText("Running", HIDDEN)).toBeNull();
    expect(
      screen.getByRole("button", {
        name: "Command cargo test -p efr-daemon, waiting for approval",
      }),
    ).toBeOnTheScreen();
    expect(screen.getByTestId("tool-block").props.className).toContain("border-border-accent");
    expect(screen.getByTestId("approval-link", HIDDEN)).toBeOnTheScreen();
  });

  it("drops the outline and the link once answered, and runs again", async () => {
    await render(
      <ToolBlockView
        call={call()}
        turnEnded={false}
        approval={{ pending: false, closedAt: NOW }}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByLabelText("Running", HIDDEN)).toBeOnTheScreen();
    expect(screen.getByTestId("tool-block").props.className).toContain("border-border-subtle");
    expect(screen.queryByTestId("approval-link", HIDDEN)).toBeNull();
  });
});

describe("a running tool block", () => {
  it("shows the verb, the command and the yolk loader", async () => {
    await render(
      <ToolBlockView call={call()} turnEnded={false} expanded={false} onToggle={jest.fn()} />,
    );
    expect(screen.getByText("$")).toBeOnTheScreen();
    expect(screen.getByText("cargo test -p efr-daemon")).toBeOnTheScreen();
    expect(screen.getByLabelText("Running", HIDDEN)).toBeOnTheScreen();
    expect(
      screen.getByRole("button", { name: "Command cargo test -p efr-daemon, running" }),
    ).toBeOnTheScreen();
  });

  it("shows the elapsed time from 2 s on", async () => {
    await render(
      <ToolBlockView call={call()} turnEnded={false} expanded={false} onToggle={jest.fn()} />,
    );
    expect(screen.queryByText("12s")).toBeNull();
    await act(async () => {
      jest.advanceTimersByTime(12_000);
    });
    expect(screen.getByText("12s")).toBeOnTheScreen();
  });

  it("shows the last 3 lines of the output and offers the rest", async () => {
    await render(
      <ToolBlockView
        call={call({ tail: TAIL, bytes: TAIL.length })}
        turnEnded={false}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("test case_14 ... ok")).toBeOnTheScreen();
    expect(screen.getByText("test case_12 ... ok")).toBeOnTheScreen();
    expect(screen.queryByText("test case_11 ... ok")).toBeNull();
    expect(screen.getByRole("button", { name: "show all 14 lines" })).toBeOnTheScreen();
  });
});

describe("an ended tool block", () => {
  it("shows ✓ with the exit code and the time", async () => {
    await render(
      <ToolBlockView
        call={call({ done: true, exitCode: 0, endedAt: NOW + 38_000 })}
        turnEnded={false}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("✓")).toBeOnTheScreen();
    expect(screen.getByText("✓").props.className).toContain("text-state-success");
    expect(screen.getByText(/0 · 38s/)).toBeOnTheScreen();
  });

  it("shows ✗ with the exit code and marks the error words", async () => {
    await render(
      <ToolBlockView
        call={call({
          done: true,
          isError: true,
          exitCode: 101,
          endedAt: NOW + 9_000,
          output: "error: could not compile `efr-daemon`",
        })}
        turnEnded={false}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("✗").props.className).toContain("text-state-error");
    expect(screen.getByText(/101 · 9s/)).toBeOnTheScreen();
    expect(screen.getByText("error").props.className).toContain("text-state-error");
  });

  it("fails a call that the interrupt left without an answer", async () => {
    await render(
      <ToolBlockView
        call={call({ exitCode: 130 })}
        turnEnded
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("✗")).toBeOnTheScreen();
    expect(screen.getByText(/130/)).toBeOnTheScreen();
    expect(screen.queryByLabelText("Running", HIDDEN)).toBeNull();
  });

  it("names a read by its lines and shows no output", async () => {
    await render(
      <ToolBlockView
        call={call({
          tool: "read_file",
          summary: "crates/efr-daemon/src/turn.rs",
          done: true,
          output: "a\nb\nc",
        })}
        turnEnded={false}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("read")).toBeOnTheScreen();
    expect(screen.getByText(/3 lines/)).toBeOnTheScreen();
    expect(screen.queryByText("a")).toBeNull();
  });
});

describe("expanding a tool block", () => {
  it("toggles on a tap, with the expanded state for VoiceOver", async () => {
    const onToggle = jest.fn<() => void>();
    const done = call({ done: true, exitCode: 0, endedAt: NOW + 1000, output: TAIL });
    await render(
      <ToolBlockView call={done} turnEnded={false} expanded={false} onToggle={onToggle} />,
    );
    const block = screen.getByRole("button", { name: /Command cargo test/ });
    expect(block.props.accessibilityState).toMatchObject({ expanded: false });
    await fireEvent.press(block);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("shows the whole output and a collapse button when expanded", async () => {
    const done = call({ done: true, exitCode: 0, endedAt: NOW + 1000, output: TAIL });
    await render(<ToolBlockView call={done} turnEnded={false} expanded onToggle={jest.fn()} />);
    expect(screen.getByText("test case_1 ... ok")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "collapse" })).toBeOnTheScreen();
  });

  it("keeps the output out of the header button, so VoiceOver reads it", async () => {
    const done = call({ done: true, exitCode: 0, endedAt: NOW + 1000, output: TAIL });
    await render(<ToolBlockView call={done} turnEnded={false} expanded onToggle={jest.fn()} />);
    const header = screen.getByRole("button", { name: /Command cargo test/ });
    expect(within(header).queryByText("test case_1 ... ok")).toBeNull();
  });
});

describe("the copy menu of a tool block", () => {
  it("opens on a long press and copies the command or the output", async () => {
    const done = call({ done: true, exitCode: 0, endedAt: NOW + 1000, output: "ok" });
    await render(
      <ToolBlockView call={done} turnEnded={false} expanded={false} onToggle={jest.fn()} />,
    );
    const menu = screen.getByTestId("tool-menu");
    expect(menu.props.shouldOpenOnLongPress).toBe(true);
    expect(menu.props.actions.map((action: { title: string }) => action.title)).toEqual([
      "Copy command",
      "Copy output",
    ]);
    await fireEvent(menu, "pressAction", { nativeEvent: { event: "output" } });
    expect(copyText).toHaveBeenCalledWith("ok");
    await fireEvent(menu, "pressAction", { nativeEvent: { event: "command" } });
    expect(copyText).toHaveBeenLastCalledWith("cargo test -p efr-daemon");
    expect(haptic).toHaveBeenCalledWith("light");
  });
});

describe("the writing block", () => {
  it("names the patch that the model writes, with its size", async () => {
    await render(<WritingBlockView input={{ tool: "apply_patch", bytes: 2_345 }} />);
    expect(screen.getByLabelText("writing patch, 2.3 KB")).toBeOnTheScreen();
  });
});
