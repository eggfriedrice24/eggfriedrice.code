import { describe, expect, it } from "vitest";

import { item } from "./testing";
import { waitsElsewhere, manyWaitingText, showsWaitDot, waitingWords, waitWord } from "./waiting";

/** A conversation `id` that waits in `state`. */
function wait(id: string, state: "approval" | "answer" | "terminal") {
  return { machine: "d", id, state };
}

describe("waitsElsewhere", () => {
  it("is none when nothing waits", () => {
    const result = waitsElsewhere({
      current: undefined,
      live: [{ machine: "d", id: "a", pendingApprovals: 0 }],
      listed: [item("d", "b", 1, { status: "running" })],
    });
    expect(result).toEqual({ kind: "none" });
    expect(showsWaitDot(result)).toBe(false);
  });

  it("gives one waiting conversation from a polled list", () => {
    const result = waitsElsewhere({
      current: { machine: "d", id: "here" },
      live: [],
      listed: [
        item("d", "here", 9, { status: "awaiting_approval" }),
        item("l", "steer", 5, { status: "awaiting_approval", title: "Fix steer race" }),
      ],
    });
    expect(result).toEqual({
      kind: "one",
      conversation: { machine: "l", id: "steer", title: "Fix steer race", state: "approval" },
    });
    expect(showsWaitDot(result)).toBe(true);
  });

  it("never counts the conversation on screen", () => {
    const result = waitsElsewhere({
      current: { machine: "d", id: "a" },
      live: [{ machine: "d", id: "a", pendingApprovals: 2 }],
      listed: [item("d", "a", 1, { status: "awaiting_approval" })],
    });
    expect(result).toEqual({ kind: "none" });
  });

  it("counts the same id on another machine as another conversation", () => {
    const result = waitsElsewhere({
      current: { machine: "d", id: "a" },
      live: [],
      listed: [item("l", "a", 1, { status: "awaiting_approval" })],
    });
    expect(result.kind).toBe("one");
  });

  it("trusts a live stream over a polled list that is behind", () => {
    const answered = waitsElsewhere({
      current: undefined,
      live: [{ machine: "d", id: "a", pendingApprovals: 0 }],
      listed: [item("d", "a", 1, { status: "awaiting_approval" })],
    });
    expect(answered).toEqual({ kind: "none" });
    const asked = waitsElsewhere({
      current: undefined,
      live: [{ machine: "d", id: "a", title: "Live title", pendingApprovals: 1 }],
      listed: [item("d", "a", 1, { status: "running", title: "Old title" })],
    });
    expect(asked).toEqual({
      kind: "one",
      conversation: { machine: "d", id: "a", title: "Live title", state: "approval" },
    });
  });

  it("counts conversations, the polled ones newest first, then the live ones they lack", () => {
    const result = waitsElsewhere({
      current: { machine: "d", id: "here" },
      live: [
        { machine: "d", id: "stacked", pendingApprovals: 3 },
        { machine: "d", id: "old", pendingApprovals: 1 },
      ],
      listed: [
        item("d", "old", 1, { status: "awaiting_approval" }),
        item("l", "new", 7, { status: "awaiting_approval" }),
        item("l", "new", 7, { status: "awaiting_approval" }),
      ],
    });
    expect(result).toEqual({
      kind: "many",
      count: 3,
      conversations: [
        { machine: "l", id: "new", state: "approval" },
        { machine: "d", id: "old", state: "approval" },
        { machine: "d", id: "stacked", state: "approval" },
      ],
    });
    expect(waitingWords(result)).toBe("3 approvals waiting");
  });

  it("counts a question of the model and a wait for the terminal, with their words", () => {
    const listed = [
      item("d", "ask", 3, { status: "running", attention: "answer" }),
      item("d", "pw", 2, { status: "running", attention: "terminal" }),
      item("l", "local", 1, { status: "awaiting_approval", attention: "terminal" }),
      item("l", "ran", 4, { status: "running" }),
      item("l", "failed", 5, { status: "idle", attention: "failed" }),
    ];
    const one = (state: "answer" | "terminal") =>
      waitsElsewhere({
        current: undefined,
        live: [],
        listed: listed.filter((row) => row.attention === state),
      });
    const question = one("answer");
    expect(question).toEqual({
      kind: "one",
      conversation: { machine: "d", id: "ask", state: "answer" },
    });
    expect(waitWord("answer")).toBe("Question waiting");
    expect(waitingWords(question)).toBe("question waiting");
    const terminal = waitsElsewhere({
      current: undefined,
      live: [],
      listed: listed.slice(1, 2),
    });
    expect(waitingWords(terminal)).toBe("answer in the terminal");
    expect(one("terminal")).toMatchObject({ kind: "many", count: 2 });

    const all = waitsElsewhere({ current: undefined, live: [], listed });
    expect(all).toMatchObject({ kind: "many", count: 3 });
    expect(waitingWords(all)).toBe("3 conversations need you");
  });

  it("gives the text of a count of one kind and of a mixed count", () => {
    expect(manyWaitingText([wait("a", "approval"), wait("b", "approval")])).toBe(
      "2 approvals waiting",
    );
    expect(manyWaitingText([wait("a", "answer"), wait("b", "answer")])).toBe("2 questions waiting");
    expect(manyWaitingText([wait("a", "approval"), wait("b", "answer")])).toBe(
      "2 conversations need you",
    );
    // A wait for the terminal counts as mixed: a password wait is no approval.
    expect(manyWaitingText([wait("a", "terminal"), wait("b", "terminal")])).toBe(
      "2 conversations need you",
    );
  });
});
