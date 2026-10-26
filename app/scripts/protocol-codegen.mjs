// The code generator of the protocol types: docs/protocol.schema.json in, the text of
// src/protocol/generated.ts out. scripts/gen-protocol.mjs runs it; the tests in
// test/protocol-codegen.test.ts import it.
//
// json-schema-to-typescript drops the other members of an object that has a `$ref`: a
// `{"$ref": "#/$defs/Draft", "properties": {"kind": ...}}` became a type with only
// `kind`, and a `$ref` next to a `description` became a copy named `Draft1`. JSON
// Schema 2020-12 reads such an object as both schemas at once, so `normalize` writes
// it as an `allOf` of the two before the generator reads it. The types stay the same.

import { compile } from "json-schema-to-typescript";
import { format } from "oxfmt";

/** Members that describe a schema but do not change what it accepts. */
const ANNOTATIONS = new Set([
  "$comment",
  "default",
  "deprecated",
  "description",
  "examples",
  "readOnly",
  "title",
  "writeOnly",
]);

/** The first lines of the generated file. */
export const BANNER = `// Generated from docs/protocol.schema.json by \`pnpm gen:protocol\` (scripts/gen-protocol.mjs).
// Do not edit it by hand. Change efr-protocol, run \`cargo xtask protocol-schema\` at the repo
// top, then run \`pnpm gen:protocol\` in app/.
//
// Numbers: a u64 of the protocol is a JS number here. It is exact up to 2^53, and the
// envelope guard (src/core/envelope.ts) refuses a larger value. Every offset into text
// counts bytes of UTF-8, not UTF-16 units: compare it with utf8Length (src/core/utf8.ts),
// never with \`text.length\`.
//
// An event, draft or item kind that this file does not list can come from a newer efrd.
// The envelope guard decodes it to an "other" wrapper, and the app skips it.
`;

/**
 * Returns a copy of `schema` in which every object with a `$ref` and other members is
 * an `allOf` of the reference and the other members. Annotations stay on the outer
 * object, so the generator still writes them as comments.
 *
 * @param {any} schema
 * @returns {any}
 */
export function normalize(schema) {
  if (Array.isArray(schema)) {
    return schema.map((member) => normalize(member));
  }
  if (typeof schema !== "object" || schema === null) {
    return schema;
  }
  /** @type {Record<string, unknown>} */
  const copy = {};
  for (const [key, value] of Object.entries(schema)) {
    copy[key] = normalize(value);
  }
  const ref = copy["$ref"];
  if (typeof ref !== "string" || Object.keys(copy).length === 1) {
    return copy;
  }
  /** @type {Record<string, unknown>} */
  const annotations = {};
  /** @type {Record<string, unknown>} */
  const rest = {};
  for (const [key, value] of Object.entries(copy)) {
    if (key === "$ref") {
      continue;
    }
    if (ANNOTATIONS.has(key)) {
      annotations[key] = value;
    } else {
      rest[key] = value;
    }
  }
  const members = Object.keys(rest).length === 0 ? [{ $ref: ref }] : [{ $ref: ref }, rest];
  return { ...annotations, allOf: members };
}

/**
 * The `kind` values of the variants of definition `name`, in schema order.
 *
 * @param {Record<string, any>} defs
 * @param {string} name
 * @returns {string[]}
 */
export function variantKinds(defs, name) {
  const definition = defs[name];
  const variants = definition?.oneOf ?? definition?.anyOf;
  if (!Array.isArray(variants)) {
    throw new Error(`$defs.${name} is not a oneOf or anyOf`);
  }
  return variants.map((variant, index) => {
    const kind = variant?.properties?.kind?.const;
    if (typeof kind !== "string") {
      throw new Error(`$defs.${name} variant ${index} has no constant kind`);
    }
    return kind;
  });
}

/**
 * The string constants of definition `name`: an `enum`, or a `oneOf` of `const`s.
 *
 * @param {Record<string, any>} defs
 * @param {string} name
 * @returns {string[]}
 */
