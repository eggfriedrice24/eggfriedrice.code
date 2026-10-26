import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import {
  applyDraft,
  applyEvent,
  applySnapshot,
  EMPTY_STREAM,
  reduce,
  type StreamState,
} from "./reducer";
import { chunked, toolCallCompleted, toolCallStarted, turnStarted } from "./testing";
import type { DraftPart, Event, EventEnvelope, SubscribeItem } from "@/lib/protocol/wire";

const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";
const AT = "2026-10-03T12:00:05Z";

function event(seq: number, body: Event): SubscribeItem {
  return { kind: "event", seq, at: AT, event: body };
}

function envelope(seq: number, body: Event): EventEnvelope {
  return { seq, at: AT, event: body };
}

function draft(afterSeq: number, part: DraftPart, turnId = TURN): SubscribeItem {
  return { kind: "draft", turn_id: turnId, after_seq: afterSeq, draft: part };
}

function text(index: number, offset: number, delta: string): DraftPart {
  return { kind: "text", index, offset, delta };
}

function messageText(state: StreamState, index = 0, turnId = TURN): string | undefined {
  return state.turns.get(turnId)?.messages.get(index)?.buffer.text;
}

function run(items: readonly SubscribeItem[], from: StreamState = EMPTY_STREAM): StreamState {
  return items.reduce(reduce, from);
}

describe("messages", () => {
  it("merges drafts and updates of one message", () => {
    const state = run([
      event(1, turnStarted(TURN)),
      draft(1, text(0, 0, "The disk ")),
      event(2, {
        kind: "assistant_message_updated",
        turn_id: TURN,
        index: 0,
        offset: 0,
        delta: "The disk is ",
      }),
      draft(2, text(0, 9, "is full.")),
    ]);
    expect(messageText(state)).toBe("The disk is full.");
    expect(state.lastSeq).toBe(2);
    expect(state.order).toEqual([TURN]);
  });

  it("replaces the text with the completed text and counts a replace", () => {
    const appended = run([
      draft(0, text(0, 0, "Hello")),
      event(3, { kind: "assistant_message_completed", turn_id: TURN, index: 0, text: "Hello!" }),
    ]);
    const message = appended.turns.get(TURN)?.messages.get(0);
    expect(message?.buffer.text).toBe("Hello!");
    expect(message?.final).toBe(true);
    expect(message?.generation).toBe(0);

    const replaced = run([
      draft(0, text(0, 0, "Hi")),
      event(3, { kind: "assistant_message_completed", turn_id: TURN, index: 0, text: "Hello" }),
    ]);
    expect(replaced.turns.get(TURN)?.messages.get(0)?.generation).toBe(1);
  });

  it("drops drafts of a final message and of an ended turn", () => {
    const completed = run([
      event(3, { kind: "assistant_message_completed", turn_id: TURN, index: 0, text: "Done." }),
      draft(2, text(0, 5, " More")),
    ]);
    expect(messageText(completed)).toBe("Done.");

    const ended = run([
      draft(0, text(0, 0, "Half")),
      event(4, { kind: "turn_interrupted", turn_id: TURN }),
      draft(3, text(0, 4, " more")),
      draft(3, { kind: "reasoning", offset: 0, delta: "late" }),
    ]);
    const turn = ended.turns.get(TURN);
    expect(turn?.ended).toBe(true);
    expect(turn?.messages.get(0)?.final).toBe(true);
    expect(messageText(ended)).toBe("Half");
    expect(turn?.reasoning.text).toBe("");
  });

  it("skips duplicate events of a replay", () => {
    const update: Event = {
      kind: "assistant_message_updated",
      turn_id: TURN,
      index: 0,
      offset: 0,
      delta: "once",
    };
    const state = run([event(5, update), event(5, update), event(4, update)]);
    expect(messageText(state)).toBe("once");
  });

  it("keeps the messages of one turn apart by index", () => {
    const state = run([draft(0, text(0, 0, "first")), draft(0, text(1, 0, "second"))]);
    expect(messageText(state, 0)).toBe("first");
    expect(messageText(state, 1)).toBe("second");
  });

  it("returns the same state for an item that changes nothing", () => {
    const state = run([draft(0, text(0, 0, "abc"))]);
    expect(reduce(state, draft(0, text(0, 1, "bc")))).toBe(state);
    expect(reduce(state, { kind: "other", wireKind: "future" })).toBe(state);
  });

  it("advances the cursor over events it does not read", () => {
    const state = run([event(9, { kind: "other", wireKind: "cwd_changed" })]);
    expect(state.lastSeq).toBe(9);
    expect(state.turns.size).toBe(0);
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
  });

  it("clears tool input drafts when the call starts and drops older ones", () => {
    const input: DraftPart = { kind: "tool_input", call: 0, tool: "apply_patch", bytes: 2300 };
    const writing = run([draft(1, input)]);
    expect(writing.turns.get(TURN)?.toolInputs.get(0)).toEqual({
      tool: "apply_patch",
      bytes: 2300,
    });

    const started = run(
      [
        event(2, toolCallStarted(TURN, "call_1", "apply_patch")),
        draft(1, { ...input, bytes: 2400 }),
      ],
      writing,
    );
    const turn = started.turns.get(TURN);
    expect(turn?.toolInputs.size).toBe(0);
    expect(turn?.toolCalls.get("call_1")).toEqual({
      tool: "apply_patch",
      tail: "",
      bytes: 0,
      done: false,
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
        event(4, toolCallCompleted(TURN, "call_1", "ok\n")),
      ],
      started,
    );
    expect(done.turns.get(TURN)?.toolCalls.get("call_1")).toEqual({
      tool: "apply_patch",
      tail: "ok\n",
      bytes: 3,
      done: true,
    });
  });
});

