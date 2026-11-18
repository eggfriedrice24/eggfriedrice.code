// The question of the model on the phone (S-Ask 1, 2 and 4) on a real stream store with a
// fake runtime: the strip and its place among the approvals, the native action sheet and
// the answer it sends, "Type an answer", and the row of the call.

import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { notifyManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createStore } from "@tanstack/store";
import { fireEvent, render, screen } from "@testing-library/react-native";
import { randomUUID } from "expo-crypto";
import type { ReactNode } from "react";

import { showActionSheet } from "@/components/ui/native/show-action-sheet";
import type { Event } from "@/lib/protocol/wire";
import { createQueryClient } from "@/lib/query/query-client";
import { FakeCaller } from "@/lib/query/testing";
import { EMPTY_STREAM, type StreamState, type UserQuestionView } from "@/lib/stream/reducer";
import {
  approvalRequested,
  at,
  eventItem,
  run,
  toolCallStarted,
  turnStarted,
  userQuestionRequested,
} from "@/lib/stream/testing";
import { useAnswerQuestion } from "../use-answer-question";
import { ApprovalStrip } from "./approval-strip";
import { QuestionRowView } from "./question-row";
import { ToolRow } from "./tool-row";
import { InScope, testScope } from "./transcript-testing";
import type { ScopeQuestions } from "./use-transcript-scope";

jest.mock("expo-crypto", () => ({ randomUUID: jest.fn() }));
jest.mock("./fade-in", () => ({
  FadeIn: (props: { children: unknown }) => props.children,
}));
jest.mock("@/components/ui/native/show-action-sheet", () => ({
  showActionSheet: jest.fn(() => Promise.resolve(undefined)),
}));

// NOTE: Query tells its observers on a timer by default; here it tells them at once.
notifyManager.setScheduler((notify) => notify());

const REF = { machine: "m1", id: "conv-1" } as const;
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
  const defaults = client.getDefaultOptions();
  client.setDefaultOptions({ ...defaults, mutations: { ...defaults.mutations, gcTime: Infinity } });
  clients.push(client);
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

/** A stream where the events of `bodies` came one second apart. */
function streamOf(...bodies: Event[]) {
  const items = [turnStarted("t1"), ...bodies].map((body, index) =>
    eventItem(index + 1, body, at(index)),
  );
  return createStore<StreamState>(run(items, EMPTY_STREAM));
}

const ASKED: Event[] = [
  toolCallStarted("t1", "k1", "ask_user"),
  userQuestionRequested("t1", "k1", "q1"),
];

interface HostProps {
  readonly stream: ReturnType<typeof streamOf>;
  readonly caller: FakeCaller;
  readonly typed: UserQuestionView[];
}

/** The strip with the answers of the screen. */
function Host(props: HostProps) {
  const answers = useAnswerQuestion(props.caller, REF, (question) => {
    props.typed.push(question);
  });
  return (
    <ApprovalStrip
      stream={props.stream}
      conversation={REF}
      machineName="desktop"
      connected
      caller={props.caller}
      onOpen={() => undefined}
      questions={answers}
    />
  );
}

async function renderHost(stream: ReturnType<typeof streamOf>) {
  const caller = new FakeCaller({ "question.respond": () => ({ seq: 9 }) });
  const typed: UserQuestionView[] = [];
  await render(wrapper(<Host stream={stream} caller={caller} typed={typed} />));
  return { caller, typed };
}

const STRIP_NAME = "Question: Which database should the tests use?";

