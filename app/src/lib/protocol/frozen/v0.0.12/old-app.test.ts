// The app of 0.0.12, which users still run, against the fixtures of this efrd.
//
// generated.ts and generated-zod.ts in this folder are the files of tag v0.0.12, as they
// are: never generate them again. The test pins their SHA-256, so a change of either one
// fails it. The decoder of 0.0.12 (envelope.ts of v0.0.12) skips an event of a kind that
// it does not know, and it also skips a known event whose members fail its schema. So a
// changed event that failed here would not break the app, but the app would lose it:
// a prompt with attachments would not show. This test decodes each result and event
// that 0.0.13 changed, with the schemas of 0.0.12, and each must pass.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { fixture, isObject, problem } from "../testing";
import { EVENT_KINDS } from "./generated";
import {
  METHOD_RESULT_SCHEMAS,
  capabilitiesSchema,
  contextPartKindSchema,
  eventSchema,
} from "./generated-zod";

const OLD_EVENT_KINDS: ReadonlySet<string> = new Set<string>(EVENT_KINDS);

/** The events that 0.0.13 gave new members. */
const CHANGED_EVENTS = [
  "events/prompt_queued",
  "events/tool_call_completed",
  "events/turn_started",
] as const;

/** The SHA-256 of each frozen file, as `git show v0.0.12:app/src/lib/protocol/<file>`. */
const FROZEN_SHA256: Readonly<Record<string, string>> = {
  "generated.ts": "a09862a18d566b6701302aec037f6898268e67b0c9d6edfa51205c399c8a1733",
  "generated-zod.ts": "a2e61eacd9795532b3f022057278543506a7166f921ecbf7b73a0ca1c6f9c6e8",
};

describe("the app of 0.0.12", () => {
  it("keeps the files of tag v0.0.12 as they are", () => {
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

  it("reads the hello results with the capabilities of attachments and push", () => {
    const cases = ["hello_result", "hello_result_remote_attach"].map(
      (name) => [name, problem(METHOD_RESULT_SCHEMAS.hello, fixture(name))] as const,
    );
    expect(cases.filter(([, found]) => found !== undefined)).toEqual([]);
    const remote = fixture("hello_result_remote_attach");
    const capabilities = isObject(remote) ? remote["capabilities"] : undefined;
    expect(problem(capabilitiesSchema, capabilities)).toBeUndefined();
  });

  it.each(CHANGED_EVENTS)("reads %s as the event that it knows with no new member", (name) => {
    const value = fixture(name);
    expect(OLD_EVENT_KINDS.has(isObject(value) ? String(value["kind"]) : "")).toBe(true);
    expect(problem(eventSchema, value)).toBeUndefined();
    const event: unknown = eventSchema.parse(value);
    expect(isObject(event) && "attachments" in event).toBe(false);
    expect(isObject(event) && "unread_attachments" in event).toBe(false);
  });

  it("skips the withheld attachments as an event of a kind that it does not know", () => {
    const value = fixture("events/attachments_withheld");
    expect(OLD_EVENT_KINDS.has(isObject(value) ? String(value["kind"]) : "")).toBe(false);
  });

  it("reads the attachments of the context as a part of a kind that it does not know", () => {
    expect(contextPartKindSchema.parse("attachments")).toBe("unknown");
  });
});
