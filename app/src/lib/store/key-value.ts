// The key-value store of the app's small settings: the composer drafts and the
// preferences. The app gives it `Storage` of expo-sqlite/kv-store (src/lib/native/kv.ts);
// tests give it a store in memory (src/lib/store/testing.ts). It holds no secret: the
// pairing record is in expo-secure-store.

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
