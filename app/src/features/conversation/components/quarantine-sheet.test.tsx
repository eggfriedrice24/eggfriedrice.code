// The quarantine question of the reviewer off (Q-Quarantine 1 to 6) on a real stream
// store: the strip, the sheet that the strip opens, the waiting card, and the answer of
// the terminal while the sheet shows.

import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { notifyManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createStore } from "@tanstack/store";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import type { ReactNode } from "react";
import { SafeAreaProvider } from "react-native-safe-area-context";

import type { SurfaceChange } from "@/lib/protocol/generated";
import type { EventOf } from "@/lib/protocol/wire";
import { createQueryClient } from "@/lib/query/query-client";
import { FakeCaller } from "@/lib/query/testing";
import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";
import {
  eventItem,
  run,
  toolCallCompleted,
  toolCallStarted,
  turnStarted,
} from "@/lib/stream/testing";
import { ApprovalSheet } from "./approval-sheet";
import { ApprovalStrip } from "./approval-strip";
import { ToolBlockView } from "./tool-block";

jest.mock("expo-crypto", () => ({ randomUUID: jest.fn(() => "id-1") }));
jest.mock("./fade-in", () => ({
  FadeIn: (props: { children: unknown }) => props.children,
}));

// NOTE: Query tells its observers on a timer by default; here it tells them at once.
notifyManager.setScheduler((notify) => notify());

const clients: QueryClient[] = [];

afterEach(() => {
  for (const client of clients.splice(0)) {
    client.clear();
  }
});

const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

function wrapper(children: ReactNode) {
  const client = createQueryClient();
  const defaults = client.getDefaultOptions();
  client.setDefaultOptions({ ...defaults, mutations: { ...defaults.mutations, gcTime: Infinity } });
  clients.push(client);
  return (
    <SafeAreaProvider initialMetrics={METRICS}>
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    </SafeAreaProvider>
  );
}

const REF = { machine: "m1", id: "conv-1" } as const;
const QUESTION = "019a9b1c-3d00-7a10-8b20-0000000000a1";

const FSMONITOR: SurfaceChange = {
  path: "/home/me/code/efr/.git/config",
  rule: "config_code_key",
  key: "core.fsmonitor",
  quarantined: true,
};
const HOOK: SurfaceChange = {
  path: "/home/me/code/efr/.git/hooks/pre-commit",
  rule: "hook_planted",
  quarantined: true,
};
const ENVRC: SurfaceChange = {
  path: "/home/me/code/efr/.envrc",
  rule: "protected_name_created",
  quarantined: false,
};

function streamWith(changes: readonly SurfaceChange[]) {
  const asked: EventOf<"surface_question_requested"> = {
    kind: "surface_question_requested",
    turn_id: "t1",
    call_id: "c1",
    question_id: QUESTION,
    changes: [...changes],
  };
  return createStore<StreamState>(
    run(
      [
        eventItem(1, { ...turnStarted("t1"), cwd: "/home/me/code/efr" }),
        eventItem(2, toolCallStarted("t1", "c1", "shell", { command: "./scripts/setup-dev.sh" })),
        eventItem(3, toolCallCompleted("t1", "c1", "done")),
        eventItem(4, {
          kind: "sandbox_surface_changed",
          turn_id: "t1",
          call_id: "c1",
          changes: [...changes, ENVRC],
          quarantined: true,
          asks: true,
        }),
        eventItem(5, asked),
      ],
      EMPTY_STREAM,
    ),
  );
}