describe("snapshots", () => {
  it("rebuilds from the events and seeds the live message", () => {
    const state = applySnapshot(EMPTY_STREAM, {
      events: [
        envelope(41, turnStarted(TURN)),
        envelope(42, {
          kind: "assistant_message_updated",
          turn_id: TURN,
          index: 0,
          offset: 12,
          delta: "a gap",
        }),
      ],
      hwm: 42,
      live: {
        turn_id: TURN,
        after_seq: 42,
        index: 0,
        text: "The journal a gap and more",
        text_bytes: 26,
        reasoning: "**Check**",
        reasoning_title: "Check",
      },
    });
    expect(state.lastSeq).toBe(42);
    expect(messageText(state)).toBe("The journal a gap and more");
    expect(state.turns.get(TURN)?.title).toBe("Check");
    expect(state.turns.get(TURN)?.reasoning.text).toBe("**Check**");
  });

  it("keeps the text the app held beyond the snapshot", () => {
    const held = run([draft(0, text(0, 0, "Hello wor")), draft(0, text(0, 9, "ld"))]);
    const state = applySnapshot(held, {
      events: [
        envelope(7, {
          kind: "assistant_message_updated",
          turn_id: TURN,
          index: 0,
          offset: 0,
          delta: "Hello",
        }),
      ],
      hwm: 7,
    });
    expect(messageText(state)).toBe("Hello world");
    expect(state.turns.get(TURN)?.messages.get(0)?.generation).toBe(0);
  });

  it("keeps older turns that the snapshot window does not hold", () => {
    const older = "019a9b1c-3d00-7a10-8b20-000000000001";
    const held = run([
      event(1, { kind: "assistant_message_completed", turn_id: older, index: 0, text: "Old." }),
    ]);
    const state = applySnapshot(held, {
      events: [envelope(10, turnStarted(TURN))],
      hwm: 10,
    });
    expect(state.order).toEqual([older, TURN]);
    expect(messageText(state, 0, older)).toBe("Old.");
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
              items.push(draft(seq, text(0, chunk.offset, chunk.delta)));
            }
            if (fate === "update" || fate === "both") {
              seq += 1;
              items.push(
                event(seq, {
                  kind: "assistant_message_updated",
                  turn_id: TURN,
                  index: 0,
                  offset: chunk.offset,
                  delta: chunk.delta,
                }),
              );
            }
          });
          let state = EMPTY_STREAM;
          for (const item of items) {
            state = reduce(state, item);
            expect(whole.startsWith(messageText(state) ?? "")).toBe(true);
          }
          state = reduce(
            state,
            event(seq + 1, {
              kind: "assistant_message_completed",
              turn_id: TURN,
              index: 0,
              text: whole,
            }),
          );
          state = reduce(state, event(seq + 2, { kind: "turn_completed", turn_id: TURN }));
          // A late draft after the end changes nothing.
          state = applyDraft(state, { turn_id: TURN, after_seq: seq, draft: text(0, 0, "x") });
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
          state = applyDraft(state, {
            turn_id: TURN,
            after_seq: 0,
            draft: text(0, chunk.offset, chunk.delta),
          });
        }
        expect(messageText(state) ?? "").toBe(whole);
      }),
    );
  });

  it("an event replayed after a resubscribe changes nothing", () => {
    fc.assert(
      fc.property(anyText, fc.array(fc.nat(), { maxLength: 20 }), (whole, cuts) => {
        const events = chunked(whole, cuts).map((chunk, i) =>
          envelope(i + 1, {
            kind: "assistant_message_updated",
            turn_id: TURN,
            index: 0,
            offset: chunk.offset,
            delta: chunk.delta,
          }),
        );
        const once = events.reduce(applyEvent, EMPTY_STREAM);
        const twice = events.reduce(applyEvent, once);
        expect(twice).toBe(once);
      }),
    );
  });
});
