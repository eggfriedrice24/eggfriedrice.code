// The approval block in each state of the board S-Approvals, and the block on a real
// stream store with a fake runtime: the answer, the wait for efrd's echo, a failed
// answer and the countdown.

import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { notifyManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createStore } from "@tanstack/store";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { randomUUID } from "expo-crypto";
import { ImpactFeedbackStyle, impactAsync } from "expo-haptics";
import type { ReactNode } from "react";

import { FakeClock } from "@/lib/net/testing";
import { createQueryClient } from "@/lib/query/query-client";
import { FakeCaller } from "@/lib/query/testing";
import { EMPTY_STREAM, type ApprovalView, type StreamState } from "@/lib/stream/reducer";
import { AT_MS, approvalResolved, eventItem, run, turnStarted } from "@/lib/stream/testing";
import type { EventOf } from "@/lib/protocol/wire";
import { ApprovalBlock, ApprovalBlockView } from "./approval-block";
import { approvalBlock, NO_ANSWER, type ApprovalBlockContext } from "../model/approval-block";
import { clockTime } from "../model/approval-block-text";

jest.mock("expo-haptics", () => ({
  ImpactFeedbackStyle: { Light: "light", Medium: "medium" },
  NotificationFeedbackType: { Success: "success", Error: "error" },
  impactAsync: jest.fn(() => Promise.resolve()),
  notificationAsync: jest.fn(() => Promise.resolve()),
}));

jest.mock("expo-crypto", () => ({ randomUUID: jest.fn() }));

beforeEach(() => {
  jest.clearAllMocks();
  let ids = 0;
  jest.mocked(randomUUID).mockImplementation(() => {
    ids += 1;
    return `id-${ids}`;
  });
});

const PUSH: ApprovalView = {
  callId: "c1",
  turnId: "t1",
  summary: 'shell: run "git push -u origin fix/steer-race"',
  diffPreview: undefined,
  reason: undefined,
  modelReason: undefined,
  exitKinds: [],
  always: undefined,
  localOnly: false,
  interactive: false,
  askedAt: new Date(2026, 9, 26, 13, 2).getTime(),
  expiresAt: undefined,
  status: "pending",
  decision: undefined,
  answeredBy: undefined,
  closedAt: undefined,
};

const CONTEXT: ApprovalBlockContext = {
  machine: "desktop",
  cwd: "/home/me/code/efr",
  connected: true,
  expiry: undefined,
  answer: NO_ANSWER,
};

async function show(approval: ApprovalView, context: Partial<ApprovalBlockContext> = {}) {
  const onAnswer = jest.fn<(decision: "allow" | "deny") => void>();
  await render(
    <ApprovalBlockView
      model={approvalBlock(approval, { ...CONTEXT, ...context })}
      onAnswer={onAnswer}
    />,
  );
  return onAnswer;
}

