import { describe, expect, it } from "vitest";

import type { DraftPart, Event, SubscribeItem } from "@/lib/protocol/wire";
import { reduce, withLink } from "@/lib/stream/reducer";
import {
  approvalRequested,
  approvalResolved,
  draftItem,
  eventItem,
  messageCompleted,
  promptQueued,
  run,
  textPart,
  toolCallStarted,
  turnStarted,
} from "@/lib/stream/testing";
import {
  compareApprovals,
  compareRows,
  compareShallow,
  createRowsSelector,
  selectApproval,
  selectMessage,
  selectPendingApprovals,
  selectPrompt,
  selectReasoning,
  selectStatus,
  selectTool,
  selectTurnEnd,
} from "./selectors";

const TURN = "t1";
let seq = 0;
const event = (body: Event): SubscribeItem => eventItem(++seq, body);
const draft = (part: DraftPart): SubscribeItem => draftItem(TURN, seq, part);

describe("the part selectors", () => {
  it("keep a settled message while the live one streams", () => {
    const state = run([
      event(turnStarted(TURN)),
      event(messageCompleted(TURN, 0, "First.")),
      draft(textPart(1, 0, "Two")),
    ]);
    const settled = selectMessage(TURN, 0);
    const live = selectMessage(TURN, 1);
    const next = reduce(state, draft(textPart(1, 3, " more")));
    expect(settled(next)).toBe(settled(state));
    expect(live(next)).not.toBe(live(state));
    expect(live(next)?.buffer.text).toBe("Two more");
  });

  it("read a tool call, an approval and the end of a turn by key", () => {
    const state = run([
      event(turnStarted(TURN)),
      event(toolCallStarted(TURN, "c", "shell", { command: "ls" })),
      event(approvalRequested(TURN, "c")),
    ]);
    expect(selectTool(TURN, "c")(state)?.summary).toBe("ls");
    expect(selectApproval(TURN, "c")(state)?.status).toBe("pending");
    expect(selectTurnEnd(TURN)(state)).toBeUndefined();
    const text = reduce(state, draft(textPart(0, 0, "x")));
    expect(selectTool(TURN, "c")(text)).toBe(selectTool(TURN, "c")(state));
    expect(selectApproval(TURN, "c")(text)).toBe(selectApproval(TURN, "c")(state));
  });

  it("give a queued prompt and the thought of a turn, equal while they stay the same", () => {
    const queued = run([event(promptQueued("t2", "later", "phone"))]);
    expect(selectPrompt("t2")(queued)).toMatchObject({ queued: true, prompt: { text: "later" } });
    const thinking = run([
      event(turnStarted(TURN)),
      draft({ kind: "reasoning", offset: 0, delta: "**Plan**", title: "Plan" }),
    ]);
    const thought = selectReasoning(TURN);
    expect(thought(thinking)).toMatchObject({ title: "Plan", live: true });
    const writing = reduce(thinking, draft(textPart(0, 0, "Hi")));
    expect(compareShallow(thought(thinking), thought(writing))).toBe(true);
    expect(compareShallow(selectPrompt(TURN)(thinking), selectPrompt(TURN)(writing))).toBe(true);
  });
});

describe("the structure selector", () => {
  it("gives the same list while only text changes", () => {
    const select = createRowsSelector();
    const state = run([event(turnStarted(TURN)), draft(textPart(0, 0, "a"))]);
    const rows = select(state);
    const next = reduce(state, draft(textPart(0, 1, "b")));
    expect(select(next)).toBe(rows);
    expect(compareRows(rows, select(next))).toBe(true);
  });
});

describe("the status selector", () => {
  it("names the running turn, what it does, where to answer and the context", () => {
    const state = run([
      event(promptQueued(TURN, "go")),
      event(turnStarted(TURN)),
      draft({ kind: "context", tokens: 42, limit: 100, window: 200 }),
      event(toolCallStarted(TURN, "c", "shell")),
      event({ ...approvalRequested(TURN, "c"), local_only: true }),
      event(promptQueued("t2", "next")),
    ]);
    expect(selectStatus(withLink(state, "live"))).toEqual({
      runningTurnId: TURN,
      runningSince: Date.parse("2026-10-26T12:00:00Z"),
      activity: "waiting for you on your computer",
      waiting: "computer",
      queued: 1,
      compacting: false,
      context: { tokens: 42, limit: 100, window: 200 },
      cwd: "/home/me",
      mode: undefined,
      link: "live",
      loaded: true,
    });
  });

  it("stays equal while the text streams", () => {
    const state = run([event(turnStarted(TURN)), draft(textPart(0, 0, "a"))]);
    const next = reduce(state, draft(textPart(0, 1, "b")));
    expect(compareShallow(selectStatus(state), selectStatus(next))).toBe(true);
    expect(selectStatus(run([])).runningTurnId).toBeUndefined();
  });
});

describe("the approvals selector", () => {
  it("lists the pending approvals, equal until one comes or goes", () => {
    const state = run([event(turnStarted(TURN)), event(approvalRequested(TURN, "a"))]);
    const pending = selectPendingApprovals(state);
    expect(pending.map((approval) => approval.callId)).toEqual(["a"]);
    const text = reduce(state, draft(textPart(0, 0, "x")));
    expect(compareApprovals(pending, selectPendingApprovals(text))).toBe(true);
    const answered = reduce(state, event(approvalResolved(TURN, "a", "allow")));
    expect(selectPendingApprovals(answered)).toEqual([]);
    expect(compareApprovals(pending, selectPendingApprovals(answered))).toBe(false);
  });
});