describe("the quarantine strip", () => {
  it("asks about the one change, offers Leave only, and opens the sheet", async () => {
    const onOpen = jest.fn<(callId: string) => void>();
    const caller = new FakeCaller({ "sandbox.surface_respond": () => ({ seq: 7 }) });
    await render(
      wrapper(
        <ApprovalStrip
          stream={streamWith([FSMONITOR])}
          conversation={REF}
          machineName="desktop"
          connected
          caller={caller}
          onOpen={onOpen}
        />,
      ),
    );
    expect(screen.getByText("keep .git/config (core.fsmonitor)?")).toBeOnTheScreen();
    // Keep stays at the terminal: words where the Keep button was, not a control.
    expect(screen.getByText("Keep on desktop")).toBeOnTheScreen();
    expect(screen.getByLabelText("Keep is possible only on desktop")).toBeOnTheScreen();
    expect(screen.queryByRole("button", { name: /^Keep/ })).toBeNull();
    await fireEvent.press(
      screen.getByRole("button", {
        name: "Git settings changed: keep .git/config (core.fsmonitor)?",
      }),
    );
    expect(onOpen).toHaveBeenCalledWith("c1");
    await fireEvent.press(screen.getByRole("button", { name: "Leave in quarantine" }));
    const sent = caller.callsOf("sandbox.surface_respond");
    expect(sent).toHaveLength(1);
    expect(sent[0]?.params).toMatchObject({
      conversation_id: REF.id,
      question_id: QUESTION,
      keep: false,
    });
  });

  it("says not sent when the Leave does not arrive, and offline with no tap when down", async () => {
    // No handler: the fake caller fails as an offline machine does.
    const caller = new FakeCaller({});
    const strip = (connected: boolean) => (
      <ApprovalStrip
        stream={streamWith([FSMONITOR])}
        conversation={REF}
        machineName="desktop"
        connected={connected}
        caller={caller}
        onOpen={() => undefined}
      />
    );
    const view = await render(wrapper(strip(true)));
    expect(screen.queryByTestId("quarantine-not-sent")).toBeNull();
    await fireEvent.press(screen.getByRole("button", { name: "Leave in quarantine" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByTestId("quarantine-not-sent")).toHaveTextContent("not sent");

    await view.rerender(wrapper(strip(false)));
    expect(screen.getByText("offline")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Leave in quarantine" })).toBeDisabled();
  });

  it("counts many changes", async () => {
    await render(
      wrapper(
        <ApprovalStrip
          stream={streamWith([FSMONITOR, HOOK])}
          conversation={REF}
          machineName="desktop"
          connected
          caller={new FakeCaller()}
          onOpen={jest.fn()}
        />,
      ),
    );
    expect(screen.getByText("keep 2 git changes?")).toBeOnTheScreen();
  });
});

describe("the quarantine sheet", () => {
  it("lists the quarantined changes, and dims with the answer of the terminal", async () => {
    const stream = streamWith([FSMONITOR, HOOK]);
    await render(
      wrapper(
        <ApprovalSheet
          stream={stream}
          conversation={REF}
          callId="c1"
          machineName="desktop"
          connected
          caller={new FakeCaller()}
          onClose={jest.fn()}
        />,
      ),
    );
    expect(screen.getByTestId("quarantine-sheet")).toBeOnTheScreen();
    expect(screen.getByText(".git/config")).toBeOnTheScreen();
    expect(screen.getByText("core.fsmonitor")).toBeOnTheScreen();
    expect(screen.getByText(".git/hooks/pre-commit")).toBeOnTheScreen();
    expect(screen.getByText("a new hook")).toBeOnTheScreen();
    // A file left in place cannot be kept, so the sheet does not list it.
    expect(screen.queryByText(".envrc")).toBeNull();
    expect(
      screen.getByText(
        "Git settings that run programs, moved to quarantine, where nothing reads them.",
      ),
    ).toBeOnTheScreen();
    expect(screen.getByText("Keep on desktop")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Leave in quarantine" })).toBeOnTheScreen();
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
    await act(async () => {
      stream.setState((state) =>
        run(
          [
            eventItem(6, {
              kind: "surface_question_answered",
              turn_id: "t1",
              question_id: QUESTION,
              keep: false,
              origin: "shell",
            }),
          ],
          state,
        ),
      );
    });
    expect(screen.getByText("left in quarantine · on desktop")).toBeOnTheScreen();
    expect(screen.queryByText("Keep on desktop")).toBeNull();
    expect(screen.queryByRole("button", { name: "Leave in quarantine" })).toBeNull();
    // Close is an item of the native header of the route (device check), not of the sheet.
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
  });

  it("says offline and takes no tap on Leave while the machine is down, as the strip", async () => {
    const caller = new FakeCaller({ "sandbox.surface_respond": () => ({ seq: 7 }) });
    const sheet = (connected: boolean) => (
      <ApprovalSheet
        stream={streamWith([FSMONITOR])}
        conversation={REF}
        callId="c1"
        machineName="desktop"
        connected={connected}
        caller={caller}
        onClose={jest.fn()}
      />
    );
    const view = await render(wrapper(sheet(false)));
    expect(screen.getByText("offline")).toBeOnTheScreen();
    const leave = screen.getByRole("button", { name: "Leave in quarantine" });
    expect(leave).toBeDisabled();
    await fireEvent.press(leave);
    expect(caller.callsOf("sandbox.surface_respond")).toHaveLength(0);

    await view.rerender(wrapper(sheet(true)));
    expect(screen.queryByText("offline")).toBeNull();
    expect(screen.getByRole("button", { name: "Leave in quarantine" })).toBeEnabled();
  });
});

describe("the card of a call whose git settings went to quarantine", () => {
  it("waits with git settings changed, and opens the sheet on a tap", async () => {
    const state = streamWith([FSMONITOR]).state;
    const call = state.turns.get("t1")?.tools.get("c1");
    if (call === undefined) {
      throw new Error("no call");
    }
    const onOpen = jest.fn<() => void>();
    await render(
      <ToolBlockView
        call={call}
        turnEnded={false}
        expanded={false}
        onToggle={jest.fn()}
        onOpenApproval={onOpen}
        surface={[".envrc (a new agent or editor config); left in place"]}
      />,
    );
    expect(screen.getByText("git settings changed")).toBeOnTheScreen();
    expect(screen.getByTestId("tool-block").props.className).toContain("border-border-accent");
    await fireEvent.press(
      screen.getByRole("button", {
        name: "Command ./scripts/setup-dev.sh, git settings changed, waiting for you",
      }),
    );
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("shows the answer as its status word, with the command as it was", async () => {
    const stream = streamWith([FSMONITOR]);
    stream.setState((state) =>
      run(
        [
          eventItem(6, {
            kind: "surface_question_answered",
            turn_id: "t1",
            question_id: QUESTION,
            keep: true,
            origin: "shell",
          }),
        ],
        state,
      ),
    );
    const call = stream.state.turns.get("t1")?.tools.get("c1");
    if (call === undefined) {
      throw new Error("no call");
    }
    await render(
      <ToolBlockView call={call} turnEnded={false} expanded={false} onToggle={jest.fn()} />,
    );
    expect(screen.getByText("kept").props.className).toContain("text-text-muted");
    expect(screen.getByText("./scripts/setup-dev.sh").props.className).toContain(
      "text-text-bright",
    );
  });
});
