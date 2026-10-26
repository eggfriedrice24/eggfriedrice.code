/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";

import {
  constants,
  generate,
  normalize,
  variantKinds,
  withoutReferenceNotes,
} from "../scripts/protocol-codegen.mjs";
import {
  DRAFT_PART_KINDS,
  ERROR_CODES,
  EVENT_KINDS,
  METHOD_NAMES,
  PROTOCOL_VERSION,
  SUBSCRIBE_ITEM_KINDS,
  type ConversationSubscribeItem,
  type MethodParams,
  type MethodResults,
} from "../src/protocol/generated";

const events = import.meta.glob<unknown>("../../crates/efr-protocol/fixtures/v1/events/*.json", {
  eager: true,
  import: "default",
});
const draftParts = import.meta.glob<unknown[]>(
  "../../crates/efr-protocol/fixtures/v1/draft_parts.json",
  { eager: true, import: "default" },
);
const errorCodes = import.meta.glob<string[]>(
  "../../crates/efr-protocol/fixtures/v1/error_codes.json",
  { eager: true, import: "default" },
);

function kindOf(value: unknown): unknown {
  return typeof value === "object" && value !== null && "kind" in value ? value.kind : undefined;
}

describe("normalize", () => {
  it("writes a $ref with structural members as an allOf of both", () => {
    const schema = {
      $ref: "#/$defs/Draft",
      description: "A draft.",
      properties: { kind: { const: "draft", type: "string" } },
      required: ["kind"],
      type: "object",
    };
    expect(normalize(schema)).toEqual({
      description: "A draft.",
      allOf: [
        { $ref: "#/$defs/Draft" },
        {
          properties: { kind: { const: "draft", type: "string" } },
          required: ["kind"],
          type: "object",
        },
      ],
    });
  });

  it("keeps the annotations of a $ref outside the reference", () => {
    expect(normalize({ $ref: "#/$defs/Seq", description: "d", default: 0 })).toEqual({
      description: "d",
      default: 0,
      allOf: [{ $ref: "#/$defs/Seq" }],
    });
  });

  it("leaves a bare $ref, other schemas and nested values alone, and copies", () => {
    const schema = {
      $defs: { A: { type: "string" } },
      properties: {
        a: { $ref: "#/$defs/A" },
        b: { anyOf: [{ $ref: "#/$defs/A", description: "x" }, { type: "null" }] },
      },
    };
    const normalized = normalize(schema);
    expect(normalized).toEqual({
      $defs: { A: { type: "string" } },
      properties: {
        a: { $ref: "#/$defs/A" },
        b: { anyOf: [{ description: "x", allOf: [{ $ref: "#/$defs/A" }] }, { type: "null" }] },
      },
    });
    expect(normalized).not.toBe(schema);
    expect(schema.properties.b.anyOf[0]).toEqual({ $ref: "#/$defs/A", description: "x" });
  });
});

describe("variantKinds and constants", () => {
  const defs = {
    E: {
      oneOf: [{ properties: { kind: { const: "a" } } }, { properties: { kind: { const: "b" } } }],
    },
    F: { anyOf: [{ properties: { kind: { const: "c" } } }] },
    G: { oneOf: [{ properties: {} }] },
    N: { enum: ["x", "y"] },
    O: { oneOf: [{ const: "p" }, { const: "q" }] },
    P: { oneOf: [{ const: 1 }] },
  };

  it("reads the kinds of a oneOf or an anyOf and fails without a constant kind", () => {
    expect(variantKinds(defs, "E")).toEqual(["a", "b"]);
    expect(variantKinds(defs, "F")).toEqual(["c"]);
    expect(() => variantKinds(defs, "G")).toThrow(/no constant kind/);
    expect(() => variantKinds(defs, "missing")).toThrow(/not a oneOf/);
  });

  it("reads an enum or a oneOf of string constants", () => {
    expect(constants(defs, "N")).toEqual(["x", "y"]);
    expect(constants(defs, "O")).toEqual(["p", "q"]);
    expect(() => constants(defs, "P")).toThrow(/string constants/);
  });
});

