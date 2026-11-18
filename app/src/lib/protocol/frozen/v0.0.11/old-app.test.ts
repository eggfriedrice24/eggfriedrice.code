/// <reference types="vite/client" />
// The app of 0.0.11, which users still run, against the fixtures of this efrd.
//
// generated.ts and generated-zod.ts in this folder are the files of tag v0.0.11, as they
// are: never generate them again. The test pins their SHA-256, so a change of either one
// fails it. A released app reads a list with closed sets of values, and one bad row
// fails its whole watch or subscribe snapshot (spec r012, section 2.1). So each change
// of the protocol must stay additive for these schemas: this test decodes each fixture
// that such an app can receive, with the rules of its decoder (envelope.ts of v0.0.11),
// and it must pass with no error.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import type { z } from "zod";

import { EVENT_KINDS } from "./generated";
import {
  METHOD_RESULT_SCHEMAS,
  conversationSummarySchema,
  eventSchema,
  recentConversationSchema,
} from "./generated-zod";

const FIXTURES = import.meta.glob<unknown>(
  "../../../../../../crates/efr-protocol/fixtures/v1/**/*.json",
  { eager: true, import: "default" },
);

const OLD_EVENT_KINDS: ReadonlySet<string> = new Set<string>(EVENT_KINDS);

type Json = Readonly<Record<string, unknown>>;

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The fixture `name` under fixtures/v1/, without `.json`. */
function fixture(name: string): unknown {
  const entry = Object.entries(FIXTURES).find(([path]) => path.endsWith(`/v1/${name}.json`));
  if (entry === undefined) {
    throw new Error(`no fixture ${name}`);
  }
  return entry[1];
}

/** The first problem of `schema` with `value`, or undefined when it passes. */
function problem(schema: z.ZodType, value: unknown): string | undefined {
  const result = schema.safeParse(value);
  return result.success ? undefined : JSON.stringify(result.error.issues[0]);
}

/**
 * An event as the old decoder reads it: a kind that it does not know is an `other`
 * event, which it skips; a kind that it knows must pass its schema.
 */
function eventProblem(value: unknown): string | undefined {
  const kind = isObject(value) ? value["kind"] : undefined;
  if (typeof kind !== "string") {
    return "an event with no kind";
  }
  return OLD_EVENT_KINDS.has(kind) ? problem(eventSchema, value) : undefined;
}

/** A subscribe item as the old decoder reads it: a snapshot checks each event alone. */
function subscribeItemProblem(value: unknown): string | undefined {
  if (!isObject(value)) {
    return "an item that is not an object";
  }
  if (value["kind"] === "event") {
    return eventProblem(value["event"]);
  }
  if (value["kind"] !== "snapshot") {
    return problem(METHOD_RESULT_SCHEMAS["conversation.subscribe"], value);
  }
  const events = Array.isArray(value["events"]) ? value["events"] : [];
  const bad = events
    .map((envelope: unknown) => eventProblem(isObject(envelope) ? envelope["event"] : undefined))
    .find((found) => found !== undefined);
  return bad ?? problem(conversationSummarySchema, value["conversation"]);
}

/** A watch item as the old decoder reads it: each row alone. */
function watchItemProblem(value: unknown): string | undefined {
  if (!isObject(value)) {
    return "an item that is not an object";
  }
  const rows = value["kind"] === "snapshot" ? value["conversations"] : [value["conversation"]];
  return (Array.isArray(rows) ? rows : [])
    .map((row: unknown) => problem(recentConversationSchema, row))
    .find((found) => found !== undefined);
}

/** The SHA-256 of each frozen file, as `git show v0.0.11:app/src/lib/protocol/<file>`. */
const FROZEN_SHA256: Readonly<Record<string, string>> = {
  "generated.ts": "8ba297c031cb57db29d404f3775deb06b25bc977794f2d4e7e39b08f5337d1ca",
  "generated-zod.ts": "7243fe6b446b81ee18abf895c5efe39a0f4ee88fc59829d9977b4b9783990eee",
};

describe("the app of 0.0.11", () => {
  it("keeps the files of tag v0.0.11 as they are", () => {
    const found = Object.fromEntries(
      Object.keys(FROZEN_SHA256).map((name) => [
        name,
        createHash("sha256")
          .update(readFileSync(join(import.meta.dirname, name)))
          .digest("hex"),
      ]),
    );
    expect(found).toEqual(FROZEN_SHA256);
  });

  it("reads each summary, list, snapshot and watch item of this efrd with no error", () => {
    const cases: readonly (readonly [string, string | undefined])[] = [
      ["hello_result", problem(METHOD_RESULT_SCHEMAS.hello, fixture("hello_result"))],
      [
        "conversations_list_result",
        problem(METHOD_RESULT_SCHEMAS["conversations.list"], fixture("conversations_list_result")),
      ],
      [
        "conversations_recent_result",
        problem(
          METHOD_RESULT_SCHEMAS["conversations.recent"],
          fixture("conversations_recent_result"),
        ),
      ],
      [
        "conversations_search_result",
        problem(
          METHOD_RESULT_SCHEMAS["conversations.search"],
          fixture("conversations_search_result"),
        ),
      ],
      [
        "conversation_subscribe_item_snapshot",
        subscribeItemProblem(fixture("conversation_subscribe_item_snapshot")),
      ],
      [
        "conversation_subscribe_item_snapshot_live",
        subscribeItemProblem(fixture("conversation_subscribe_item_snapshot_live")),
      ],
      [
        "conversation_subscribe_item_event",
        subscribeItemProblem(fixture("conversation_subscribe_item_event")),
      ],
      [
        "conversations_watch_item_snapshot",
        watchItemProblem(fixture("conversations_watch_item_snapshot")),
      ],
      [
        "conversations_watch_item_changed",
        watchItemProblem(fixture("conversations_watch_item_changed")),
      ],
    ];
    expect(cases.filter(([, found]) => found !== undefined)).toEqual([]);
  });

  it("reads a summary of each attention and drops the member", () => {
    const summaries = fixture("conversation_attention");
    expect(Array.isArray(summaries) && summaries.length).toBe(5);
    for (const summary of Array.isArray(summaries) ? summaries : []) {
      const result = conversationSummarySchema.safeParse(summary);
      expect(result.success).toBe(true);
      expect(result.success && "attention" in result.data).toBe(false);
    }
  });

  it("reads the job events with a batch as the events that it knows", () => {
    for (const name of ["events/job_watch_matched", "events/job_ended"]) {
      const value = fixture(name);
      expect(OLD_EVENT_KINDS.has(isObject(value) ? String(value["kind"]) : "")).toBe(true);
      expect(eventProblem(value)).toBeUndefined();
    }
  });

  it("skips the events of the model's question as events of a kind that it does not know", () => {
    for (const name of ["events/user_question_requested", "events/user_question_answered"]) {
      const value = fixture(name);
      const kind = isObject(value) ? String(value["kind"]) : "";
      expect(OLD_EVENT_KINDS.has(kind)).toBe(false);
      expect(eventProblem(value)).toBeUndefined();
    }
  });
});
