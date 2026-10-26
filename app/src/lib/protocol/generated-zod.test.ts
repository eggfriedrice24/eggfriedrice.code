/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import type { z } from "zod";

import { decodeSubscribeItem, isJsonObject } from "./envelope";
import { METHOD_NAMES, type ConversationSummary, type MethodName } from "./generated";
import * as generated from "./generated-zod";

// The frozen fixtures of efr-protocol: the same JSON that the Rust tests read.
const FIXTURES = import.meta.glob<unknown>(
  "../../../../crates/efr-protocol/fixtures/v1/**/*.json",
  { eager: true, import: "default" },
);
const SCHEMA_FILE = import.meta.glob<unknown>("../../../../docs/protocol.schema.json", {
  eager: true,
  import: "default",
});
const LOCAL = import.meta.glob<unknown>("./fixtures/*.json", { eager: true, import: "default" });
const RECORDINGS = import.meta.glob<string>("../../../test/recordings/*.ndjson", {
  eager: true,
  query: "?raw",
  import: "default",
});

/** The fixtures that hold a list of values of one definition, by file name. */
const LISTS: Readonly<Record<string, string>> = {
  always_saved: "AlwaysSaved",
  approval_decisions: "ApprovalDecision",
  approved_bys: "ApprovedBy",
  attach_hows: "AttachHow",
  block_reasons: "BlockReason",
  bus_proxy_off_reasons: "BusProxyOff",
  context_part_kinds: "ContextPartKind",
  draft_parts: "DraftPart",
  error_codes: "ErrorCode",
  exit_kinds: "ExitKind",
  forward_outcomes: "ForwardOutcome",
  full_refusals: "FullRefusal",
  grants: "Grant",
  held_reasons: "HeldReason",
  input_waits: "InputWait",
  job_states: "JobState",
  job_stop_causes: "JobStopBy",
  judge_kinds: "JudgeKind",
  launches: "Launch",
  modes: "Mode",
  not_saved_reasons: "NotSavedReason",
  notice_kinds: "NoticeKind",
  pair_ends: "PairEnd",
  route_scopes: "RouteScope",
  route_switches: "Switch",
  scope_names: "ScopeName",
  search_kinds: "SearchKind",
  search_partials: "SearchPartial",
  shell_command_ends: "ShellCommandEnd",
  undo_actions: "UndoAction",
  undo_reasons: "UndoReason",
  withdraw_targets: "WithdrawTarget",
};

/** The fixtures that are not a value of the protocol schema, and why. */
const NOT_WIRE_VALUES: Readonly<Record<string, string>> = {
  device_prove_message: "the test vector of the signed message; prove.test.ts reads it",
  undo_refusals: "the data of a turn.undo error, which the protocol schema does not type",
};

type Schema = z.ZodType;

function isSchema(value: unknown): value is Schema {
  return isJsonObject(value) && typeof value["safeParse"] === "function";
}

/** Every exported schema, by its name. */
const SCHEMAS = new Map<string, Schema>(
  Object.entries(generated).flatMap(([name, value]): [string, Schema][] =>
    isSchema(value) ? [[name, value]] : [],
  ),
);

/** The schema of definition `name`, named as the code generator names it. */
function schemaOf(name: string): Schema {
  const schema = SCHEMAS.get(`${name.charAt(0).toLowerCase()}${name.slice(1)}Schema`);
  if (schema === undefined) {
    throw new Error(`generated-zod.ts has no schema for $defs.${name}`);
  }
  return schema;
}

/** The method of each file stem, such as `conversation_subscribe`, longest first. */
const METHOD_STEMS: readonly (readonly [string, MethodName])[] = METHOD_NAMES.map(
  (method) => [method.replaceAll(".", "_"), method] as const,
).sort((a, b) => b[0].length - a[0].length);

/** The name of a fixture path under fixtures/v1/, without `.json`. */
function fixtureName(path: string): string {
  return path.replace(/^.*\/fixtures\/v1\//, "").replace(/\.json$/, "");
}

/** The JSON text of `value` with the keys of each object in order, to compare values. */
function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, member: unknown) =>
    isJsonObject(member)
      ? Object.fromEntries(Object.entries(member).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : member,
  );
}

/** What a schema makes of `value`: the value when it passes, else the first problem. */
function parsed(schema: Schema, value: unknown): unknown {
  const result = schema.safeParse(value);
  return result.success ? result.data : `fails: ${JSON.stringify(result.error.issues[0])}`;
}

/** The values of fixture `name` and the schema of each, or the reason to skip it. */
function targets(name: string, value: unknown): [Schema, unknown][] | string {
  if (name in NOT_WIRE_VALUES) {
    return NOT_WIRE_VALUES[name] ?? "";
  }
  const list = LISTS[name];
  if (list !== undefined) {
    return Array.isArray(value) ? value.map((member) => [schemaOf(list), member]) : "not a list";
  }
  if (name.startsWith("events/")) {
    return [[schemaOf("Event"), value]];
  }
  if (name.startsWith("frames/client_")) {
    return [[schemaOf("ClientFrame"), value]];
  }
  if (name.startsWith("frames/server_")) {
    return [[schemaOf("ServerFrame"), value]];
  }
  if (name === "pairing_qr_url") {
    return [[schemaOf("PairingQr"), isJsonObject(value) ? value["qr"] : undefined]];
  }
  for (const [stem, method] of METHOD_STEMS) {
    if (name === `${stem}_params`) {
      const named = isJsonObject(value) && value["method"] === method;
      return named ? [[schemaOf("Method"), value]] : `not a request of ${method}`;
    }
    if (name.startsWith(`${stem}_result`) || name.startsWith(`${stem}_item`)) {
      return [[generated.METHOD_RESULT_SCHEMAS[method], value]];
    }
  }
  return "no schema: add the fixture to the table of this test";
}