describe("withoutReferenceNotes", () => {
  it("drops the note and a comment that held only the note", () => {
    const raw = [
      "/**",
      " * A seq.",
      " *",
      " * This interface was referenced by `EfrProtocol`'s JSON-Schema",
      ' * via the `definition` "Seq".',
      " */",
      "export type Seq = number",
      "/**",
      " * This interface was referenced by `EfrProtocol`'s JSON-Schema",
      ' * via the `definition` "Base64Bytes".',
      " */",
      "export type Base64Bytes = string",
    ].join("\n");
    expect(withoutReferenceNotes(raw)).toBe(
      [
        "/**",
        " * A seq.",
        " */",
        "export type Seq = number",
        "export type Base64Bytes = string",
      ].join("\n"),
    );
  });
});

describe("generate", () => {
  it("writes the banner, the types and the constants of a small schema", async () => {
    const schema = {
      $schema: "https://json-schema.org/draft/2020-12/schema",
      title: "EfrProtocol",
      type: "object",
      properties: { item: { $ref: "#/$defs/ConversationSubscribeItem" } },
      $defs: {
        ProtocolVersion: { const: 1, type: "integer" },
        Event: { anyOf: [{ properties: { kind: { const: "e" } }, required: ["kind"] }] },
        DraftPart: { oneOf: [{ properties: { kind: { const: "d" } }, required: ["kind"] }] },
        Draft: { type: "object", properties: { turn_id: { type: "string" } } },
        ConversationSubscribeItem: {
          oneOf: [
            {
              $ref: "#/$defs/Draft",
              properties: { kind: { const: "draft", type: "string" } },
              required: ["kind"],
              type: "object",
            },
          ],
        },
        MethodName: { enum: ["hello"] },
        ErrorCode: { oneOf: [{ const: "busy", type: "string" }] },
      },
    };
    const text = await generate(JSON.stringify(schema));
    expect(text.startsWith("// Generated from docs/protocol.schema.json")).toBe(true);
    expect(text).toContain(
      'export type ConversationSubscribeItem = Draft & {\n  kind: "draft";\n};',
    );
    expect(text).toContain("export const PROTOCOL_VERSION = 1 satisfies ProtocolVersion;");
    expect(text).toContain('export const EVENT_KINDS = ["e"] as const');
    expect(text).not.toContain("referenced by");
  });
});

describe("the generated constants", () => {
  it("speak protocol 1", () => {
    expect(PROTOCOL_VERSION).toBe(1);
  });

  it("list the kind of every event fixture but the unknown one", () => {
    const kinds = new Set<unknown>(EVENT_KINDS);
    const names = Object.keys(events);
    expect(names.length).toBeGreaterThan(40);
    for (const path of names) {
      // events/unknown.json holds a kind from a newer efrd, which no build lists.
      const known = !path.endsWith("/unknown.json");
      expect({ path, known: kinds.has(kindOf(events[path])) }).toEqual({ path, known });
    }
  });

  it("list the draft parts, the item kinds, the methods and the error codes", () => {
    const parts = Object.values(draftParts)[0];
    expect(Array.isArray(parts)).toBe(true);
    for (const part of parts ?? []) {
      expect(DRAFT_PART_KINDS).toContain(kindOf(part));
    }
    expect([...SUBSCRIBE_ITEM_KINDS]).toEqual(["event", "snapshot", "draft"]);
    expect(METHOD_NAMES).toContain("device.prove");
    expect(METHOD_NAMES).toContain("conversation.subscribe");
    expect([...ERROR_CODES].sort()).toEqual([...(Object.values(errorCodes)[0] ?? [])].sort());
  });

  it("type a subscribe item with its kind and the members of its variant", () => {
    const draft: ConversationSubscribeItem = {
      kind: "draft",
      turn_id: "t",
      after_seq: 1,
      draft: { kind: "text", index: 0, offset: 0, delta: "a" },
    };
    // @ts-expect-error: a draft item without its kind is not a subscribe item.
    const unkinded: ConversationSubscribeItem = { turn_id: "t", after_seq: 1, draft: draft.draft };
    const prove: MethodParams["device.prove"] = { device_id: "d", signature: "MAY=" };
    const scopes: MethodResults["device.prove"]["scopes"] = ["read"];
    expect([draft.kind, unkinded.kind, prove.device_id, scopes[0]]).toEqual([
      "draft",
      undefined,
      "d",
      "read",
    ]);
  });
});
