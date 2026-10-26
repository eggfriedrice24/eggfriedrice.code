import { describe, expect, it } from "vitest";

import { MemoryKeyValue } from "@/lib/store/testing";
import {
  DEFAULT_PREFERENCES,
  PREFERENCES_KEY,
  createPreferences,
  parsePreferences,
} from "./preferences";
import { savedPreferencesSchema } from "./preferences-schemas";

const DESKTOP = "019a9b1c-3d00-7a10-8b20-000000000001";
const LAPTOP = "019a9b1c-3d00-7a10-8b20-000000000002";

describe("the preferences", () => {
  it("start with haptics on, steer and no machine", () => {
    const preferences = createPreferences({ storage: new MemoryKeyValue() });
    expect(preferences.store.get()).toEqual({
      haptics: true,
      sendMode: "steer",
      lastMachine: null,
    });
    // The schema falls back to the same defaults.
    expect(savedPreferencesSchema.parse({})).toEqual(DEFAULT_PREFERENCES);
  });

  it("change at once, write each change and read it back on the next launch", async () => {
    const storage = new MemoryKeyValue();
    const preferences = createPreferences({ storage });
    preferences.setHaptics(false);
    preferences.setSendMode("queue");
    preferences.setLastMachine(DESKTOP);
    expect(preferences.store.get()).toEqual({
      haptics: false,
      sendMode: "queue",
      lastMachine: DESKTOP,
    });
    await preferences.settled();
    expect(storage.writes).toBe(3);
    expect(createPreferences({ storage }).store.get()).toEqual(preferences.store.get());
  });

  it("write nothing and keep the store object when nothing changes", async () => {
    const storage = new MemoryKeyValue();
    const preferences = createPreferences({ storage });
    const state = preferences.store.get();
    preferences.setHaptics(true);
    preferences.setSendMode("steer");
    preferences.setLastMachine(null);
    preferences.forgetMachine(DESKTOP);
    await preferences.settled();
    expect(preferences.store.get()).toBe(state);
    expect(storage.writes).toBe(0);
  });

  it("forget the last machine only when it is the forgotten one", () => {
    const preferences = createPreferences({ storage: new MemoryKeyValue() });
    preferences.setLastMachine(DESKTOP);
    preferences.forgetMachine(LAPTOP);
    expect(preferences.store.get().lastMachine).toBe(DESKTOP);
    preferences.forgetMachine(DESKTOP);
    expect(preferences.store.get().lastMachine).toBeNull();
  });

  it("keep each good field of a saved value and default the others", () => {
    expect(parsePreferences(null)).toBe(DEFAULT_PREFERENCES);
    expect(parsePreferences("{")).toBe(DEFAULT_PREFERENCES);
    expect(parsePreferences("[]")).toBe(DEFAULT_PREFERENCES);
    expect(
      parsePreferences(
        JSON.stringify({ haptics: false, sendMode: "later", lastMachine: "x", extra: 1 }),
      ),
    ).toEqual({ haptics: false, sendMode: "steer", lastMachine: null });
    expect(parsePreferences(JSON.stringify({ lastMachine: LAPTOP }))).toEqual({
      haptics: true,
      sendMode: "steer",
      lastMachine: LAPTOP,
    });
  });

  it("go on in memory when the store fails", async () => {
    const storage = new MemoryKeyValue();
    storage.items.set(PREFERENCES_KEY, JSON.stringify({ haptics: false }));
    storage.failing = true;
    const errors: unknown[] = [];
    const preferences = createPreferences({
      storage,
      onStorageError: (error) => errors.push(error),
    });
    expect(preferences.store.get()).toEqual(DEFAULT_PREFERENCES);
    preferences.setSendMode("queue");
    await preferences.settled();
    expect(errors).toHaveLength(2);
    expect(preferences.store.get().sendMode).toBe("queue");
  });
});
