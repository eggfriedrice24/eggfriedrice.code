// The approval strip and the approval sheet (boards C-Waiting, S-Approvals and AP-Sheets)
// on a real stream store with a fake runtime: the line, the small text that matters, the
// answer and the wait for efrd's echo, the next question, a question that only the
// terminal answers, the seconds left, and a question that closes while the sheet shows.

import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { notifyManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createStore } from "@tanstack/store";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { randomUUID } from "expo-crypto";
import { ImpactFeedbackStyle, impactAsync } from "expo-haptics";
import type { ReactNode } from "react";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { FakeClock } from "@/lib/net/testing";
import type { EventOf } from "@/lib/protocol/wire";
import { createQueryClient } from "@/lib/query/query-client";
import { FakeCaller } from "@/lib/query/testing";
import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";
import {
  AT_MS,
  approvalResolved,
  eventItem,
  run,
  toolCallStarted,
  turnStarted,
} from "@/lib/stream/testing";
import { ApprovalSheet } from "./approval-sheet";
import { ApprovalStrip } from "./approval-strip";

jest.mock("expo-haptics", () => ({
  ImpactFeedbackStyle: { Light: "light", Medium: "medium" },
  NotificationFeedbackType: { Success: "success", Error: "error" },
  impactAsync: jest.fn(() => Promise.resolve()),
  notificationAsync: jest.fn(() => Promise.resolve()),
}));

jest.mock("expo-crypto", () => ({ randomUUID: jest.fn() }));

// NOTE: Query tells its observers on a timer by default; here it tells them at once.
notifyManager.setScheduler((notify) => notify());

const clients: QueryClient[] = [];

beforeEach(() => {
  jest.clearAllMocks();
  let ids = 0;
  jest.mocked(randomUUID).mockImplementation(() => {
    ids += 1;
    return `id-${ids}`;
  });
});

afterEach(() => {
  for (const client of clients.splice(0)) {
    client.clear();
  }
});

function wrapper(children: ReactNode) {
  const client = createQueryClient();
  // NOTE: a finished mutation keeps a real timer until its gcTime ends. Infinity sets none.
  const defaults = client.getDefaultOptions();
  client.setDefaultOptions({ ...defaults, mutations: { ...defaults.mutations, gcTime: Infinity } });
  clients.push(client);
  return (
    <SafeAreaProvider initialMetrics={METRICS}>
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    </SafeAreaProvider>
  );
}

const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

const REF = { machine: "m1", id: "conv-1" } as const;

function ask(
  call: string,
  line: string,
  extra: Partial<EventOf<"approval_requested">> = {},
): EventOf<"approval_requested"> {
  return {
    kind: "approval_requested",
    turn_id: "t1",
    call_id: call,
    summary: `shell: run ${JSON.stringify(line)}`,
    reason: "cautious asks before any network access.",
    ...extra,
  };
}

function streamWith(...asks: EventOf<"approval_requested">[]) {
  let seq = 1;
  const items = [eventItem(seq, { ...turnStarted("t1"), cwd: "/home/me/code/efr" })];
  for (const one of asks) {
    items.push(eventItem(++seq, toolCallStarted("t1", one.call_id, "shell")));
    items.push(eventItem(++seq, one));
  }
  return createStore<StreamState>(run(items, EMPTY_STREAM));
}

