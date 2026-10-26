// The code generator of the protocol: docs/protocol.schema.json in, the text of two files
// out. src/lib/protocol/generated.ts holds the types (json-schema-to-typescript) and
// src/lib/protocol/generated-zod.ts the zod schemas (the emitter in this file).
// scripts/gen-protocol.mjs runs it; the tests in test/protocol-codegen.test.ts import it.
//
// json-schema-to-typescript drops the other members of an object that has a `$ref`: a
// `{"$ref": "#/$defs/Draft", "properties": {"kind": ...}}` became a type with only
// `kind`, and a `$ref` next to a `description` became a copy named `Draft1`. JSON
// Schema 2020-12 reads such an object as both schemas at once, so `normalize` writes
// it as an `allOf` of the two before the generator reads it. The types stay the same.
//
// json-schema-to-typescript also types a schema with only a description as an object.
// Such a schema accepts any value, and efrd sends a string in one of them (the `input`
// of a freeform tool call). So `normalize` marks it `tsType: "unknown"`.

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

/** Members whose value is a map from names to schemas. */
const SCHEMA_MAPS = new Set(["$defs", "properties"]);

/** Members whose value is a schema or an array of schemas. */
const SCHEMA_MEMBERS = new Set([
  "additionalProperties",
  "allOf",
  "anyOf",
  "items",
  "not",
  "oneOf",
  "prefixItems",
]);

/** The first lines of the generated types. */
export const BANNER = `// Generated from docs/protocol.schema.json by \`pnpm gen:protocol\` (scripts/gen-protocol.mjs).
// Do not edit it by hand. Change efr-protocol, run \`cargo xtask protocol-schema\` at the repo
// top, then run \`pnpm gen:protocol\` in app/.
//
// Numbers: a u64 of the protocol is a JS number here. It is exact up to 2^53, and the
// decoder (src/lib/protocol/envelope.ts) refuses a larger value. Every offset into text
// counts bytes of UTF-8, not UTF-16 units: compare it with utf8Length (src/lib/stream/utf8.ts),
// never with \`text.length\`.
//
// An event, draft or item kind that this file does not list can come from a newer efrd.
// The decoder decodes it to an "other" wrapper, and the app skips it.
`;

/** The first lines of the generated zod schemas. */
export const ZOD_BANNER = `// Generated from docs/protocol.schema.json by \`pnpm gen:protocol\` (scripts/gen-protocol.mjs).
// Do not edit it by hand. Change efr-protocol, run \`cargo xtask protocol-schema\` at the repo
// top, then run \`pnpm gen:protocol\` in app/.
//
// One schema for each definition, after the definitions that it names. Each schema
// satisfies its type in generated.ts, so a drift between the two files is a type error.
// An optional member uses exactOptional: JSON never carries undefined. A u64 is a safe
// integer (z.int()), never a bigint. A wire object strips the members that it does not
// list, so a member from a newer efrd passes.
//
// Use the schemas through the decoder (src/lib/protocol/envelope.ts). It finds an event,
// draft or item kind that this file does not list before a schema reads it, and gives an
// "other" wrapper for it instead of an error.
`;

/**
 * @param {unknown} value
 * @returns {value is Record<string, any>}
 */
function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * True for a schema with annotations and nothing else: it accepts any value.
 *
 * @param {Record<string, unknown>} schema
 * @returns {boolean}
 */
function onlyAnnotations(schema) {
  const keys = Object.keys(schema);
  return keys.length > 0 && keys.every((key) => ANNOTATIONS.has(key));
}

/**
 * Returns a copy of `schema` for json-schema-to-typescript. Every object with a `$ref`
 * and other members is an `allOf` of the reference and the other members; annotations
 * stay on the outer object, so the generator still writes them as comments. A schema
 * with only annotations gets `tsType: "unknown"`. Values that are data, such as a
 * `default` or a `const`, stay as they are.
 *
 * @param {any} schema
 * @returns {any}
 */
