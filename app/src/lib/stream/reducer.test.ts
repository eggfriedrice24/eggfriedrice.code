/// <reference types="vite/client" />
import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { CALL, fixtureEvent, item, rawEvent, rawItem, TURN } from "@/lib/protocol/testing";
import type { DraftPart, Event, SubscribeItem } from "@/lib/protocol/wire";
import {
  EMPTY_STREAM,
  findApproval,
  inputSummary,
  isQueued,
  pendingApprovals,
  reduce,
  runningTurn,
  type StreamState,
  underlay,
  withLink,
} from "./reducer";
import {
  approvalRequested,
  approvalResolved,
  AT_MS,
  at,
  chunked,
  draftItem,
  envelope,
  eventItem,
  messageCompleted,
  messageUpdated,
  promptQueued,
  run,
  snapshotItem,
  textPart,
  toolCallCompleted,
  toolCallStarted,
  turnCompleted,
  turnStarted,
} from "./testing";

const CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000000001";

const events = import.meta.glob<unknown>(
  "../../../../crates/efr-protocol/fixtures/v1/events/*.json",
  { eager: true, import: "default" },
);

function draft(afterSeq: number, part: DraftPart, turnId = TURN): SubscribeItem {
  return draftItem(turnId, afterSeq, part);
}

function event(seq: number, body: Event, time?: string): SubscribeItem {
  return eventItem(seq, body, time);
}

function messageText(state: StreamState, index = 0, turnId = TURN): string | undefined {
  return state.turns.get(turnId)?.messages.get(index)?.buffer.text;
}

describe("messages", () => {
  it("merges drafts and updates of one message", () => {
    const state = run([
      event(1, turnStarted(TURN)),
      draft(1, textPart(0, 0, "The disk ")),
      event(2, messageUpdated(TURN, 0, 0, "The disk is ")),
      draft(2, textPart(0, 9, "is full.")),
    ]);
    expect(messageText(state)).toBe("The disk is full.");
    expect(state.lastSeq).toBe(2);
    expect(state.order).toEqual([TURN]);
    expect(state.loaded).toBe(true);
  });

  it("replaces the text with the completed text and counts a replace", () => {
    const appended = run([
      draft(0, textPart(0, 0, "Hello")),
      event(3, messageCompleted(TURN, 0, "Hello!")),
    ]);
    const message = appended.turns.get(TURN)?.messages.get(0);
    expect(message?.buffer.text).toBe("Hello!");
    expect(message?.final).toBe(true);
    expect(message?.generation).toBe(0);

    const replaced = run([
      draft(0, textPart(0, 0, "Hi")),
      event(3, messageCompleted(TURN, 0, "Hello")),
    ]);
    expect(replaced.turns.get(TURN)?.messages.get(0)?.generation).toBe(1);
  });

  it("drops drafts of a final message and of an ended turn", () => {
    const completed = run([
      event(3, messageCompleted(TURN, 0, "Done.")),
      draft(2, textPart(0, 5, " More")),
    ]);
    expect(messageText(completed)).toBe("Done.");

    const ended = run([
      draft(0, textPart(0, 0, "Half")),
      event(4, { kind: "turn_interrupted", turn_id: TURN }),
      draft(3, textPart(0, 4, " more")),
      draft(3, { kind: "reasoning", offset: 0, delta: "late" }),
    ]);
    const turn = ended.turns.get(TURN);
    expect(turn?.end?.outcome).toBe("interrupted");
    expect(turn?.messages.get(0)?.final).toBe(true);
    expect(messageText(ended)).toBe("Half");
    expect(turn?.reasoning.text).toBe("");
  });

  it("skips duplicate events of a replay", () => {
    const update = messageUpdated(TURN, 0, 0, "once");
    const state = run([event(5, update), event(5, update), event(4, update)]);
    expect(messageText(state)).toBe("once");
  });

  it("keeps the messages of one turn apart by index", () => {
    const state = run([draft(0, textPart(0, 0, "first")), draft(0, textPart(1, 0, "second"))]);
    expect(messageText(state, 0)).toBe("first");
    expect(messageText(state, 1)).toBe("second");
  });

  it("returns the same state for an item that changes nothing", () => {
    const state = run([draft(0, textPart(0, 0, "abc"))]);
    expect(reduce(state, draft(0, textPart(0, 1, "bc")))).toBe(state);
    expect(reduce(state, { kind: "other", wireKind: "future" })).toBe(state);
  });

  it("advances the cursor over events it does not read, and keeps the turns", () => {
    const before = run([event(1, turnStarted(TURN))]);
    const state = reduce(before, event(9, { kind: "other", wireKind: "cwd_changed" }));
    expect(state.lastSeq).toBe(9);
    expect(state.turns).toBe(before.turns);
    const ignored = reduce(state, event(10, fixtureEvent("shell_started", 10).event));
    expect(ignored.turns).toBe(before.turns);
  });
});

