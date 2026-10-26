import { describe, expect, it } from "vitest";

import type { DraftPart, Event, SubscribeItem } from "@/lib/protocol/wire";
import { reduce } from "@/lib/stream/reducer";
import {
  approvalRequested,
  draftItem,
  eventItem,
  messageCompleted,
  promptQueued,
  run,
  textPart,
  toolCallStarted,
  turnCompleted,
  turnStarted,
} from "@/lib/stream/testing";
import { createRowBuilder, rowKindOf, sameRows } from "./rows";

const TURN = "t1";
let seq = 0;
const event = (body: Event): SubscribeItem => eventItem(++seq, body);
const draft = (part: DraftPart): SubscribeItem => draftItem(TURN, seq, part);

describe("createRowBuilder", () => {
  it("lists the parts of a turn oldest first, with its running line while it runs", () => {
    const build = createRowBuilder();
    const state = run([
      event(promptQueued(TURN, "hi", "phone")),
      event(turnStarted(TURN)),
      draft(textPart(0, 0, "Hello")),
      event(toolCallStarted(TURN, "c", "shell", { command: "ls" })),
      event(approvalRequested(TURN, "c", "run ls")),
      event({ kind: "turn_steered", turn_id: TURN, text: "and /tmp" }),
    ]);
    expect(build(state).map((row) => row.key)).toEqual([
      "p:t1",
      "m:t1:0",
      "t:t1:c",
      "a:t1:c",
      `s:t1:${seq}`,
      "w:t1",
    ]);
  });

  it("keeps the same list and rows while only the text grows", () => {
    const build = createRowBuilder();
    const before = run([event(turnStarted(TURN)), draft(textPart(0, 0, "Hey"))]);
    const rows = build(before);
    const after = reduce(before, draft(textPart(0, 3, " you")));
    expect(after.turns.get(TURN)).not.toBe(before.turns.get(TURN));
    expect(build(after)).toBe(rows);
  });

  it("builds only the rows of the turn that changed", () => {
    const build = createRowBuilder();
    const old = "t0";
    const before = run([
      event(turnStarted(old)),
      event(messageCompleted(old, 0, "Old.")),
      event(turnCompleted(old)),
      event(turnStarted(TURN)),
      draft(textPart(0, 0, "Hi")),
    ]);
    const first = build(before);
    const after = reduce(before, event(toolCallStarted(TURN, "c", "shell")));
    const second = build(after);
    expect(second).not.toBe(first);
    expect(second.slice(0, 2)).toEqual(first.slice(0, 2));
    expect(second[0]).toBe(first[0]);
    expect(second.map((row) => row.key)).toEqual(["m:t0:0", "e:t0", "m:t1:0", "t:t1:c", "w:t1"]);
  });

  it("keeps each row object when a row comes, and ends the running line at the turn's end", () => {
    const build = createRowBuilder();
    const items: SubscribeItem[] = [event(turnStarted(TURN)), draft(textPart(0, 0, "Hi"))];
    const first = build(run(items));
    items.push(event(messageCompleted(TURN, 0, "Hi")), event(turnCompleted(TURN)));
    const second = build(run(items));
    expect(second.map((row) => row.key)).toEqual(["m:t1:0", "e:t1"]);
    expect(second[0]).toBe(first.find((row) => row.key === "m:t1:0"));
  });

  it("shows the reasoning after the prompt, and no message before its first text", () => {
    const build = createRowBuilder();
    const rows = build(
      run([
        event(promptQueued(TURN, "think")),
        event(turnStarted(TURN)),
        draft({ kind: "reasoning", offset: 0, delta: "**Plan**" }),
        draft(textPart(0, 0, "")),
      ]),
    );
    expect(rows.map((row) => row.key)).toEqual(["p:t1", "r:t1", "w:t1"]);
  });

  it("shows a queued prompt without a running line", () => {
    const build = createRowBuilder();
    const rows = build(run([event(promptQueued(TURN, "next", "phone"))]));
    expect(rows.map((row) => row.key)).toEqual(["p:t1"]);
  });

  it("hides a steer that an interrupt sent again as a prompt", () => {
    const build = createRowBuilder();
    const state = run([
      event(turnStarted(TURN)),
      event({ kind: "turn_steered", turn_id: TURN, text: "also" }),
    ]);
    const steer = seq;
    const after = reduce(
      run([event({ kind: "turn_interrupted", turn_id: TURN })], state),
      event({ ...promptQueued("t2", "also"), steers: [steer] }),
    );
    expect(build(after).map((row) => row.key)).toEqual(["e:t1", "p:t2"]);
  });
});

describe("rowKindOf and sameRows", () => {
  it("reads the kind from the key, and compares row objects", () => {
    expect(rowKindOf("m:t:0")).toBe("message");
    expect(rowKindOf("w:t")).toBe("status");
    expect(rowKindOf("x:t")).toBeUndefined();
    const row = { kind: "end", key: "e:t", turnId: "t" } as const;
    expect(sameRows([row], [row])).toBe(true);
    expect(sameRows([row], [{ ...row }])).toBe(false);
  });
});