describe("the approval strip", () => {
  it("shows the command on one line and answers with the medium haptic", async () => {
    const stream = streamWith(ask("c1", "git push -u origin fix/steer-race"));
    const caller = new FakeCaller({ "approval.respond": () => ({ seq: 9 }) });
    const onOpen = jest.fn<(callId: string) => void>();
    await render(
      wrapper(
        <ApprovalStrip
          stream={stream}
          conversation={REF}
          machineName="desktop"
          connected
          caller={caller}
          onOpen={onOpen}
        />,
      ),
    );
    expect(screen.getByText("git push -u origin fix/steer-race")).toBeOnTheScreen();
    expect(screen.queryByText("1 of 1")).toBeNull();
    await fireEvent.press(
      screen.getByRole("button", { name: "Approval needed: git push -u origin fix/steer-race" }),
    );
    expect(onOpen).toHaveBeenCalledWith("c1");
    await fireEvent.press(screen.getByRole("button", { name: "Allow" }));
    expect(impactAsync).toHaveBeenCalledWith(ImpactFeedbackStyle.Medium);
    expect(caller.callsOf("approval.respond").map((call) => call.params)).toEqual([
      { call_id: "c1", command_id: "id-1", conversation_id: "conv-1", decision: "allow" },
    ]);
    // efrd took the answer, but the stream has no `approval_resolved` yet.
    expect(screen.getByRole("button", { name: "Allowing" })).toBeBusy();
    expect(screen.getByRole("button", { name: "Deny" })).toBeDisabled();
    await act(async () => {
      stream.setState((state) =>
        run([eventItem(9, approvalResolved("t1", "c1", "allow", "phone"))], state),
      );
    });
    expect(screen.queryByTestId("approval-strip")).toBeNull();
  });

  it("says 1 of 3 while more wait, and shows the next once one is answered", async () => {
    const stream = streamWith(
      ask("c1", "git push -u origin fix/steer-race"),
      ask("c2", "gh pr create --draft --fill"),
      ask("c3", "cargo publish -p efr-proto"),
    );
    await render(
      wrapper(
        <ApprovalStrip
          stream={stream}
          conversation={REF}
          machineName="desktop"
          connected
          caller={new FakeCaller()}
          onOpen={jest.fn()}
        />,
      ),
    );
    expect(screen.getByText("1 of 3")).toBeOnTheScreen();
    await act(async () => {
      stream.setState((state) =>
        run([eventItem(20, approvalResolved("t1", "c1", "deny", "shell"))], state),
      );
    });
    expect(screen.getByText("gh pr create --draft --fill")).toBeOnTheScreen();
    expect(screen.getByText("1 of 2")).toBeOnTheScreen();
  });

  it("dims both buttons and says offline while the machine is not connected", async () => {
    await render(
      wrapper(
        <ApprovalStrip
          stream={streamWith(ask("c1", "git push"))}
          conversation={REF}
          machineName="desktop"
          connected={false}
          caller={new FakeCaller()}
          onOpen={jest.fn()}
        />,
      ),
    );
    expect(screen.getByText("offline")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Allow" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Deny" })).toBeDisabled();
  });

  it("names the machine in place of the buttons for a question of the terminal", async () => {
    await render(
      wrapper(
        <ApprovalStrip
          stream={streamWith(
            ask("c1", "sudo btrfs subvolume delete /snapshots/2026-09-*", {
              local_only: true,
              interactive: true,
            }),
          )}
          conversation={REF}
          machineName="laptop"
          connected
          caller={new FakeCaller()}
          onOpen={jest.fn()}
        />,
      ),
    );
    expect(screen.getByText("answer on laptop")).toBeOnTheScreen();
    expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
  });

  it("shows the seconds left only under a minute", async () => {
    const clock = new FakeClock(AT_MS);
    await render(
      wrapper(
        <ApprovalStrip
          stream={streamWith(ask("c1", "git push", { expires_in_secs: 120 }))}
          conversation={REF}
          machineName="desktop"
          connected
          caller={new FakeCaller()}
          clock={clock}
          onOpen={jest.fn()}
        />,
      ),
    );
    expect(screen.queryByText(/\d+s$/)).toBeNull();
    await act(() => clock.advance(78_000));
    expect(screen.getByText("42s")).toBeOnTheScreen();
    await act(() => clock.advance(42_000));
    expect(screen.getByText("time is up")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Allow" })).toBeDisabled();
  });
});

describe("the approval sheet", () => {
  it("shows the full command and one quiet line, and closes once efrd took the answer", async () => {
    const stream = streamWith(ask("c1", "git push -u origin fix/steer-race"));
    const caller = new FakeCaller({ "approval.respond": () => ({ seq: 9 }) });
    const onClose = jest.fn<() => void>();
    await render(
      wrapper(
        <ApprovalSheet
          stream={stream}
          conversation={REF}
          callId="c1"
          machineName="desktop"
          connected
          caller={caller}
          onClose={onClose}
        />,
      ),
    );
    expect(screen.getByText(/git push -u origin fix\/steer-race/)).toBeOnTheScreen();
    expect(
      screen.getByText("in ~/code/efr · cautious asks before any network access"),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Deny" }));
    expect(caller.callsOf("approval.respond").map((call) => call.params)).toEqual([
      { call_id: "c1", command_id: "id-1", conversation_id: "conv-1", decision: "deny" },
    ]);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("dims a question that closed while it shows, with Close and Next in the header", async () => {
    const stream = streamWith(
      ask("c1", "git push -u origin fix/steer-race", { expires_in_secs: 60 }),
      ask("c2", "gh pr create --draft --fill"),
      ask("c3", "cargo publish -p efr-proto"),
    );
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
    expect(screen.getByText("1 of 3")).toBeOnTheScreen();
    await act(async () => {
      stream.setState((state) =>
        run([eventItem(20, { kind: "approval_expired", turn_id: "t1", call_id: "c1" })], state),
      );
    });
    expect(screen.getByText("expired")).toBeOnTheScreen();
    expect(screen.getByText("· 2 more waiting")).toBeOnTheScreen();
    expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
    // Close and Next are items of the native header of the route (`nextQuestion`, device
    // check), not buttons of the sheet.
    expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
  });

  it("says why only the terminal can answer, with no buttons", async () => {
    await render(
      wrapper(
        <ApprovalSheet
          stream={streamWith(
            ask("c1", "sudo btrfs subvolume delete /snapshots/2026-09-*", {
              local_only: true,
              interactive: true,
              reason: null,
            }),
          )}
          conversation={REF}
          callId="c1"
          machineName="laptop"
          connected
          caller={new FakeCaller()}
          onClose={jest.fn()}
        />,
      ),
    );
    expect(
      screen.getByText("in ~/code/efr · needs your password, which only the terminal can take"),
    ).toBeOnTheScreen();
    expect(screen.getByText("answer on laptop")).toBeOnTheScreen();
    expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
  });

  it("says so for a question that the stream does not hold", async () => {
    await render(
      wrapper(
        <ApprovalSheet
          stream={streamWith(ask("c1", "git push"))}
          conversation={REF}
          callId="other"
          machineName="desktop"
          connected
          caller={new FakeCaller()}
          onClose={jest.fn()}
        />,
      ),
    );
    expect(screen.getByText("The question is not open now.")).toBeOnTheScreen();
  });
});