describe("reasoning and tools", () => {
  it("appends reasoning and keeps the newest title", () => {
    const state = run([
      draft(0, { kind: "reasoning", offset: 0, delta: "**Plan**\n" }),
      draft(0, { kind: "reasoning", offset: 9, delta: "look at du", title: "Plan" }),
      draft(0, { kind: "reasoning", offset: 19, delta: " output" }),
    ]);
    const turn = state.turns.get(TURN);
    expect(turn?.reasoning.text).toBe("**Plan**\nlook at du output");
    expect(turn?.title).toBe("Plan");
    expect(turn?.started).toBe(true);
  });

  it("marks the end of the first thought at the first answer after reasoning", () => {
    const thinking = [
      event(1, promptQueued(TURN, "Go.")),
      event(2, turnStarted(TURN), at(0)),
      draft(2, { kind: "reasoning", offset: 0, delta: "Checking 97 first." }),
    ];
    expect(run(thinking).turns.get(TURN)?.thoughtEndedAt).toBeUndefined();
    const answered = run([
      ...thinking,
      event(3, messageUpdated(TURN, 0, 0, "Only 97"), at(9)),
      event(4, messageUpdated(TURN, 0, 7, " is prime."), at(10)),
    ]);
    expect(answered.turns.get(TURN)?.thoughtEndedAt).toBe(AT_MS + 9_000);
    // No reasoning, no thought to end.
    const plain = run([
      event(1, promptQueued(TURN, "Go.")),
      event(2, turnStarted(TURN), at(0)),
      event(3, messageUpdated(TURN, 0, 0, "Hi"), at(1)),
    ]);
    expect(plain.turns.get(TURN)?.thoughtEndedAt).toBeUndefined();
  });

  it("clears tool input drafts when the call starts, drops older ones, and records the result", () => {
    const input: DraftPart = { kind: "tool_input", call: 0, tool: "apply_patch", bytes: 2300 };
    const writing = run([draft(1, input)]);
    expect(writing.turns.get(TURN)?.toolInputs.get(0)).toEqual({
      tool: "apply_patch",
      bytes: 2300,
    });

    const started = run(
      [
        event(2, toolCallStarted(TURN, "call_1", "shell", { command: "ls -la\nmore" }), at(1)),
        draft(1, { ...input, bytes: 2400 }),
      ],
      writing,
    );
    const turn = started.turns.get(TURN);
    expect(turn?.toolInputs.size).toBe(0);
    expect(turn?.tools.get("call_1")).toMatchObject({
      tool: "shell",
      summary: "ls -la",
      tail: "",
      bytes: 0,
      done: false,
      startedAt: AT_MS + 1000,
    });

    const done = run(
      [
        event(3, {
          kind: "tool_call_output_updated",
          turn_id: TURN,
          call_id: "call_1",
          tail: "ok\n",
          bytes: 3,
        }),
        event(
          4,
          {
            ...toolCallCompleted(TURN, "call_1", "ok\n"),
            is_error: true,
            exit_code: -1,
            refusal: "efr's config (floor)",
            diff: "--- a/x\n+++ b/x\n",
            left_running: true,
          },
          at(39),
        ),
      ],
      started,
    );
    expect(done.turns.get(TURN)?.tools.get("call_1")).toEqual({
      callId: "call_1",
      tool: "shell",
      summary: "ls -la",
      tail: "ok\n",
      bytes: 3,
      done: true,
      isError: true,
      exitCode: -1,
      output: "ok\n",
      refusal: "efr's config (floor)",
      diff: "--- a/x\n+++ b/x\n",
      leftRunning: true,
      startedAt: AT_MS + 1000,
      endedAt: AT_MS + 39_000,
    });
  });

  it("keeps the turn object when an output update repeats the same tail", () => {
    const update: Event = {
      kind: "tool_call_output_updated",
      turn_id: TURN,
      call_id: "c",
      tail: "x",
      bytes: 1,
    };
    const state = run([event(1, toolCallStarted(TURN, "c", "shell")), event(2, update)]);
    const again = reduce(state, event(3, update));
    expect(again.turns).toBe(state.turns);
    expect(again.lastSeq).toBe(3);
  });
});