describe("the strip of a question", () => {
  it("shows the question and Answer, and a tap opens the native sheet", async () => {
    jest.mocked(showActionSheet).mockResolvedValueOnce("option:1");
    const { caller } = await renderHost(streamOf(...ASKED));
    expect(screen.getByTestId("question-strip")).toBeOnTheScreen();
    expect(screen.getByText("Which database should the tests use?")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: STRIP_NAME }));
    const sheet = jest.mocked(showActionSheet).mock.calls[0]?.[0];
    expect(sheet?.actions.map((action) => action.label)).toEqual([
      "Postgres 16",
      "SQLite",
      "both",
      "Type an answer",
      "Skip",
    ]);
    expect(caller.callsOf("question.respond").map((call) => call.params)).toEqual([
      {
        command_id: "id-1",
        conversation_id: "conv-1",
        question_id: "q1",
        answer: { kind: "option", index: 1 },
      },
    ]);
  });

  it("skips, and puts the composer in the answer state", async () => {
    jest.mocked(showActionSheet).mockResolvedValueOnce("skip").mockResolvedValueOnce("type");
    const { caller, typed } = await renderHost(streamOf(...ASKED));
    await fireEvent.press(screen.getByRole("button", { name: "Answer" }));
    expect(caller.callsOf("question.respond").map((call) => call.params)).toEqual([
      expect.objectContaining({ answer: { kind: "skipped" } }),
    ]);
    await fireEvent.press(screen.getByRole("button", { name: STRIP_NAME }));
    expect(typed.map((question) => question.questionId)).toEqual(["q1"]);
  });

  it("does nothing on Cancel", async () => {
    const { caller, typed } = await renderHost(streamOf(...ASKED));
    await fireEvent.press(screen.getByRole("button", { name: STRIP_NAME }));
    expect(caller.callsOf("question.respond")).toEqual([]);
    expect(typed).toEqual([]);
  });

  it("gives the slot to a question that is older than the approval, and counts both", async () => {
    await renderHost(
      streamOf(...ASKED, toolCallStarted("t1", "c1", "shell"), approvalRequested("t1", "c1")),
    );
    expect(screen.getByTestId("question-strip")).toBeOnTheScreen();
    expect(screen.getByText("1 of 2")).toBeOnTheScreen();
  });

  it("gives the slot to an approval that is older than the question", async () => {
    await renderHost(
      streamOf(toolCallStarted("t1", "c1", "shell"), approvalRequested("t1", "c1"), ...ASKED),
    );
    expect(screen.getByTestId("approval-strip")).toBeOnTheScreen();
    expect(screen.getByText("1 of 2")).toBeOnTheScreen();
  });
});

describe("the row of a question (S-Ask 4)", () => {
  it("opens the sheet from a waiting row", async () => {
    const onOpen = jest.fn<(anchor: unknown) => void>();
    const waiting = { question: "Which database?", state: "waiting for you", waiting: true };
    await render(<QuestionRowView row={waiting} onOpen={onOpen} />);
    await fireEvent.press(
      screen.getByRole("button", { name: "Which database?, waiting for you." }),
    );
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("takes a tap only while the phone can answer, as the strip does", async () => {
    const { scope } = testScope(streamOf(...ASKED).state);
    const open = jest.fn<ScopeQuestions["open"]>();
    const row = (enabled: boolean) => (
      <InScope
        scope={{ ...scope, questions: { open, sending: undefined, enabled: () => enabled } }}
      >
        <ToolRow rowKey="t1:k1" turnId="t1" callId="k1" />
      </InScope>
    );
    // Offline, or with its answer in flight: the row waits but takes no tap.
    const view = await render(row(false));
    expect(screen.getByTestId("question-row")).toBeOnTheScreen();
    expect(screen.queryByRole("button")).toBeNull();
    await view.rerender(row(true));
    await fireEvent.press(
      screen.getByRole("button", {
        name: "Which database should the tests use?, waiting for you.",
      }),
    );
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("takes no tap after its end", async () => {
    const ended = { question: "Which database?", state: "skipped", waiting: false };
    await render(<QuestionRowView row={ended} onOpen={() => undefined} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("skipped")).toBeOnTheScreen();
    // A past answer is no control: VoiceOver reads no "dimmed".
    const row = screen.getByLabelText("Which database?, skipped.");
    expect(row.props.accessibilityState?.disabled).not.toBe(true);
  });
});
