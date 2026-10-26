// Small guards for JSON values that pairing reads: results from efrd and the stored
// pairing record.

/** A JSON object. */
export type JsonObject = Readonly<Record<string, unknown>>;

/** True for a JSON object: not null, not an array. */
export function isJsonObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** `value` as an array of strings, or undefined when it is not one. */
export function stringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const strings: string[] = [];
  for (const item of value as readonly unknown[]) {
    if (typeof item !== "string") {
      return undefined;
    }
    strings.push(item);
  }
  return strings;
}