describe("prompts, steers and turn ends", () => {
  it("keeps the parts of a turn in the order they came", () => {
    const state = run([
      event(1, promptQueued(TURN, "hi", "phone"), at(0)),
      event(2, turnStarted(TURN), at(1)),
      draft(2, textPart(0, 0, "a")),
      event(3, toolCallStarted(TURN, "t", "shell", { command: "ls" })),
      event(4, approvalRequested(TURN, "t", "run ls")),
      event(5, { kind: "turn_steered", turn_id: TURN, text: "also check /tmp" }, at(5)),
      event(6, messageCompleted(TURN, 1, "b")),
      event(7, messageCompleted(TURN, 0, "a")),
    ]);
    const turn = state.turns.get(TURN);
    expect(turn?.prompt).toEqual({
      text: "hi",
      origin: "phone",
      commandId: `cmd-${TURN}`,
      at: AT_MS,
    });
    expect(turn?.parts).toEqual([
      { kind: "message", index: 0 },
      { kind: "tool", callId: "t" },
      { kind: "approval", callId: "t" },
      { kind: "steer", seq: 5 },
      { kind: "message", index: 1 },
    ]);
    expect(turn?.steers.get(5)).toEqual({
      seq: 5,
      text: "also check /tmp",
      at: AT_MS + 5000,
      status: "sent",
      resent: false,
    });
    expect(turn?.startedAt).toBe(AT_MS + 1000);
    expect(turn?.cwd).toBe("/home/me");
    expect(runningTurn(state)?.turnId).toBe(TURN);
  });

  it("tells a queued prompt, a running turn and a withdrawn prompt apart", () => {
    const queued = run([event(1, promptQueued(TURN, "next", "phone"))]);
    const turn = queued.turns.get(TURN);
    expect(turn !== undefined && isQueued(turn)).toBe(true);
    expect(runningTurn(queued)).toBeUndefined();
    const withdrawn = reduce(
      queued,
      event(2, { kind: "prompt_withdrawn", turn_id: TURN, origin: "phone" }),
    );
    expect(withdrawn.turns.get(TURN)?.end?.outcome).toBe("withdrawn");
  });

  it("follows each steer: delivered, withdrawn, and resent as a prompt", () => {
    const next = "019a9b1c-3d00-7a10-8b20-000000000099";
    const state = run([
      event(1, turnStarted(TURN)),
      event(2, { kind: "turn_steered", turn_id: TURN, text: "one" }),
      event(3, { kind: "turn_steered", turn_id: TURN, text: "two" }),
      event(4, { kind: "turn_steered", turn_id: TURN, text: "three" }),
      event(5, { kind: "steering_delivered", turn_id: TURN, steers: [2] }),
      event(6, { kind: "steering_withdrawn", turn_id: TURN, steers: [3], origin: "phone" }),
      event(7, { kind: "turn_interrupt_requested", turn_id: TURN, origin: "phone" }),
      event(8, { kind: "turn_interrupted", turn_id: TURN }),
      event(9, { ...promptQueued(next, "four"), steers: [4] }),
    ]);
    const steers = state.turns.get(TURN)?.steers;
    expect([...(steers?.values() ?? [])].map((steer) => [steer.status, steer.resent])).toEqual([
      ["delivered", false],
      ["withdrawn", false],
      ["sent", true],
    ]);
    expect(state.turns.get(TURN)?.interruptedBy).toBe("phone");
  });

  it("records how a turn ended, with its context, its error and its stop", () => {
    const failed = run([
      event(1, turnStarted(TURN)),
      draft(1, { kind: "context", tokens: 10, limit: 100, window: 200 }),
      draft(1, { kind: "compacting", trigger: "auto" }),
    ]);
    expect(failed.context).toEqual({ tokens: 10, limit: 100, window: 200 });
    expect(failed.compacting).toBe(true);
    const lighter = reduce(
      failed,
      draft(1, { kind: "context", tokens: 4, limit: 100, window: 200 }),
    );
    expect(lighter.compacting).toBe(false);
    expect(reduce(lighter, draft(1, { kind: "context", tokens: 4, limit: 100, window: 200 }))).toBe(
      lighter,
    );
    const ended = reduce(
      lighter,
      event(
        2,
        {
          kind: "turn_failed",
          turn_id: TURN,
          error: { code: "internal", message: "The provider failed." },
          context: { tokens: 5, limit: 100, window: 200 },
        },
        at(22),
      ),
    );
    expect(ended.context?.tokens).toBe(5);
    expect(ended.turns.get(TURN)?.end).toEqual({
      outcome: "failed",
      at: AT_MS + 22_000,
      context: { tokens: 5, limit: 100, window: 200 },
      error: "The provider failed.",
      stop: undefined,
      usage: undefined,
    });

    const stopped = run([
      event(1, { ...turnCompleted(TURN), stop: { reason: "max_tokens" } }),
      event(2, { ...turnCompleted("other"), stop: { reason: "end_turn" } }),
    ]);
    expect(stopped.turns.get(TURN)?.end?.stop).toBe("max_tokens");
    expect(stopped.turns.get("other")?.end?.stop).toBeUndefined();
  });
});

