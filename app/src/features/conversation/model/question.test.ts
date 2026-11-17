// The question of the model on the phone (S-Ask): the strip, the action sheet, the typed
// answer and the row of the call.

import { describe, expect, it } from "vitest";

import { CALL, TURN } from "@/lib/protocol/testing";
import type { UserQuestionView } from "@/lib/stream/reducer";
import {
  ANSWER_HINT,
  MAX_ANSWER_BYTES,
  answerText,
  questionChoice,
  questionRow,
  questionSheet,
  questionStrip,
} from "./question";

const QUESTION: UserQuestionView = {
  questionId: "q1",
  turnId: TURN,
  callId: CALL,
  question: "Which database\nshould the tests use?",
  options: [
    { label: "Postgres 16", description: "the same as production; I recommend it." },
    { label: "SQLite", description: null },
    { label: "both" },
  ],
  answer: { kind: "pending" },
  origin: undefined,
  askedAt: 1,
};

const ONLINE = { connected: true, sending: false, failed: false };

describe("questionStrip (S-Ask 1)", () => {
  it("shows the question on one line, with Answer enabled", () => {
    expect(questionStrip(QUESTION, ONLINE, { index: 0, count: 1 })).toEqual({
      questionId: "q1",
      callId: CALL,
      line: "Which database should the tests use?",
      label: "Question: Which database should the tests use?",
      meta: [],
      sending: false,
      enabled: true,
    });
  });

  it("counts the other waits, and says offline and not sent", () => {
    const strip = questionStrip(
      QUESTION,
      { connected: false, sending: false, failed: true },
      { index: 0, count: 2 },
    );
    expect(strip?.meta.map((meta) => meta.text)).toEqual(["not sent", "1 of 2", "offline"]);
    expect(strip?.enabled).toBe(false);
    const sending = questionStrip(QUESTION, { ...ONLINE, sending: true }, { index: 0, count: 1 });
    expect([sending?.sending, sending?.enabled]).toEqual([true, false]);
  });

  it("is none once the question ended", () => {
    const skipped: UserQuestionView = { ...QUESTION, answer: { kind: "skipped" } };
    expect(questionStrip(skipped, ONLINE, { index: 0, count: 1 })).toBeUndefined();
  });
});

describe("questionSheet (S-Ask 2)", () => {
  it("gives the question as the title, the options as the message and the actions", () => {
    const sheet = questionSheet(QUESTION);
    expect(sheet.title).toBe(QUESTION.question);
    expect(sheet.message).toBe(
      "Postgres 16: the same as production; I recommend it.\nSQLite\nboth",
    );
    expect(sheet.actions.map((action) => action.label)).toEqual([
      "Postgres 16",
      "SQLite",
      "both",
      "Type an answer",
      "Skip",
    ]);
    expect(sheet.actions.some((action) => action.destructive === true)).toBe(false);
    expect(sheet.cancelLabel).toBeUndefined();
  });

  it("reads each pick", () => {
    const ids = questionSheet(QUESTION).actions.map((action) => action.id);
    expect(ids.map(questionChoice)).toEqual([
      { kind: "reply", reply: { kind: "option", index: 0 } },
      { kind: "reply", reply: { kind: "option", index: 1 } },
      { kind: "reply", reply: { kind: "option", index: 2 } },
      { kind: "type" },
      { kind: "reply", reply: { kind: "skipped" } },
    ]);
    expect(questionChoice(undefined)).toBeUndefined();
    expect(questionChoice("option:")).toBeUndefined();
    expect(questionChoice("other")).toBeUndefined();
  });
});

describe("answerText (S-Ask 3)", () => {
  it("makes one line of the typed text and trims it", () => {
    expect(answerText("  Postgres 16,\nbut on port 5433 \t")).toBe("Postgres 16, but on port 5433");
    expect(answerText("a\u0007b")).toBe("a b");
  });

  it("refuses an empty answer and one over 1024 bytes", () => {
    expect(answerText("")).toBeUndefined();
    expect(answerText(" \n ")).toBeUndefined();
    expect(answerText("é".repeat(MAX_ANSWER_BYTES / 2))).toHaveLength(MAX_ANSWER_BYTES / 2);
    expect(answerText(`${"é".repeat(MAX_ANSWER_BYTES / 2)}x`)).toBeUndefined();
  });

  it("has the fixed hint of both clients", () => {
    expect(ANSWER_HINT).toBe("Do not type a password or a token here.");
  });
});

/** The row of QUESTION after `answer`, on the machine `desktop`. */
const row = (
  answer: UserQuestionView["answer"],
  origin?: UserQuestionView["origin"],
  sending = false,
) => questionRow({ ...QUESTION, answer, origin }, "desktop", sending);

describe("questionRow (S-Ask 4)", () => {
  it("waits, then sends", () => {
    expect(row({ kind: "pending" })).toEqual({
      question: "Which database should the tests use?",
      state: "waiting for you",
      waiting: true,
    });
    expect(row({ kind: "pending" }, undefined, true).state).toBe("sending…");
  });

  it("says how it ended", () => {
    expect(row({ kind: "option", index: 0, label: "Postgres 16" }, "phone")).toEqual({
      question: "Which database should the tests use?",
      state: "answered · Postgres 16",
      waiting: false,
    });
    expect(row({ kind: "text", text: "Postgres 16,\non port 5433" }, "phone").state).toBe(
      'answered · "Postgres 16, on port 5433"',
    );
    expect(row({ kind: "option", index: 1, label: "SQLite" }, "shell").state).toBe(
      "answered on desktop · SQLite",
    );
    expect(row({ kind: "skipped" }, "phone").state).toBe("skipped");
    expect(row({ kind: "unanswered", away: true }).state).toBe("nobody answered");
    expect(row({ kind: "interrupted" }).state).toBe("not answered · the turn was interrupted");
  });
});
