/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";

import {
  EMPTY_DETAILS,
  inputSummary,
  pendingApprovals,
  reduceDetails,
  runningTurn,
} from "./details";

// The frozen fixtures of efr-protocol: the same JSON that the Rust tests read.
const events = import.meta.glob<Record<string, unknown>>(
  "../../../../../crates/efr-protocol/fixtures/v1/events/*.json",
  { eager: true, import: "default" },
);
const items = import.meta.glob<unknown>(
  "../../../../../crates/efr-protocol/fixtures/v1/conversation_subscribe_item_*.json",
  { eager: true, import: "default" },
);

function fixture<T>(table: Record<string, T>, name: string): T {
  const entry = Object.entries(table).find(([path]) => path.endsWith(`/${name}.json`));
  if (entry === undefined) {
    throw new Error(`no fixture ${name}`);
  }
  return entry[1];
}

const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";
const CALL = "019a9b1c-3d00-7a10-8b20-000000000004";
const AT = "2026-10-03T12:00:05Z";

let seq = 0;
function envelope(event: unknown, at = AT) {
  seq += 1;
  return { kind: "event", seq, at, event };
}

function run(...list: unknown[]) {
  return list.reduce(reduceDetails, EMPTY_DETAILS);
}

describe("reduceDetails", () => {
  it("reads every event fixture without a throw, and skips an unknown kind", () => {
    let state = EMPTY_DETAILS;
    for (const event of Object.values(events)) {
      state = reduceDetails(state, envelope(event));
    }
    expect(state.lastSeq).toBe(seq);
    const unknown = reduceDetails(state, envelope(fixture(events, "unknown")));
    expect(unknown.turns).toBe(state.turns);
  });

  it("holds an approval question with its expiry, from the fixture", () => {
    const state = run(envelope(fixture(events, "approval_requested")));
    const approval = state.turns.get(TURN)?.approvals.get(CALL);
    expect(approval).toMatchObject({
      summary: "run `journalctl --vacuum-size=500M` as root",
      interactive: true,
      localOnly: false,
      state: "pending",
      reason: "the user asked for the report in Documents",
      askedAt: Date.parse(AT),
      expiresAt: Date.parse(AT) + 120_000,
    });
    expect(approval?.diffPreview).toContain("+SystemMaxUse=500M");
    expect(pendingApprovals(state)).toHaveLength(1);
  });

  it("reads `local_only` from its fixture", () => {
    const state = run(envelope(fixture(events, "approval_requested_local_only")));
    const approval = [...(state.turns.values().next().value?.approvals.values() ?? [])][0];
    expect(approval?.localOnly).toBe(true);
  });

  it("tells an answer, an expiry and an expiry while away apart", () => {
    const ask = (call: string) =>
      envelope({ kind: "approval_requested", turn_id: TURN, call_id: call, summary: call });
    const state = run(
      ask("a"),
      ask("b"),
      ask("c"),
      envelope({
        kind: "approval_resolved",
        turn_id: TURN,
        call_id: "a",
        decision: "allow",
        origin: "phone",
      }),
      envelope({ kind: "approval_expired", turn_id: TURN, call_id: "b" }),
      envelope({ kind: "approval_expired", turn_id: TURN, call_id: "c", away: true }),
    );
    const approvals = state.turns.get(TURN)?.approvals;
    expect(approvals?.get("a")).toMatchObject({
      state: "resolved",
      decision: "allow",
      answeredBy: "phone",
    });
    expect(approvals?.get("b")).toMatchObject({ state: "expired", away: false });
    expect(approvals?.get("c")).toMatchObject({ state: "expired", away: true });
  });

  it("expires a question that is open when its turn ends", () => {
    const state = run(
      envelope({ kind: "turn_started", turn_id: TURN }),
      envelope({ kind: "approval_requested", turn_id: TURN, call_id: "a", summary: "x" }),
      envelope({ kind: "turn_interrupted", turn_id: TURN }),
    );
    expect(state.turns.get(TURN)?.approvals.get("a")?.state).toBe("expired");
    expect(state.turns.get(TURN)?.end?.outcome).toBe("interrupted");
    expect(runningTurn(state)).toBeUndefined();
  });

  it("keeps the parts of a turn in the order they came", () => {
    let steer = { seq: 0 };
    const state = run(
      envelope({ kind: "prompt_queued", turn_id: TURN, text: "hi", origin: "phone" }),
      envelope({ kind: "turn_started", turn_id: TURN }),
      {
        kind: "draft",
        turn_id: TURN,
        after_seq: 2,
        draft: { kind: "text", index: 0, offset: 0, delta: "a" },
      },
      envelope({
        kind: "tool_call_started",
        turn_id: TURN,
        call_id: "t",
        tool: "shell",
        input: { command: "ls -la\nmore" },
      }),
      envelope({ kind: "approval_requested", turn_id: TURN, call_id: "t", summary: "run ls" }),
      (steer = envelope({ kind: "turn_steered", turn_id: TURN, text: "also check /tmp" })),
      envelope({ kind: "assistant_message_completed", turn_id: TURN, index: 1, text: "b" }),
      envelope({ kind: "assistant_message_completed", turn_id: TURN, index: 0, text: "a" }),
    );
    const turn = state.turns.get(TURN);
    expect(turn?.prompt).toEqual({ text: "hi", origin: "phone" });
    expect(turn?.parts).toEqual([
      { kind: "message", index: 0 },
      { kind: "tool", callId: "t" },
      { kind: "approval", callId: "t" },
      { kind: "steer", seq: steer.seq },
      { kind: "message", index: 1 },
    ]);
    expect(turn?.tools.get("t")).toMatchObject({ tool: "shell", summary: "ls -la", done: false });
    expect(runningTurn(state)?.turnId).toBe(TURN);
  });

  it("records the result of a tool call", () => {
    const state = run(
      envelope({
        kind: "tool_call_started",
        turn_id: TURN,
        call_id: "t",
        tool: "apply_patch",
        input: "*** Begin Patch",
      }),
      envelope({
        kind: "tool_call_completed",
        turn_id: TURN,
        call_id: "t",
        output: "",
        truncated: false,
        is_error: true,
        exit_code: -1,
        refusal: "efr's config (floor)",
        diff: "--- a/x\n+++ b/x\n",
        left_running: true,
      }),
    );
    expect(state.turns.get(TURN)?.tools.get("t")).toEqual({
      tool: "apply_patch",
      summary: "*** Begin Patch",
      done: true,
      isError: true,
      exitCode: -1,
      refusal: "efr's config (floor)",
      diff: "--- a/x\n+++ b/x\n",
      leftRunning: true,
    });
  });

  it("follows the context of the running turn and its compaction", () => {
    const draft = (part: unknown) => ({ kind: "draft", turn_id: TURN, after_seq: 1, draft: part });
    let state = run(
      envelope({ kind: "turn_started", turn_id: TURN }),
      draft({ kind: "context", tokens: 10, limit: 100, window: 200 }),
      draft({ kind: "compacting", trigger: "auto" }),
    );
    expect(state.context).toEqual({ tokens: 10, limit: 100, window: 200 });
    expect(state.compacting).toBe(true);
    state = reduceDetails(state, draft({ kind: "context", tokens: 4, limit: 100, window: 200 }));
    expect(state.compacting).toBe(false);
    state = reduceDetails(
      state,
      envelope({
        kind: "turn_failed",
        turn_id: TURN,
        error: { code: "internal", message: "The provider failed." },
        context: { tokens: 5, limit: 100, window: 200 },
      }),
    );
    expect(state.context?.tokens).toBe(5);
    expect(state.turns.get(TURN)?.end).toMatchObject({
      outcome: "failed",
      error: "The provider failed.",
    });
  });

  it("skips an event at or below the last sequence number", () => {
    const first = envelope({ kind: "prompt_queued", turn_id: TURN, text: "one", origin: "shell" });
    const state = run(first);
    expect(reduceDetails(state, first)).toBe(state);
    expect(reduceDetails(state, { ...first, seq: 0 })).toBe(state);
  });

  it("rebuilds from a snapshot and keeps what it knew before the window", () => {
    const before = run(
      envelope({
        kind: "prompt_queued",
        turn_id: TURN,
        text: "why is the disk full",
        origin: "shell",
      }),
      envelope({ kind: "turn_started", turn_id: TURN }),
      envelope({ kind: "assistant_message_completed", turn_id: TURN, index: 0, text: "a" }),
    );
    const after = reduceDetails(
      before,
      fixture(items, "conversation_subscribe_item_snapshot_live"),
    );
    const turn = after.turns.get(TURN);
    expect(turn?.prompt?.text).toBe("why is the disk full");
    expect(turn?.parts).toEqual([
      { kind: "message", index: 0 },
      { kind: "message", index: 1 },
    ]);
    expect(turn?.started).toBe(true);
    expect(after.lastSeq).toBe(Math.max(42, before.lastSeq));
  });

  it("ignores what is not an item", () => {
    for (const value of [null, 3, "x", [], { kind: "other" }, { kind: "event", seq: "1" }]) {
      expect(reduceDetails(EMPTY_DETAILS, value)).toBe(EMPTY_DETAILS);
    }
  });
});

describe("inputSummary", () => {
  it("names the command, the path or the first line", () => {
    expect(inputSummary("shell", { command: "cargo test\n--all" })).toBe("cargo test");
    expect(inputSummary("shell", { command: ["git", "status"] })).toBe("git status");
    expect(inputSummary("read_file", { path: "src/main.rs" })).toBe("src/main.rs");
    expect(inputSummary("apply_patch", "\n*** Begin Patch\n")).toBe("*** Begin Patch");
    expect(inputSummary("mystery", { n: 1 })).toBe("mystery");
    expect(inputSummary("shell", { command: "x".repeat(500) })).toHaveLength(200);
  });
});
