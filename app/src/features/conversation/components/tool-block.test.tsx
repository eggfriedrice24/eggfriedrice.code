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

const WAITING = { status: "pending", closedAt: undefined, answeredBy: undefined } as const;

describe("a tool block whose approval waits", () => {
  it("shows the yolk dot and the words, no loader and no time, and an accent outline", async () => {
    await render(
      <ToolBlockView
        call={call()}
        turnEnded={false}
        approval={WAITING}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("waiting for approval")).toBeOnTheScreen();
    expect(screen.queryByLabelText("Running", HIDDEN)).toBeNull();
    const card = screen.getByRole("button", {
      name: "Command cargo test -p efr-daemon, waiting for approval",
    });
    expect(card.props.accessibilityHint).toBe("Opens the approval.");
    expect(screen.getByTestId("tool-block").props.className).toContain("border-border-accent");
  });

  it("opens the approval sheet on a tap, and never toggles the output", async () => {
    const onOpen = jest.fn<() => void>();
    const onToggle = jest.fn<() => void>();
    await render(
      <ToolBlockView
        call={call()}
        turnEnded={false}
        approval={WAITING}
        expanded={false}
        onToggle={onToggle}
        onOpenApproval={onOpen}
      />,
    );
    await fireEvent.press(screen.getByRole("button", { name: /waiting for approval/ }));
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onToggle).not.toHaveBeenCalled();
  });

  it("shows the seconds left under a minute", async () => {
    await render(
      <ToolBlockView
        call={call()}
        turnEnded={false}
        approval={WAITING}
        left="42s"
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("42s")).toBeOnTheScreen();
    expect(
      screen.getByRole("button", { name: /waiting for approval, 42s left/ }),
    ).toBeOnTheScreen();
  });

  it("says allowed once answered, then runs with the loader", async () => {
    await render(
      <ToolBlockView
        call={call()}
        turnEnded={false}
        approval={{ status: "allowed", closedAt: NOW, answeredBy: "phone" }}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("allowed")).toBeOnTheScreen();
    expect(screen.getByTestId("tool-block").props.className).toContain("border-border-subtle");
    await act(async () => {
      jest.advanceTimersByTime(3_000);
    });
    expect(screen.queryByText("allowed")).toBeNull();
    expect(screen.getByLabelText("Running", HIDDEN)).toBeOnTheScreen();
  });

  it("says denied with the command in text_muted and no output", async () => {
    await render(
      <ToolBlockView
        call={call({ done: true, isError: true, refusal: "the user denied it" })}
        turnEnded={false}
        approval={{ status: "denied", closedAt: NOW, answeredBy: "shell" }}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("denied")).toBeOnTheScreen();
    expect(screen.getByText("cargo test -p efr-daemon").props.className).toContain(
      "text-text-muted",
    );
    expect(screen.queryByText("the user denied it")).toBeNull();
    expect(screen.queryByText("✗")).toBeNull();
  });
});

describe("a tool block in auto", () => {
  it("shows reviewing with the loader, and the line of what the call asks for", async () => {
    await render(
      <ToolBlockView
        call={call({
          exit: {
            kinds: ["host"],
            grants: [{ kind: "host", host: "api.github.com", port: 443 }],
            line: "gh pr create",
            background: false,
            hosts: ["api.github.com"],
            targets: [],
            programs: [],
            outside: [],
            uploads: [],
            reviewing: true,
            judged: undefined,
            saved: undefined,
          },
        })}
        turnEnded={false}
        expanded={false}
        onToggle={jest.fn()}
        note={{ text: "api.github.com:443 reachable", tone: "text_subtle", loud: undefined }}
      />,
    );
    expect(screen.getByText("reviewing")).toBeOnTheScreen();
    expect(screen.getByLabelText("Reviewing", HIDDEN)).toBeOnTheScreen();
    expect(screen.getByTestId("exit-note")).toHaveTextContent("api.github.com:443 reachable");
    expect(screen.queryByTestId("exit-note-loud")).toBeNull();
  });

  it("puts the host network on its own line after the verdict, as AU-Auto does", async () => {
    await render(
      <ToolBlockView
        call={call({ done: true, exitCode: 0, endedAt: NOW + 1_200 })}
        turnEnded={false}
        expanded={false}
        onToggle={jest.fn()}
        note={{
          text: "reviewer allowed: ssh laptop uptime for this call (medium risk):",
          tone: "text_subtle",
          loud: "host network",
        }}
      />,
    );
    expect(
      screen.getByText("reviewer allowed: ssh laptop uptime for this call (medium risk):"),
    ).toBeOnTheScreen();
    expect(screen.getByTestId("exit-note-loud")).toHaveTextContent("host network");
    expect(screen.getByText("host network").props.className).toContain("text-text-primary");
  });

  it("lists the files that run code which the call changed, one quiet line each", async () => {
    await render(
      <ToolBlockView
        call={call({ done: true, exitCode: 0, endedAt: NOW + 1_400 })}
        turnEnded={false}
        expanded={false}
        onToggle={jest.fn()}
        surface={[
          ".git/config (core.fsmonitor); moved to quarantine",
          ".envrc (a new agent or editor config); left in place",
        ]}
      />,
    );
    expect(screen.getByText(".git/config (core.fsmonitor); moved to quarantine")).toBeOnTheScreen();
    expect(
      screen.getByText(".envrc (a new agent or editor config); left in place"),
    ).toBeOnTheScreen();
    expect(screen.queryByText("waiting for approval")).toBeNull();
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