describe("approvals", () => {
  it("holds a question with its facts, from the fixtures", () => {
    const state = run([item(rawEvent("approval_requested", 1))]);
    expect(findApproval(state, CALL)).toMatchObject({
      summary: "run `journalctl --vacuum-size=500M` as root",
      interactive: true,
      localOnly: false,
      status: "pending",
      modelReason: "the user asked for the report in Documents",
      askedAt: Date.parse("2026-10-26T12:00:00Z"),
      expiresAt: Date.parse("2026-10-26T12:02:00Z"),
    });
    expect(findApproval(state, CALL)?.diffPreview).toContain("+SystemMaxUse=500M");
    expect(pendingApprovals(state)).toHaveLength(1);

    const local = run([item(rawEvent("approval_requested_local_only", 1))]);
    expect(pendingApprovals(local)[0]?.localOnly).toBe(true);

    const reason = run([item(rawEvent("approval_requested_reason", 1))]);
    expect(pendingApprovals(reason)[0]?.reason).toEqual(expect.any(String));
  });

  it("records each answer, who gave it and when; the first answer wins", () => {
    for (const [decision, status] of [
      ["deny", "denied"],
      ["allow", "allowed"],
      ["allow_always", "allowed"],
    ] as const) {
      const state = run([
        event(1, approvalRequested(TURN, "a")),
        event(2, approvalResolved(TURN, "a", decision, "shell"), at(3)),
        event(3, { kind: "approval_expired", turn_id: TURN, call_id: "a" }),
      ]);
      expect(findApproval(state, "a")).toMatchObject({
        status,
        decision,
        answeredBy: "shell",
        closedAt: AT_MS + 3000,
      });
      expect(pendingApprovals(state)).toEqual([]);
    }
  });

  it("tells an expiry from an expiry while away", () => {
    const state = run([
      event(1, approvalRequested(TURN, "b")),
      event(2, approvalRequested(TURN, "c")),
      event(3, { kind: "approval_expired", turn_id: TURN, call_id: "b" }),
      event(4, { kind: "approval_expired", turn_id: TURN, call_id: "c", away: true }),
    ]);
    expect(findApproval(state, "b")?.status).toBe("expired");
    expect(findApproval(state, "c")?.status).toBe("expired_away");
  });

  it("expires a question that is open when its turn ends", () => {
    const state = run([
      event(1, turnStarted(TURN)),
      event(2, approvalRequested(TURN, "a")),
      event(3, { kind: "turn_interrupted", turn_id: TURN }, at(8)),
    ]);
    expect(findApproval(state, "a")).toMatchObject({ status: "expired", closedAt: AT_MS + 8000 });
    expect(runningTurn(state)).toBeUndefined();
  });

  it("ignores an answer to a question it never saw", () => {
    const state = run([event(1, approvalResolved(TURN, "x", "allow"))]);
    expect(state.turns.get(TURN)?.approvals.size).toBe(0);
  });

  it("settles a pending question that a snapshot misses, unless the conversation waits", () => {
    const held = run([event(1, approvalRequested(TURN, CALL))]);
    const waiting = reduce(held, snapshotItem(CONVERSATION, "awaiting_approval", [], 40));
    expect(findApproval(waiting, CALL)?.status).toBe("pending");
    expect(waiting.lastSeq).toBe(40);

    const idle = reduce(held, snapshotItem(CONVERSATION, "idle", [], 40));
    expect(findApproval(idle, CALL)?.status).toBe("settled");

    const answered = reduce(
      held,
      snapshotItem(
        CONVERSATION,
        "idle",
        [
          envelope(30, approvalRequested(TURN, CALL)),
          envelope(31, approvalResolved(TURN, CALL, "deny")),
        ],
        40,
      ),
    );
    expect(findApproval(answered, CALL)).toMatchObject({ status: "denied", answeredBy: "phone" });
  });
});

