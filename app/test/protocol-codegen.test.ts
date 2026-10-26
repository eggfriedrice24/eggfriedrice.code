/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";

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
} from "@/lib/protocol/generated";
import {
  constants,
  definitionOrder,
  generate,
  generateZod,
  normalize,
  references,
  schemaName,
  variantKinds,
  withoutReferenceNotes,
  zodSchemas,
} from "../scripts/protocol-codegen.mjs";

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

  it("types a schema with only annotations as unknown, but not a property map or data", () => {
    const schema = {
      properties: {
        input: { description: "Any JSON value." },
        title: { description: "A title.", type: "string" },
      },
      default: { description: "data, not a schema" },
      anyOf: [{ title: "T" }],
    };
    expect(normalize(schema)).toEqual({
      properties: {
        input: { description: "Any JSON value.", tsType: "unknown" },
        title: { description: "A title.", type: "string" },
      },
      default: { description: "data, not a schema" },
      anyOf: [{ title: "T", tsType: "unknown" }],
    });
    expect(normalize({ properties: { description: { title: "d" } } })).toEqual({
      properties: { description: { title: "d", tsType: "unknown" } },
    });
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

/** The source of the schema of definition `name` in the output of `zodSchemas`. */
function schemaOf(defs: Record<string, unknown>, name: string): string {
  const source = zodSchemas(defs);
  const start = source.indexOf(`export const ${schemaName(name)}`);
  if (start < 0) {
    throw new Error(`no schema for ${name}`);
  }
  const end = source.indexOf(";\n", start);
  return source.slice(start, end < 0 ? undefined : end + 1);
}

describe("the zod emitter", () => {
  it("names a schema after its definition in camelCase", () => {
    expect(schemaName("ConversationSummary")).toBe("conversationSummarySchema");
    expect(schemaName("Seq")).toBe("seqSchema");
    expect(() => schemaName("not_pascal")).toThrow(/PascalCase/);
  });

  it("finds the references of a schema, but not in data", () => {
    const schema = {
      properties: { a: { $ref: "#/$defs/A" }, b: { anyOf: [{ $ref: "#/$defs/B" }] } },
      default: { $ref: "#/$defs/C" },
    };
    expect(references(schema)).toEqual(["A", "B"]);
    expect(() => references({ $ref: "other.json#/x" })).toThrow(/does not name a definition/);
  });

  it("orders each definition after the ones it names, and groups a cycle", () => {
    const defs = {
      A: { $ref: "#/$defs/B" },
      B: { type: "string" },
      C: { properties: { d: { $ref: "#/$defs/D" } }, type: "object" },
      D: { properties: { c: { $ref: "#/$defs/C" } }, type: "object" },
    };
    expect(definitionOrder(defs)).toEqual([["B"], ["A"], ["C", "D"]]);
    expect(() => definitionOrder({ A: { $ref: "#/$defs/Missing" } })).toThrow(/does not exist/);
  });

  it("writes optional members with exactOptional and null with nullable", () => {
    const defs = {
      Summary: {
        properties: {
          id: { format: "uuid", type: "string" },
          title: { type: ["string", "null"] },
          at: { format: "date-time", type: "string" },
          parent: { anyOf: [{ $ref: "#/$defs/Seq" }, { type: "null" }] },
        },
        required: ["id", "at"],
        type: "object",
      },
      Seq: { format: "uint64", minimum: 0, type: "integer" },
    };
    expect(schemaOf(defs, "Summary")).toBe(
      "export const summarySchema = z.object({ id: z.guid(), title: z.string().nullable().exactOptional(), " +
        "at: z.iso.datetime({ offset: true }), parent: seqSchema.nullable().exactOptional() }) satisfies z.ZodType<Summary>;",
    );
    expect(schemaOf(defs, "Seq")).toBe(
      "export const seqSchema = z.int().nonnegative() satisfies z.ZodType<Seq>;",
    );
  });

  it("maps each integer format and adds only the bounds that it does not imply", () => {
    const defs = {
      A: { format: "uint32", minimum: 0, type: "integer" },
      B: { format: "uint8", maximum: 255, minimum: 0, type: "integer" },
      C: { format: "uint16", maximum: 100, minimum: 1, type: "integer" },
      D: { format: "int32", type: "integer" },
      E: { minimum: 2, type: "integer" },
    };
    expect(schemaOf(defs, "A")).toContain("= z.uint32() satisfies");
    expect(schemaOf(defs, "B")).toContain("= z.int().min(0).max(255) satisfies");
    expect(schemaOf(defs, "C")).toContain("= z.int().min(0).max(65535).min(1).max(100) satisfies");
    expect(schemaOf(defs, "D")).toContain("= z.int32() satisfies");
    expect(schemaOf(defs, "E")).toContain("= z.int().min(2) satisfies");
    expect(() => zodSchemas({ F: { format: "uint128", type: "integer" } })).toThrow(/uint128/);
  });

  it("writes constants, enums and a pattern", () => {
    const defs = {
      Version: { const: 1, type: "integer" },
      Mode: {
        oneOf: [
          { const: "auto", type: "string" },
          { const: "manual", type: "string" },
        ],
      },
      Code: { enum: ["busy", "invalid"], type: "string" },
      Job: { pattern: "^j/[1-9]$", type: "string" },
    };
    expect(schemaOf(defs, "Version")).toContain("= z.literal(1) satisfies");
    expect(schemaOf(defs, "Mode")).toContain('= z.enum(["auto","manual"]) satisfies');
    expect(schemaOf(defs, "Code")).toContain('= z.enum(["busy","invalid"]) satisfies');
    expect(schemaOf(defs, "Job")).toContain(String.raw`= z.string().regex(/^j\/[1-9]$/) satisfies`);
  });

  it("writes a union tagged by a constant member as a discriminated union", () => {
    const defs = {
      Base: { properties: { at: { type: "string" } }, required: ["at"], type: "object" },
      Part: {
        oneOf: [
          {
            properties: { kind: { const: "text", type: "string" }, delta: { type: "string" } },
            required: ["kind", "delta"],
            type: "object",
          },
          {
            $ref: "#/$defs/Base",
            properties: { kind: { const: "base", type: "string" } },
            required: ["kind"],
            type: "object",
          },
        ],
      },
      Request: {
        oneOf: [
          {
            properties: { method: { const: "hello" }, params: {} },
            required: ["method", "params"],
            type: "object",
          },
          {
            properties: { method: { const: "bye" }, params: {} },
            required: ["method", "params"],
            type: "object",
          },
        ],
      },
      Loose: { anyOf: [{ $ref: "#/$defs/Base" }, { type: "string" }] },
    };
    expect(schemaOf(defs, "Part")).toBe(
      'export const partSchema = z.discriminatedUnion("kind", [z.object({ kind: z.literal("text"), delta: z.string() }), ' +
        'baseSchema.extend({ kind: z.literal("base") })]) satisfies z.ZodType<Part>;',
    );
    expect(schemaOf(defs, "Request")).toContain('z.discriminatedUnion("method", [');
    expect(schemaOf(defs, "Request")).toContain("params: z.unknown()");
    expect(schemaOf(defs, "Loose")).toContain("= z.union([baseSchema, z.string()]) satisfies");
  });

  it("makes an object strict, loose or stripping as additionalProperties says", () => {
    const defs = {
      Strict: {
        additionalProperties: false,
        properties: { a: { type: "boolean" } },
        type: "object",
      },
      Loose: { additionalProperties: true, properties: {}, type: "object" },
      Plain: { type: "object" },
      Map: { additionalProperties: { type: "string" }, type: "object" },
    };
    expect(schemaOf(defs, "Strict")).toContain(
      "= z.strictObject({ a: z.boolean().exactOptional() }) satisfies",
    );
    expect(schemaOf(defs, "Loose")).toContain("= z.looseObject({}) satisfies");
    expect(schemaOf(defs, "Plain")).toContain("= z.object({}) satisfies");
    expect(schemaOf(defs, "Map")).toContain("= z.object({}).catchall(z.string()) satisfies");
  });

  it("writes an array of an exact length as a tuple, as the type generator does", () => {
    const defs = {
      Rgb: { items: { type: "integer" }, maxItems: 3, minItems: 3, type: "array" },
      Some: { items: { type: "string" }, maxItems: 4, minItems: 1, type: "array" },
      Any: { type: "array" },
    };
    expect(schemaOf(defs, "Rgb")).toContain("= z.tuple([z.int(), z.int(), z.int()]) satisfies");
    expect(schemaOf(defs, "Some")).toContain("= z.array(z.string()).min(1).max(4) satisfies");
    expect(schemaOf(defs, "Any")).toContain("= z.array(z.unknown()) satisfies");
  });

  it("writes an allOf as an intersection and any value as unknown", () => {
    const defs = {
      Id: { properties: { id: { type: "integer" } }, required: ["id"], type: "object" },
      Method: { properties: { method: { type: "string" } }, required: ["method"], type: "object" },
      Request: { allOf: [{ $ref: "#/$defs/Id" }, { $ref: "#/$defs/Method" }] },
      Body: {
        properties: { data: { description: "Any value." }, item: {} },
        required: ["item"],
        type: "object",
      },
    };
    expect(schemaOf(defs, "Request")).toContain(
      "= z.intersection(idSchema, methodSchema) satisfies",
    );
    expect(schemaOf(defs, "Body")).toContain(
      "= z.object({ data: z.unknown().exactOptional(), item: z.unknown() }) satisfies",
    );
  });

  it("names a definition of a cycle lazily and types it", () => {
    const defs = {
      Node: {
        properties: { children: { items: { $ref: "#/$defs/Node" }, type: "array" } },
        required: ["children"],
        type: "object",
      },
    };
    expect(schemaOf(defs, "Node")).toBe(
      "export const nodeSchema: z.ZodType<Node> = z.object({ children: z.array(z.lazy(() => nodeSchema)) });",
    );
  });

  it("stops at a keyword, a format or a form that it does not know", () => {
    expect(() => zodSchemas({ A: { minLength: 1, type: "string" } })).toThrow(
      /#\/\$defs\/A has the keyword minLength/,
    );
    expect(() => zodSchemas({ A: { format: "email", type: "string" } })).toThrow(/email/);
    expect(() =>
      zodSchemas({
        A: { type: "string" },
        B: { $ref: "#/$defs/A", properties: { x: { type: "string" } }, type: "object" },
      }),
    ).toThrow(/not an object/);
    expect(() => zodSchemas({ A: { type: ["string", "integer"] } })).toThrow(/more than one type/);
    expect(() => zodSchemas({ A: { properties: {}, required: ["x"], type: "object" } })).toThrow(
      /requires x/,
    );
  });

  it("writes the banner, the type import and the map of the result schemas", async () => {
    const schema = {
      $defs: {
        MethodName: { enum: ["hello"], type: "string" },
        HelloResult: { properties: { daemon_id: { type: "string" } }, type: "object" },
        MethodResults: {
          additionalProperties: false,
          properties: { hello: { $ref: "#/$defs/HelloResult" } },
          required: ["hello"],
          type: "object",
        },
      },
    };
    const text = await generateZod(JSON.stringify(schema));
    expect(text.startsWith("// Generated from docs/protocol.schema.json")).toBe(true);
    expect(text).toContain('import { z } from "zod";');
    expect(text).toContain(
      'import type { HelloResult, MethodName, MethodResults } from "./generated";',
    );
    expect(text).toContain("export const METHOD_RESULT_SCHEMAS: {");
    expect(text).toContain("} = methodResultsSchema.shape;");
  });
});