describe("the generated zod schemas", () => {
  it("have a schema for each definition of the protocol schema", () => {
    const file = Object.values(SCHEMA_FILE)[0];
    const defs = isJsonObject(file) && isJsonObject(file["$defs"]) ? file["$defs"] : {};
    const names = Object.keys(defs);
    expect(names.length).toBeGreaterThan(200);
    expect(
      names.filter((name) => !SCHEMAS.has(`${name.charAt(0).toLowerCase()}${name.slice(1)}Schema`)),
    ).toEqual([]);
    expect(SCHEMAS.size).toBe(names.length);
  });

  it("read every frozen fixture of efr-protocol as it is, with no member dropped", () => {
    const names = Object.keys(FIXTURES);
    expect(names.length).toBeGreaterThan(150);
    const problems: unknown[] = [];
    for (const path of names) {
      const name = fixtureName(path);
      const value = FIXTURES[path];
      if (name === "events/unknown") {
        continue;
      }
      const found = targets(name, value);
      if (typeof found === "string") {
        if (!(name in NOT_WIRE_VALUES)) {
          problems.push({ name, problem: found });
        }
        continue;
      }
      for (const [schema, member] of found) {
        const result = parsed(schema, member);
        if (canonical(result) !== canonical(member)) {
          problems.push({ name, member, result });
        }
      }
    }
    expect(problems).toEqual([]);
  });

  it("refuse the event of a kind that no build knows", () => {
    const unknown = Object.entries(FIXTURES).find(([path]) =>
      path.endsWith("/events/unknown.json"),
    );
    expect(unknown).toBeDefined();
    expect(schemaOf("Event").safeParse(unknown?.[1]).success).toBe(false);
  });

  it("refuse a u64 above 2^53, an id that is not a UUID and a time without an offset", () => {
    const seq = schemaOf("Seq");
    expect([0, 2 ** 53 - 1, 2 ** 53, -1, 1.5].map((value) => seq.safeParse(value).success)).toEqual(
      [true, true, false, false, false],
    );
    const id = schemaOf("ConversationId");
    expect(id.safeParse("019a9b1c-3d00-7a10-8b20-000000000001").success).toBe(true);
    expect(id.safeParse("019a9b1c3d007a108b20000000000001").success).toBe(false);
    const summary = schemaOf("ConversationSummary");
    const base = {
      id: "019a9b1c-3d00-7a10-8b20-000000000001",
      status: "idle",
      created_at: "2026-10-26T12:00:00Z",
      updated_at: "2026-10-26T12:00:00+02:00",
      last_seq: 1,
    };
    expect(summary.safeParse(base).success).toBe(true);
    expect(summary.safeParse({ ...base, updated_at: "2026-10-26T12:00:00" }).success).toBe(false);
  });

  it("make a drift between a schema and its type a type error", () => {
    const withoutId = generated.conversationSummarySchema.omit({ id: true });
    // @ts-expect-error: without `id` the schema does not satisfy the type of a summary.
    const drift = withoutId satisfies z.ZodType<ConversationSummary>;
    expect(drift.safeParse({}).success).toBe(false);
  });
});

/** The decoder fixture `name` of the app (fixtures/README.md). */
function local(name: string): unknown {
  const entry = Object.entries(LOCAL).find(([path]) => path.endsWith(`/${name}.json`));
  if (entry === undefined) {
    throw new Error(`no fixture ${name}`);
  }
  return entry[1];
}

describe("the decoder over the fixtures of the app", () => {
  it("decodes an item of an unknown kind to an other item", () => {
    expect(decodeSubscribeItem(local("item-unknown-kind"))).toEqual({
      ok: true,
      value: { kind: "other", wireKind: "presence" },
    });
  });

  it("decodes a known event with an unknown nested value to an invalid other event", () => {
    expect(decodeSubscribeItem(local("item-event-unknown-nested-value"))).toEqual({
      ok: true,
      value: {
        kind: "event",
        seq: 43,
        conversation_id: "019a9b1c-3d00-7a10-8b20-000000000001",
        at: "2026-10-03T12:00:06Z",
        event: { kind: "other", wireKind: "tool_call_started", invalid: true },
      },
    });
  });

  it("decodes a known draft with an unknown nested value to an invalid other part", () => {
    expect(decodeSubscribeItem(local("item-draft-unknown-nested-value"))).toEqual({
      ok: true,
      value: {
        kind: "draft",
        turn_id: "019a9b1c-3d00-7a10-8b20-000000000002",
        after_seq: 43,
        draft: { kind: "other", wireKind: "compacting", invalid: true },
      },
    });
  });
});

describe("the decoder over the recordings of efrd", () => {
  it("decodes every item of every recording, with no invalid wrapper", () => {
    const paths = Object.keys(RECORDINGS);
    expect(paths.length).toBe(8);
    const problems: unknown[] = [];
    let items = 0;
    for (const [path, text] of Object.entries(RECORDINGS)) {
      for (const line of text.split("\n").filter((row) => row.trim() !== "")) {
        items += 1;
        const decoded = decodeSubscribeItem(JSON.parse(line));
        const json = JSON.stringify(decoded);
        if (!decoded.ok || json.includes('"invalid":true') || json.includes('"kind":"other"')) {
          problems.push({ path, line: line.slice(0, 200), decoded });
        }
      }
    }
    expect(items).toBeGreaterThan(500);
    expect(problems).toEqual([]);
  });
});