export function normalize(schema) {
  if (!isObject(schema)) {
    return schema;
  }
  /** @type {Record<string, unknown>} */
  const copy = {};
  for (const [key, value] of Object.entries(schema)) {
    if (SCHEMA_MAPS.has(key) && isObject(value)) {
      copy[key] = Object.fromEntries(
        Object.entries(value).map(([name, member]) => [name, normalize(member)]),
      );
    } else if (SCHEMA_MEMBERS.has(key)) {
      copy[key] = Array.isArray(value)
        ? value.map((member) => normalize(member))
        : normalize(value);
    } else {
      copy[key] = structuredClone(value);
    }
  }
  if (onlyAnnotations(copy)) {
    return { ...copy, tsType: "unknown" };
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
 * Formats a generated file with oxfmt.
 *
 * @param {string} name
 * @param {string} source
 * @returns {Promise<string>}
 */
async function formatted(name, source) {
  const result = await format(name, source, {});
  if (result.errors.length > 0) {
    const messages = result.errors.map((error) => error.message).join("; ");
    throw new Error(`oxfmt could not format ${name}: ${messages}`);
  }
  return result.code;
}

/**
 * The `$defs` of the schema text.
 *
 * @param {any} schema
 * @returns {Record<string, any>}
 */
function definitions(schema) {
  const defs = schema?.["$defs"];
  if (!isObject(defs)) {
    throw new Error("the schema has no $defs");
  }
  return defs;
}

/**
 * The formatted text of src/lib/protocol/generated.ts for the text of the schema file.
 *
 * @param {string} schemaText
 * @returns {Promise<string>}
 */
export async function generate(schemaText) {
  const schema = JSON.parse(schemaText);
  const defs = definitions(schema);
  const types = await compile(normalize(schema), "EfrProtocol", {
    additionalProperties: false,
    bannerComment: "",
    format: false,
    unknownAny: true,
    unreachableDefinitions: true,
  });
  const source = `${BANNER}\n${withoutReferenceNotes(types)}${runtimeConstants(defs)}`;
  return formatted("generated.ts", source);
}

// The zod emitter. It knows the keywords that efr-protocol's schema uses, and it stops
// with an error at any other keyword, so a new keyword cannot pass without a check.

/** The keywords that the emitter reads, or skips because they accept any value. */
const ZOD_KEYWORDS = new Set([
  ...ANNOTATIONS,
  "$defs",
  "$ref",
  "$schema",
  "additionalProperties",
  "allOf",
  "anyOf",
  "const",
  "contentEncoding",
  "enum",
  "format",
  "items",
  "maxItems",
  "maximum",
  "minItems",
  "minimum",
  "oneOf",
  "pattern",
  "properties",
  "required",
  "type",
]);

/** Keywords that do not change what a schema accepts, for the emitter. */
const ZOD_SKIPPED = new Set([...ANNOTATIONS, "$defs", "$schema", "contentEncoding"]);

/** The schema of a string of each format. */
const STRING_FORMATS = new Map([
  ["date-time", "z.iso.datetime({ offset: true })"],
  ["ipv4", "z.ipv4()"],
  // z.guid() and not z.uuid(): the tests of efrd make ids of version 0, which z.uuid()
  // refuses.
  ["uuid", "z.guid()"],
]);

/**
 * The schema of an integer of each format, and the bounds that it implies.
 *
 * @type {Map<string, { base: string, min: number, max: number | undefined }>}
 */
const INTEGER_FORMATS = new Map([
  ["int32", { base: "z.int32()", min: -(2 ** 31), max: 2 ** 31 - 1 }],
  ["uint8", { base: "z.int().min(0).max(255)", min: 0, max: 255 }],
  ["uint16", { base: "z.int().min(0).max(65535)", min: 0, max: 65_535 }],
  ["uint32", { base: "z.uint32()", min: 0, max: 2 ** 32 - 1 }],
  // z.int() takes only safe integers, so a u64 above 2^53 - 1 fails.
  ["uint64", { base: "z.int().nonnegative()", min: 0, max: undefined }],
]);

/** The most items of an array that the emitter writes as a tuple, as the type generator. */
const MAX_TUPLE = 20;

/**
 * The name of the schema of definition `name`: `ConversationSummary` gives
 * `conversationSummarySchema`.
 *
 * @param {string} name
 * @returns {string}
 */
export function schemaName(name) {
  if (!/^[A-Z][A-Za-z0-9]*$/.test(name)) {
    throw new Error(`$defs.${name} is not a name in PascalCase`);
  }
  return `${name.charAt(0).toLowerCase()}${name.slice(1)}Schema`;
}

/**
 * The definition that `ref` names.
 *
 * @param {string} ref
 * @returns {string}
 */
function refTarget(ref) {
  const match = /^#\/\$defs\/([^/]+)$/.exec(ref);
  if (match?.[1] === undefined) {
    throw new Error(`the reference ${ref} does not name a definition`);
  }
  return match[1];
}

/**
 * The definitions that `schema` names, in sorted order.
 *
 * @param {unknown} schema
 * @returns {string[]}
 */
export function references(schema) {
  /** @type {Set<string>} */
  const found = new Set();
  /** @param {unknown} node */
  const walk = (node) => {
    if (Array.isArray(node)) {
      node.forEach(walk);
      return;
    }
    if (!isObject(node)) {
      return;
    }
    for (const [key, value] of Object.entries(node)) {
      if (key === "$ref" && typeof value === "string") {
        found.add(refTarget(value));
      } else if (SCHEMA_MAPS.has(key) && isObject(value)) {
        Object.values(value).forEach(walk);
      } else if (SCHEMA_MEMBERS.has(key)) {
        walk(value);
      }
    }
  };
  walk(schema);
  return [...found].sort();
}

/**
 * The definitions in groups that reference each other (strongly connected components),
 * each group after every group that it references. A group of one that does not
 * reference itself has no cycle.
 *
 * @param {Record<string, any>} defs
 * @returns {string[][]}
 */
export function definitionOrder(defs) {
  const names = Object.keys(defs).sort();
  /** @type {Map<string, string[]>} */
  const edges = new Map();
  for (const name of names) {
    const targets = references(defs[name]);
    for (const target of targets) {
      if (!(target in defs)) {
        throw new Error(`$defs.${name} names $defs.${target}, which does not exist`);
      }
    }
    edges.set(name, targets);
  }
  // Tarjan's algorithm: it finds each group after the groups that it references.
  /** @type {Map<string, number>} */
  const index = new Map();
  /** @type {Map<string, number>} */
  const low = new Map();
  /** @type {string[]} */
  const stack = [];
  /** @type {Set<string>} */
  const onStack = new Set();
  /** @type {string[][]} */
  const groups = [];
  /** @param {string} name */
  const visit = (name) => {
    index.set(name, index.size);
    low.set(name, index.get(name) ?? 0);
    stack.push(name);
    onStack.add(name);
    for (const target of edges.get(name) ?? []) {
      if (!index.has(target)) {
        visit(target);
        low.set(name, Math.min(low.get(name) ?? 0, low.get(target) ?? 0));
      } else if (onStack.has(target)) {
        low.set(name, Math.min(low.get(name) ?? 0, index.get(target) ?? 0));
      }
    }
    if (low.get(name) === index.get(name)) {
      /** @type {string[]} */
      const group = [];
      let member;
      do {
        member = stack.pop();
        if (member === undefined) {
          throw new Error("the stack of the definition order is empty");
        }
        onStack.delete(member);
        group.push(member);
      } while (member !== name);
      groups.push(group.sort());
    }
  };
  for (const name of names) {
    if (!index.has(name)) {
      visit(name);
    }
  }
  return groups;
}

/**
 * A property key as TypeScript source.
 *
 * @param {string} key
 * @returns {string}
 */
function propertyKey(key) {
  return /^[A-Za-z_$][\w$]*$/.test(key) ? key : JSON.stringify(key);
}

/**
 * The emitter's state for one schema file.
 */
class ZodEmitter {
  /**
   * @param {Record<string, any>} defs
   * @param {string[][]} groups
   */
  constructor(defs, groups) {
    this.defs = defs;
    /** @type {Map<string, number>} The group of each definition. */
    this.groupOf = new Map();
    /** @type {Set<string>} Definitions in a cycle: they are typed, and named lazily. */
    this.cyclic = new Set();
    groups.forEach((group, number) => {
      for (const name of group) {
        this.groupOf.set(name, number);
      }
      const selfReference =
        group.length === 1 && references(defs[group[0] ?? ""]).includes(group[0] ?? "");
      if (group.length > 1 || selfReference) {
        group.forEach((name) => this.cyclic.add(name));
      }
    });
    /** The group of the definition that the emitter writes now. */
    this.current = -1;
  }

  /**
   * The source of a reference to definition `name`. Inside a cycle it is lazy.
   *
   * @param {string} name
   * @returns {string}
   */
  reference(name) {
    const schema = schemaName(name);
    return this.groupOf.get(name) === this.current && this.cyclic.has(name)
      ? `z.lazy(() => ${schema})`
      : schema;
  }

  /**
   * True when definition `name` has a schema that is a zod object, so that the emitter can
   * extend it or put it in a discriminated union.
   *
   * @param {string} name
   * @returns {boolean}
   */
  isObjectDefinition(name) {
    const def = this.defs[name];
    if (this.cyclic.has(name) || !isObject(def)) {
      return false;
    }
    if (typeof def["$ref"] === "string") {
      return false;
    }
    const kinds = structural(def);
    return (
      !kinds.includes("oneOf") &&
      !kinds.includes("anyOf") &&
      !kinds.includes("allOf") &&
      (def["type"] === "object" || (def["type"] === undefined && isObject(def["properties"])))
    );
  }

  /**
   * The constant members of an option of a union, or undefined when the option is not
   * an object whose schema is a zod object.
   *
   * @param {any} option
   * @returns {Map<string, unknown> | undefined}
   */
  constantMembers(option) {
    if (!isObject(option)) {
      return undefined;
    }
    /** @type {Map<string, unknown>} */
    const found = new Map();
    if (typeof option["$ref"] === "string") {
      const target = refTarget(option["$ref"]);
      if (!this.isObjectDefinition(target)) {
        return undefined;
      }
      const inner = this.constantMembers(this.defs[target]);
      if (inner === undefined) {
        return undefined;
      }
      inner.forEach((value, key) => found.set(key, value));
    } else if (
      !(
        option["type"] === "object" ||
        (option["type"] === undefined && isObject(option["properties"]))
      )
    ) {
      return undefined;
    } else if (structural(option).some((key) => ["oneOf", "anyOf", "allOf"].includes(key))) {
      return undefined;
    }
    const properties = isObject(option["properties"]) ? option["properties"] : {};
    const required = Array.isArray(option["required"]) ? option["required"] : [];
    for (const [key, member] of Object.entries(properties)) {
      if (isObject(member) && "const" in member && required.includes(key)) {
        found.set(key, member["const"]);
      }
    }
    return found;
  }

  /**
   * The member that tells the options of a union apart: a required constant of every
   * option, with a different value in each. `kind` comes first.
   *
   * @param {any[]} options
   * @returns {string | undefined}
   */
  discriminator(options) {
    const members = options.map((option) => this.constantMembers(option));
    if (members.some((found) => found === undefined)) {
      return undefined;
    }
    const first = members[0];
    if (first === undefined) {
      return undefined;
    }
    const keys = [...first.keys()].sort((a, b) =>
      a === "kind" ? -1 : b === "kind" ? 1 : a.localeCompare(b),
    );
    return keys.find((key) => {
      const values = members.map((found) => found?.get(key));
      return (
        values.every((value) => typeof value === "string") && new Set(values).size === values.length
      );
    });
  }

  /**
   * The zod source of one schema.
   *
   * @param {any} schema
   * @param {string} path
   * @returns {string}
   */
  expression(schema, path) {
    if (schema === true) {
      return "z.unknown()";
    }
    if (!isObject(schema)) {
      throw new Error(`${path} is not a schema`);
    }
    for (const key of Object.keys(schema)) {
      if (!ZOD_KEYWORDS.has(key)) {
        throw new Error(`${path} has the keyword ${key}, which the zod emitter does not know`);
      }
    }
    const kinds = structural(schema);
    if (kinds.length === 0) {
      return "z.unknown()";
    }
    if (typeof schema["$ref"] === "string") {
      return this.refExpression(schema, kinds, path);
    }
    if (kinds.includes("allOf")) {
      return this.allOfExpression(schema, kinds, path);
    }
    if ("const" in schema) {
      return `z.literal(${JSON.stringify(schema["const"])})`;
    }
    if (Array.isArray(schema["enum"])) {
      return enumExpression(schema["enum"], path);
    }
    if (kinds.includes("oneOf") || kinds.includes("anyOf")) {
      return this.unionExpression(schema, kinds, path);
    }
    return this.typedExpression(schema, path);
  }

  /**
   * A `$ref`, alone or with the members of an object that extend it.
   *
   * @param {Record<string, any>} schema
   * @param {string[]} kinds
   * @param {string} path
   * @returns {string}
   */
  refExpression(schema, kinds, path) {
    const target = refTarget(schema["$ref"]);
    const reference = this.reference(target);
    const rest = kinds.filter((key) => key !== "$ref");
    if (rest.length === 0) {
      return reference;
    }
    const extendsObject =
      rest.every((key) => ["properties", "required", "type"].includes(key)) &&
      (schema["type"] === undefined || schema["type"] === "object");
    if (!extendsObject || !this.isObjectDefinition(target)) {
      throw new Error(`${path} adds members to $defs.${target}, which is not an object`);
    }
    return `${reference}.extend(${this.shape(schema, path)})`;
  }

  /**
   * An `allOf`: the intersection of its members.
   *
   * @param {Record<string, any>} schema
   * @param {string[]} kinds
   * @param {string} path
   * @returns {string}
   */
  allOfExpression(schema, kinds, path) {
    const members = schema["allOf"];
    if (kinds.length !== 1 || !Array.isArray(members) || members.length === 0) {
      throw new Error(`${path} has an allOf with other members, or an empty one`);
    }
    return members
      .map((member, index) => this.expression(member, `${path}/allOf/${index}`))
      .reduce((left, right) => `z.intersection(${left}, ${right})`);
  }

  /**
   * A `oneOf` or `anyOf`. A `null` option makes the rest nullable; string constants are
   * an enum; objects with a constant member that tells them apart are a discriminated
   * union.
   *
   * @param {Record<string, any>} schema
   * @param {string[]} kinds
   * @param {string} path
   * @returns {string}
   */
  unionExpression(schema, kinds, path) {
    const key = kinds.includes("oneOf") ? "oneOf" : "anyOf";
    const options = schema[key];
    const extra = kinds.filter((kind) => kind !== key && kind !== "type");
    if (!Array.isArray(options) || options.length === 0 || extra.length > 0) {
      throw new Error(`${path} has a ${key} with other members, or an empty one`);
    }
    const nulls = options.filter(isNullSchema);
    const rest = options.filter((option) => !isNullSchema(option));
    /** @type {string} */
    let base;
    if (rest.length === 0) {
      return "z.null()";
    } else if (rest.every(isStringConstant)) {
      base = enumExpression(
        rest.map((option) => option["const"]),
        path,
      );
    } else if (rest.length === 1) {
      base = this.expression(rest[0], `${path}/${key}/${options.indexOf(rest[0])}`);
    } else {
      const sources = rest.map((option) =>
        this.expression(option, `${path}/${key}/${options.indexOf(option)}`),
      );
      const tag = this.discriminator(rest);
      base =
        tag === undefined
          ? `z.union([${sources.join(", ")}])`
          : `z.discriminatedUnion(${JSON.stringify(tag)}, [${sources.join(", ")}])`;
    }
    return nulls.length > 0 ? `${base}.nullable()` : base;
  }

  /**
   * A schema with a `type`: a string, number, boolean, array or object, maybe nullable.
   *
   * @param {Record<string, any>} schema
   * @param {string} path
   * @returns {string}
   */
  typedExpression(schema, path) {
    const declared = schema["type"];
    /** @type {unknown[]} */
    let types;
    if (declared === undefined) {
      if (!isObject(schema["properties"])) {
        throw new Error(`${path} has no type`);
      }
      types = ["object"];
    } else {
      types = Array.isArray(declared) ? declared : [declared];
    }
    const nullable = types.includes("null");
    const others = types.filter((type) => type !== "null");
    if (others.length === 0) {
      return "z.null()";
    }
    if (others.length > 1) {
      throw new Error(`${path} has more than one type besides null`);
    }
    const base = this.baseExpression(String(others[0]), schema, path);
    return nullable ? `${base}.nullable()` : base;
  }

  /**
   * The schema of one JSON type.
   *
   * @param {string} type
   * @param {Record<string, any>} schema
   * @param {string} path
   * @returns {string}
   */
  baseExpression(type, schema, path) {
    switch (type) {
      case "string":
        return stringExpression(schema, path);
      case "integer":
        return integerExpression(schema, path);
      case "number":
        return withBounds("z.number()", undefined, undefined, schema);
      case "boolean":
        return "z.boolean()";
      case "array":
        return this.arrayExpression(schema, path);
      case "object":
        return this.objectExpression(schema, path);
      default:
        throw new Error(`${path} has the type ${type}, which the zod emitter does not know`);
    }
  }

  /**
   * An array, or a tuple when it has an exact length (as the type generator writes it).
   *
   * @param {Record<string, any>} schema
   * @param {string} path
   * @returns {string}
   */
  arrayExpression(schema, path) {
    const items = schema["items"];
    if (Array.isArray(items)) {
      throw new Error(`${path} has an items array, which the zod emitter does not know`);
    }
    const item = items === undefined ? "z.unknown()" : this.expression(items, `${path}/items`);
    const min = schema["minItems"];
    const max = schema["maxItems"];
    if (typeof min === "number" && min === max && min <= MAX_TUPLE) {
      return `z.tuple([${Array.from({ length: min }, () => item).join(", ")}])`;
    }
    let source = `z.array(${item})`;
    if (typeof min === "number" && min > 0) {
      source += `.min(${min})`;
    }
    if (typeof max === "number") {
      source += `.max(${max})`;
    }
    return source;
  }

  /**
   * An object. `additionalProperties: false` makes it strict, `true` loose; else it
   * strips the members that it does not list.
   *
   * @param {Record<string, any>} schema
   * @param {string} path
   * @returns {string}
   */
  objectExpression(schema, path) {
    const extra = schema["additionalProperties"];
    const shape = this.shape(schema, path);
    if (extra === false) {
      return `z.strictObject(${shape})`;
    }
    if (extra === true) {
      return `z.looseObject(${shape})`;
    }
    if (extra === undefined) {
      return `z.object(${shape})`;
    }
    return `z.object(${shape}).catchall(${this.expression(extra, `${path}/additionalProperties`)})`;
  }

  /**
   * The shape of an object: one member for each property, `exactOptional` when it is
   * not required.
   *
   * @param {Record<string, any>} schema
   * @param {string} path
   * @returns {string}
   */
  shape(schema, path) {
    const properties = schema["properties"] ?? {};
    if (!isObject(properties)) {
      throw new Error(`${path}/properties is not an object`);
    }
    const required = schema["required"] ?? [];
    if (!Array.isArray(required)) {
      throw new Error(`${path}/required is not an array`);
    }
    for (const name of required) {
      if (!(name in properties)) {
        throw new Error(`${path} requires ${name}, which it does not list`);
      }
    }
    const members = Object.entries(properties).map(([name, member]) => {
      const source = this.expression(member, `${path}/properties/${name}`);
      const optional = required.includes(name) ? "" : ".exactOptional()";
      return `${propertyKey(name)}: ${source}${optional}`;
    });
    return members.length === 0 ? "{}" : `{ ${members.join(", ")} }`;
  }
}

/**
 * The keys of `schema` that change what it accepts.
 *
 * @param {Record<string, unknown>} schema
 * @returns {string[]}
 */
function structural(schema) {
  return Object.keys(schema).filter((key) => !ZOD_SKIPPED.has(key));
}

/**
 * @param {unknown} schema
 * @returns {boolean}
 */
function isNullSchema(schema) {
  return isObject(schema) && schema["type"] === "null" && structural(schema).length === 1;
}

/**
 * @param {unknown} schema
 * @returns {boolean}
 */
function isStringConstant(schema) {
  return (
    isObject(schema) &&
    typeof schema["const"] === "string" &&
    structural(schema).every((key) => key === "const" || key === "type")
  );
}

/**
 * @param {unknown[]} values
 * @param {string} path
 * @returns {string}
 */
function enumExpression(values, path) {
  if (values.length === 0 || !values.every((value) => typeof value === "string")) {
    throw new Error(`${path} has an enum that is not a set of strings`);
  }
  return `z.enum(${JSON.stringify(values)})`;
}

/**
 * A string: the schema of its format, and the check of its pattern.
 *
 * @param {Record<string, any>} schema
 * @param {string} path
 * @returns {string}
 */
function stringExpression(schema, path) {
  const fmt = schema["format"];
  const source = fmt === undefined ? "z.string()" : STRING_FORMATS.get(fmt);
  if (source === undefined) {
    throw new Error(`${path} has the string format ${fmt}, which the zod emitter does not know`);
  }
  const pattern = schema["pattern"];
  if (pattern === undefined) {
    return source;
  }
  if (typeof pattern !== "string") {
    throw new Error(`${path} has a pattern that is not a string`);
  }
  // RegExp.source escapes each "/", so the source is a valid regular expression literal.
  return `${source}.regex(/${new RegExp(pattern).source}/)`;
}

/**
 * An integer: the schema of its format, with the bounds that the format does not imply.
 *
 * @param {Record<string, any>} schema
 * @param {string} path
 * @returns {string}
 */
function integerExpression(schema, path) {
  const fmt = schema["format"];
  if (fmt === undefined) {
    return withBounds("z.int()", undefined, undefined, schema);
  }
  const known = INTEGER_FORMATS.get(fmt);
  if (known === undefined) {
    throw new Error(`${path} has the integer format ${fmt}, which the zod emitter does not know`);
  }
  return withBounds(known.base, known.min, known.max, schema);
}

/**
 * Adds `.min()` and `.max()` for the bounds of `schema` that are stricter than the
 * implied ones.
 *
 * @param {string} base
 * @param {number | undefined} impliedMin
 * @param {number | undefined} impliedMax
 * @param {Record<string, any>} schema
 * @returns {string}
 */
function withBounds(base, impliedMin, impliedMax, schema) {
  let source = base;
  const min = schema["minimum"];
  const max = schema["maximum"];
  if (typeof min === "number" && (impliedMin === undefined || min > impliedMin)) {
    source += `.min(${min})`;
  }
  if (typeof max === "number" && (impliedMax === undefined || max < impliedMax)) {
    source += `.max(${max})`;
  }
  return source;
}

/**
 * The map of the result schemas by method name, when the schema has `MethodResults`.
 *
 * @param {Record<string, any>} defs
 * @returns {string}
 */
function methodResultSchemas(defs) {
  if (!("MethodResults" in defs) || !("MethodName" in defs)) {
    return "";
  }
  return [
    "",
    "/** The schema of the result of each method; for a stream method, of each item. */",
    "export const METHOD_RESULT_SCHEMAS: {",
    "  readonly [M in MethodName]: z.ZodType<MethodResults[M]>;",
    "} = methodResultsSchema.shape;",
    "",
  ].join("\n");
}

/**
 * The source of the schema of each definition, in reference order, without the banner.
 *
 * @param {Record<string, any>} defs
 * @returns {string}
 */
export function zodSchemas(defs) {
  const groups = definitionOrder(defs);
  const emitter = new ZodEmitter(defs, groups);
  /** @type {string[]} */
  const lines = [];
  groups.forEach((group, number) => {
    emitter.current = number;
    for (const name of group) {
      const source = emitter.expression(defs[name], `#/$defs/${name}`);
      lines.push(
        emitter.cyclic.has(name)
          ? `export const ${schemaName(name)}: z.ZodType<${name}> = ${source};`
          : `export const ${schemaName(name)} = ${source} satisfies z.ZodType<${name}>;`,
      );
    }
  });
  const types = Object.keys(defs).sort();
  for (const extra of ["MethodName", "MethodResults"]) {
    if (extra in defs && !types.includes(extra)) {
      types.push(extra);
    }
  }
  return [
    'import { z } from "zod";',
    "",
    `import type { ${types.join(", ")} } from "./generated";`,
    "",
    lines.join("\n\n"),
    methodResultSchemas(defs),
  ].join("\n");
}

/**
 * The formatted text of src/lib/protocol/generated-zod.ts for the text of the schema file.
 *
 * @param {string} schemaText
 * @returns {Promise<string>}
 */
export async function generateZod(schemaText) {
  const defs = definitions(JSON.parse(schemaText));
  return formatted("generated-zod.ts", `${ZOD_BANNER}\n${zodSchemas(defs)}`);
}