describe("the approval block while it waits", () => {
  it("draws the gold outline, the header, the command, the cwd, the reason and the buttons", async () => {
    const onAnswer = await show(
      { ...PUSH, reason: "Pushes to a remote. cautious asks before any network access." },
      { expiry: "expires in 4m" },
    );
    const block = screen.getByLabelText("Approval needed");
    expect(block.props.className).toContain("border-border-accent");
    expect(block.props.className).toContain("rounded-box");
    expect(screen.getByText("needs approval")).toBeOnTheScreen();
    expect(screen.getByText("expires in 4m")).toBeOnTheScreen();
    expect(screen.getByText("$ git push -u origin fix/steer-race")).toBeOnTheScreen();
    expect(screen.getByText("in ~/code/efr")).toBeOnTheScreen();
    expect(
      screen.getByText("Pushes to a remote. cautious asks before any network access."),
    ).toBeOnTheScreen();
    expect(screen.getByText("cautious").props.className).toContain("text-mono-code");
    await fireEvent.press(screen.getByRole("button", { name: "Allow" }));
    await fireEvent.press(screen.getByRole("button", { name: "Deny" }));
    expect(onAnswer.mock.calls).toEqual([["allow"], ["deny"]]);
  });

  it("draws no reason line without a reason", async () => {
    await show(PUSH);
    expect(screen.queryByText(/asks before/)).toBeNull();
    expect(screen.getByRole("button", { name: "Allow" })).toBeEnabled();
  });

  it("wraps a long command and never cuts it", async () => {
    const line =
      'docker system prune --all --volumes --filter "until=720h" --force && docker builder prune --all --force';
    await show({ ...PUSH, summary: `shell: run ${JSON.stringify(line)}` });
    const command = screen.getByText(`$ ${line}`);
    expect(command.props.numberOfLines).toBeUndefined();
  });

  it("shows a file write with its edit line and its diff, each changed line spoken", async () => {
    await show({
      ...PUSH,
      summary: "write_file: write /home/me/.zshrc (user data)",
      diffPreview:
        '@@ -12,2 +12,3 @@\n export PATH="$HOME/.cargo/bin:$PATH"\n-source ~/.zsh/nvm.zsh\n+zsh-defer source ~/.zsh/nvm.zsh\n+zsh-defer source ~/.zsh/pyenv.zsh\n',
      reason: "Writes outside this conversation's folder, ~/.dotfiles.",
    });
    expect(screen.getByText("edit ~/.zshrc +2 −1")).toBeOnTheScreen();
    expect(screen.getByTestId("approval-diff")).toBeOnTheScreen();
    expect(screen.getByLabelText("removed: -source ~/.zsh/nvm.zsh")).toBeOnTheScreen();
    expect(screen.getByLabelText("added: +zsh-defer source ~/.zsh/nvm.zsh")).toBeOnTheScreen();
    expect(screen.getByText("zsh-defer ").props.className).toContain("text-diff-highlight-added");
    expect(screen.getByText("~/.dotfiles")).toBeOnTheScreen();
  });

  it("keeps Allow on the braille cell while the answer is on its way, and dims Deny", async () => {
    await show(PUSH, { answer: { sending: "allow", answeredHere: false, failed: undefined } });
    const allow = screen.getByRole("button", { name: "Allowing" });
    expect(allow).toBeBusy();
    expect(allow).toBeDisabled();
    expect(allow.props.className).not.toContain("opacity-40");
    const deny = screen.getByRole("button", { name: "Deny" });
    expect(deny).toBeDisabled();
    expect(deny.props.className).toContain("opacity-40");
    expect(screen.getByTestId("braille-cell", { includeHiddenElements: true })).toBeOnTheScreen();
  });

  it("says Denying while a deny is on its way", async () => {
    await show(PUSH, { answer: { sending: "deny", answeredHere: false, failed: undefined } });
    expect(screen.getByRole("button", { name: "Denying" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Allow" })).toBeDisabled();
  });

  it("closes the buttons while the machine is offline", async () => {
    await show(PUSH, { connected: false });
    expect(screen.getByRole("button", { name: "Allow" })).toBeDisabled();
    expect(screen.getByText("You can answer when desktop is connected again.")).toBeOnTheScreen();
  });
});

describe("the approval block that only the terminal answers", () => {
  it("names the machine and the reason for a privilege question, with no buttons", async () => {
    await show(
      {
        ...PUSH,
        summary: 'shell: run "sudo systemctl restart tailscaled"',
        localOnly: true,
        interactive: true,
      },
      { cwd: "/home/me", expiry: "expires in 9m" },
    );
    expect(screen.getByLabelText("Approval needed, answer on desktop")).toBeOnTheScreen();
    expect(screen.getByText("answer on desktop")).toBeOnTheScreen();
    expect(
      screen.getByText("It needs your password, which only the terminal can take."),
    ).toBeOnTheScreen();
    expect(screen.getByText("in ~")).toBeOnTheScreen();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("sends a background job to the terminal", async () => {
    await show({
      ...PUSH,
      summary: 'shell: run "restic backup ~/code --tag nightly &"',
      localOnly: true,
    });
    expect(
      screen.getByText(
        "It keeps running after the turn ends, so efr only starts it from the terminal.",
      ),
    ).toBeOnTheScreen();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("tells that an always rule is set in the terminal, and keeps the one-time answer", async () => {
    await show({
      ...PUSH,
      summary: 'shell: run "cargo publish -p efr-proto"',
      always: "run cargo publish",
    });
    expect(screen.getByText("always rule on desktop")).toBeOnTheScreen();
    expect(
      screen.getByText(
        'Answering can add an "always" rule, and rules are only set in the terminal.',
      ),
    ).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Allow" })).toBeEnabled();
  });
});

describe("the closed approval block", () => {
  it("collapses to a quiet line when answered, without the outline", async () => {
    await show(
      {
        ...PUSH,
        status: "allowed",
        decision: "allow",
        answeredBy: "phone",
        closedAt: new Date(2026, 9, 26, 14, 32).getTime(),
      },
      { answer: { sending: undefined, answeredHere: true, failed: undefined } },
    );
    expect(screen.getByText("✓")).toBeOnTheScreen();
    expect(screen.getByText("allowed on this phone · 14:32")).toBeOnTheScreen();
    const subject = screen.getByText("$ git push -u origin fix/steer-race");
    expect(subject.props.numberOfLines).toBe(1);
    expect(screen.getByTestId("approval-block").props.className).not.toContain("border");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("says that a terminal denied it", async () => {
    await show({
      ...PUSH,
      status: "denied",
      decision: "deny",
      answeredBy: "shell",
      closedAt: new Date(2026, 9, 26, 14, 33).getTime(),
    });
    expect(screen.getByText("✗")).toBeOnTheScreen();
    expect(screen.getByText("denied in a terminal on desktop · 14:33")).toBeOnTheScreen();
  });

  it("tells an expiry and an expiry while away apart", async () => {
    const closedAt = new Date(2026, 9, 26, 13, 12).getTime();
    await show({ ...PUSH, status: "expired", closedAt });
    expect(screen.getByText("expired · not answered · 13:12")).toBeOnTheScreen();
    await show({ ...PUSH, status: "expired_away", closedAt });
    expect(screen.getByText("expired while you were away · 13:02-13:12")).toBeOnTheScreen();
    expect(
      screen.getByLabelText(
        "Expired while you were away, 13:02-13:12: $ git push -u origin fix/steer-race",
      ),
    ).toBeOnTheScreen();
  });
});

const REF = { machine: "m1", id: "conv-1" } as const;

function approvalEvent(
  extra: Partial<EventOf<"approval_requested">> = {},
): EventOf<"approval_requested"> {
  return {
    kind: "approval_requested",
    turn_id: "t1",
    call_id: "c1",
    summary: 'shell: run "git push origin main"',
    reason: "cautious asks before any network access.",
    ...extra,
  };
}

function streamWith(extra: Partial<EventOf<"approval_requested">> = {}) {
  return createStore<StreamState>(
    run([eventItem(1, turnStarted("t1")), eventItem(2, approvalEvent(extra))], EMPTY_STREAM),
  );
}

// NOTE: Query tells its observers on a timer by default; here it tells them at once, so
// each change happens inside the act of the event that caused it.
notifyManager.setScheduler((notify) => notify());

const clients: QueryClient[] = [];

afterEach(() => {
  for (const client of clients.splice(0)) {
    client.clear();
  }
});

function wrapper(children: ReactNode) {
  const client = createQueryClient();
  // NOTE: a finished mutation keeps a real timer until its gcTime ends, which would keep
  // Jest running. Infinity sets no timer.
  const defaults = client.getDefaultOptions();
  client.setDefaultOptions({ ...defaults, mutations: { ...defaults.mutations, gcTime: Infinity } });
  clients.push(client);
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

describe("the approval block on a stream", () => {
  it("answers with the medium haptic and waits for efrd's echo", async () => {
    const stream = streamWith();
    const caller = new FakeCaller({ "approval.respond": () => ({ seq: 3 }) });
    await render(
      wrapper(
        <ApprovalBlock
          stream={stream}
          conversation={REF}
          turnId="t1"
          callId="c1"
          machineName="desktop"
          connected
          caller={caller}
        />,
      ),
    );
    expect(screen.getByText("in ~")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Allow" }));
    expect(impactAsync).toHaveBeenCalledWith(ImpactFeedbackStyle.Medium);
    expect(caller.callsOf("approval.respond").map((call) => [call.machine, call.params])).toEqual([
      ["m1", { call_id: "c1", command_id: "id-1", conversation_id: "conv-1", decision: "allow" }],
    ]);
    // NOTE: efrd took the answer, but the stream has no `approval_resolved` yet.
    expect(screen.getByRole("button", { name: "Allowing" })).toBeBusy();
    await fireEvent.press(screen.getByRole("button", { name: "Deny" }));
    expect(caller.callsOf("approval.respond")).toHaveLength(1);
    await act(async () => {
      stream.setState((state) =>
        run([eventItem(3, approvalResolved("t1", "c1", "allow", "phone"))], state),
      );
    });
    expect(screen.getByText(`allowed on this phone · ${clockTime(AT_MS)}`)).toBeOnTheScreen();
  });

  it("shows a failed answer and sends it again with the same command id", async () => {
    const stream = streamWith();
    const caller = new FakeCaller();
    await render(
      wrapper(
        <ApprovalBlock
          stream={stream}
          conversation={REF}
          turnId="t1"
          callId="c1"
          machineName="desktop"
          connected
          caller={caller}
        />,
      ),
    );
    await fireEvent.press(screen.getByRole("button", { name: "Deny" }));
    expect(
      await screen.findByText("The answer did not reach desktop. Try again."),
    ).toBeOnTheScreen();
    caller.answer({ "approval.respond": () => ({ seq: 3 }) });
    await fireEvent.press(screen.getByRole("button", { name: "Deny" }));
    const sent = caller.callsOf("approval.respond").map((call) => call.params);
    expect(sent).toEqual([
      { call_id: "c1", command_id: "id-1", conversation_id: "conv-1", decision: "deny" },
      { call_id: "c1", command_id: "id-1", conversation_id: "conv-1", decision: "deny" },
    ]);
    expect(await screen.findByRole("button", { name: "Denying" })).toBeDisabled();
    expect(screen.queryByText("The answer did not reach desktop. Try again.")).toBeNull();
  });

  it("sends nothing for a question that only the terminal answers", async () => {
    const caller = new FakeCaller({ "approval.respond": () => ({ seq: 3 }) });
    await render(
      wrapper(
        <ApprovalBlock
          stream={streamWith({ local_only: true, interactive: true })}
          conversation={REF}
          turnId="t1"
          callId="c1"
          machineName="laptop"
          connected
          caller={caller}
        />,
      ),
    );
    expect(screen.getByText("answer on laptop")).toBeOnTheScreen();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("counts the time to answer down with the clock, and closes the buttons at its end", async () => {
    const clock = new FakeClock(AT_MS);
    await render(
      wrapper(
        <ApprovalBlock
          stream={streamWith({ expires_in_secs: 300 })}
          conversation={REF}
          turnId="t1"
          callId="c1"
          machineName="desktop"
          connected
          caller={new FakeCaller()}
          clock={clock}
        />,
      ),
    );
    expect(screen.getByText("expires in 5m")).toBeOnTheScreen();
    await act(() => clock.advance(241_000));
    expect(screen.getByText("expires in 59s")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Allow" })).toBeEnabled();
    await act(() => clock.advance(59_000));
    expect(screen.getByText("time is up")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Allow" })).toBeDisabled();
    expect(clock.pendingTimers).toBe(0);
  });

  it("renders nothing for a question that the stream does not hold", async () => {
    await render(
      wrapper(
        <ApprovalBlock
          stream={streamWith()}
          conversation={REF}
          turnId="t1"
          callId="other"
          machineName="desktop"
          connected
          caller={new FakeCaller()}
        />,
      ),
    );
    expect(screen.queryByTestId("approval-block")).toBeNull();
  });
});