describe("snapshots", () => {
  it("rebuilds from the events, seeds the live message and keeps the summary", () => {
    const state = reduce(
      EMPTY_STREAM,
      snapshotItem(
        CONVERSATION,
        "running",
        [envelope(41, turnStarted(TURN)), envelope(42, messageUpdated(TURN, 0, 12, "a gap"))],
        42,
        {
          turn_id: TURN,
          after_seq: 42,
          index: 0,
          text: "The journal a gap and more",
          text_bytes: 26,
          reasoning: "**Check**",
          reasoning_title: "Check",
        },
      ),
    );
    expect(state.lastSeq).toBe(42);
    expect(messageText(state)).toBe("The journal a gap and more");
    expect(state.turns.get(TURN)?.title).toBe("Check");
    expect(state.turns.get(TURN)?.reasoning.text).toBe("**Check**");
    expect(state.summary?.status).toBe("running");
  });

  it("keeps the text the app held beyond the snapshot", () => {
    const held = run([draft(0, textPart(0, 0, "Hello wor")), draft(0, textPart(0, 9, "ld"))]);
    const state = reduce(
      held,
      snapshotItem(CONVERSATION, "running", [envelope(7, messageUpdated(TURN, 0, 0, "Hello"))], 7),
    );
    expect(messageText(state)).toBe("Hello world");
    expect(state.turns.get(TURN)?.messages.get(0)?.generation).toBe(0);
  });

  it("keeps older turns, the prompt and the first parts that the window does not hold", () => {
    const older = "019a9b1c-3d00-7a10-8b20-000000000003";
    const held = run([
      event(1, messageCompleted(older, 0, "Old.")),
      event(2, promptQueued(TURN, "why is the disk full")),
      event(3, turnStarted(TURN)),
      event(4, messageCompleted(TURN, 0, "a")),
    ]);
    const state = reduce(held, item(rawItem("snapshot_live")));
    expect(state.order).toEqual([older, TURN]);
    expect(messageText(state, 0, older)).toBe("Old.");
    const turn = state.turns.get(TURN);
    expect(turn?.prompt?.text).toBe("why is the disk full");
    expect(turn?.parts).toEqual([
      { kind: "message", index: 0 },
      { kind: "message", index: 1 },
    ]);
    expect(turn?.started).toBe(true);
    expect(state.lastSeq).toBe(42);
  });

  it("keeps the link state across a snapshot", () => {
    const live = withLink(EMPTY_STREAM, "live");
    expect(withLink(live, "live")).toBe(live);
    expect(reduce(live, snapshotItem(CONVERSATION, "idle", [], 3)).link).toBe("live");
  });
});

describe("underlay", () => {
  it("puts cached turns under what the subscription gave, and changes nothing without them", () => {
    const older = "019a9b1c-3d00-7a10-8b20-000000000003";
    const current = run([event(10, turnStarted(TURN)), draft(10, textPart(0, 0, "Hi"))]);
    expect(underlay(current, [])).toBe(current);
    const state = underlay(current, [
      envelope(1, messageCompleted(older, 0, "Old.")),
      envelope(2, turnStarted(TURN)),
    ]);
    expect(state.order).toEqual([older, TURN]);
    expect(messageText(state)).toBe("Hi");
    expect(state.lastSeq).toBe(10);

    const cold = underlay(EMPTY_STREAM, [envelope(1, messageCompleted(older, 0, "Old."))]);
    expect(cold.lastSeq).toBe(1);
    expect(cold.loaded).toBe(true);
  });
});