export function constants(defs, name) {
  const definition = defs[name];
  const values = Array.isArray(definition?.enum)
    ? definition.enum
    : definition?.oneOf?.map((/** @type {any} */ variant) => variant?.const);
  if (!Array.isArray(values) || !values.every((value) => typeof value === "string")) {
    throw new Error(`$defs.${name} is not a set of string constants`);
  }
  return values;
}

/**
 * A JSON array of strings, for the generated constants.
 *
 * @param {string[]} values
 * @returns {string}
 */
function list(values) {
  return JSON.stringify(values);
}

/**
 * The runtime constants that the app needs next to the types: the protocol version
 * and the known kinds and names, each checked against its type.
 *
 * @param {Record<string, any>} defs
 * @returns {string}
 */
export function runtimeConstants(defs) {
  const version = defs["ProtocolVersion"]?.const;
  if (typeof version !== "number") {
    throw new Error("$defs.ProtocolVersion has no constant");
  }
  return [
    "",
    "/** The protocol version of this file, as `hello` sends it. */",
    `export const PROTOCOL_VERSION = ${version} satisfies ProtocolVersion;`,
    "",
    "/** Every event kind of this file, in schema order. */",
    `export const EVENT_KINDS = ${list(variantKinds(defs, "Event"))} as const satisfies readonly Event["kind"][];`,
    "",
    "/** Every draft part kind of this file. */",
    `export const DRAFT_PART_KINDS = ${list(variantKinds(defs, "DraftPart"))} as const satisfies readonly DraftPart["kind"][];`,
    "",
    "/** Every item kind of `conversation.subscribe`. */",
    `export const SUBSCRIBE_ITEM_KINDS = ${list(variantKinds(defs, "ConversationSubscribeItem"))} as const satisfies readonly ConversationSubscribeItem["kind"][];`,
    "",
    "/** Every method name. */",
    `export const METHOD_NAMES = ${list(constants(defs, "MethodName"))} as const satisfies readonly MethodName[];`,
    "",
    "/** Every error code. The set is closed: a new code bumps the protocol version. */",
    `export const ERROR_CODES = ${list(constants(defs, "ErrorCode"))} as const satisfies readonly ErrorCode[];`,
    "",
  ].join("\n");
}

/**
 * Removes the note that the generator adds to the comment of each definition ("This
 * interface was referenced by ... via the `definition` X"): the type's name says it.
 * A comment that held only the note goes away.
 *
 * @param {string} types
 * @returns {string}
 */
export function withoutReferenceNotes(types) {
  return types
    .replaceAll(
      /\n \*\s*\n \* This interface was referenced by `[^`]+`'s JSON-Schema\n \* via the `definition` "[^"]+"\./g,
      "",
    )
    .replaceAll(
      /\n?\/\*\*\s*\n \* This interface was referenced by `[^`]+`'s JSON-Schema\n \* via the `definition` "[^"]+"\.\n \*\/\n/g,
      "\n",
    );
}

/**
 * The formatted text of src/protocol/generated.ts for the text of the schema file.
 *
 * @param {string} schemaText
 * @returns {Promise<string>}
 */
export async function generate(schemaText) {
  const schema = JSON.parse(schemaText);
  const defs = schema["$defs"];
  if (typeof defs !== "object" || defs === null) {
    throw new Error("the schema has no $defs");
  }
  const types = await compile(normalize(schema), "EfrProtocol", {
    additionalProperties: false,
    bannerComment: "",
    format: false,
    unknownAny: true,
    unreachableDefinitions: true,
  });
  const source = `${BANNER}\n${withoutReferenceNotes(types)}${runtimeConstants(defs)}`;
  const formatted = await format("generated.ts", source, {});
  if (formatted.errors.length > 0) {
    const messages = formatted.errors.map((error) => error.message).join("; ");
    throw new Error(`oxfmt could not format the generated types: ${messages}`);
  }
  return formatted.code;
}
