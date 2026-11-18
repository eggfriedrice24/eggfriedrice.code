// The key-value store of the app's small settings: the composer drafts and the
// preferences. The app gives it `Storage` of expo-sqlite/kv-store (src/lib/native/kv.ts);
// tests give it a store in memory (src/lib/store/testing.ts). It holds no secret: the
// pairing record is in expo-secure-store.

import type { ZodType } from "zod";

/**
 * The saved JSON of `key` as `schema` reads it, or undefined when there is none, when it
 * is broken, or when the read fails. A store reads its value once, when it is made.
 */
export function readSaved<T>(
  storage: KeyValueStorage,
  key: string,
  schema: ZodType<T>,
  onError?: (error: unknown) => void,
): T | undefined {
  let json: string | null;
  try {
    json = storage.getItemSync(key);
  } catch (error) {
    onError?.(error);
    return undefined;
  }
  if (json === null) {
    return undefined;
  }
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return undefined;
  }
  const parsed = schema.safeParse(value);
  return parsed.success ? parsed.data : undefined;
}

/** The part of expo-sqlite's kv-store `Storage` that the app's stores use. */
export interface KeyValueStorage {
  /**
   * The value of `key`, read at once. A store reads its value once, when it is made, so
   * a screen shows the saved value in its first frame.
   */
  getItemSync(key: string): string | null;
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
  removeItem(key: string): Promise<void>;
}