describe("the history cursor", () => {
  it("comes from the last snapshot, and a page of history under it keeps it", () => {
    expect(EMPTY_STREAM.historyCursor).toBeUndefined();
    const window = snapshotItem(CONVERSATION, "idle", [envelope(5, turnStarted(TURN))], 5);
    if (window.kind !== "snapshot") {
      throw new Error("not a snapshot");
    }
    const windowed = reduce(EMPTY_STREAM, { ...window, history_cursor: "c5" });
    expect(windowed.historyCursor).toBe("c5");
    const older = "019a9b1c-3d00-7a10-8b20-000000000003";
    const paged = underlay(windowed, [envelope(1, messageCompleted(older, 0, "Old."))]);
    expect(paged.order).toEqual([older, TURN]);
    expect(paged.historyCursor).toBe("c5");
    // A window that starts at the first event has no older history.
    expect(reduce(EMPTY_STREAM, snapshotItem(CONVERSATION, "idle", [], 0)).historyCursor).toBe(
      null,
    );
  });
});

describe("the fixtures", () => {
  it("reads every event fixture without a throw", () => {
    let state = EMPTY_STREAM;
    let seq = 0;
    for (const path of Object.keys(events).sort()) {
      seq += 1;
      const name = path.slice(path.lastIndexOf("/") + 1, -".json".length);
      state = reduce(state, item(rawEvent(name, seq)));
    }
    expect(state.lastSeq).toBe(seq);
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

describe("properties", () => {
  const anyText = fc.string({ unit: "binary", maxLength: 300 });

  it("any chunking and any drop pattern ends with the completed text", () => {
    fc.assert(
      fc.property(
        anyText,
        fc.array(fc.nat(), { maxLength: 40 }),
        fc.array(fc.constantFrom("draft", "update", "both", "drop"), { maxLength: 40 }),
        (whole, cuts, fates) => {
          const items: SubscribeItem[] = [event(1, turnStarted(TURN))];
          let seq = 1;
          chunked(whole, cuts).forEach((chunk, i) => {
            const fate = fates[i] ?? "both";
            if (fate === "draft" || fate === "both") {
              items.push(draft(seq, textPart(0, chunk.offset, chunk.delta)));
            }
            if (fate === "update" || fate === "both") {
              seq += 1;
              items.push(event(seq, messageUpdated(TURN, 0, chunk.offset, chunk.delta)));
            }
          });
          let state = EMPTY_STREAM;
          for (const next of items) {
            state = reduce(state, next);
            expect(whole.startsWith(messageText(state) ?? "")).toBe(true);
          }
          state = reduce(state, event(seq + 1, messageCompleted(TURN, 0, whole)));
          state = reduce(state, event(seq + 2, turnCompleted(TURN)));
          // A late draft after the end changes nothing.
          expect(reduce(state, draft(seq, textPart(0, 0, "x")))).toBe(state);
          expect(messageText(state)).toBe(whole);
          expect(state.turns.get(TURN)?.messages.get(0)?.final).toBe(true);
        },
      ),
    );
  });

  it("a stream without drops shows the whole text before the completed event", () => {
    fc.assert(
      fc.property(anyText, fc.array(fc.nat(), { maxLength: 40 }), (whole, cuts) => {
        let state = EMPTY_STREAM;
        for (const chunk of chunked(whole, cuts)) {
          state = reduce(state, draft(0, textPart(0, chunk.offset, chunk.delta)));
        }
        expect(messageText(state) ?? "").toBe(whole);
      }),
    );
  });

  it("an event replayed after a resubscribe changes nothing", () => {
    fc.assert(
      fc.property(anyText, fc.array(fc.nat(), { maxLength: 20 }), (whole, cuts) => {
        const updates = chunked(whole, cuts).map((chunk, i) =>
          event(i + 1, messageUpdated(TURN, 0, chunk.offset, chunk.delta)),
        );
        const once = run(updates);
        expect(run(updates, once)).toBe(once);
      }),
    );
  });
});
