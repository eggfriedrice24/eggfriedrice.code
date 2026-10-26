import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MemoryKeyValue } from "@/lib/store/testing";
import {
  BLANK_ROOT,
  DRAFTS_KEY,
  DRAFT_SAVE_MS,
  createDrafts,
  draftKey,
  selectDraft,
} from "./drafts";
import { CONVERSATION_A, DESKTOP, LAPTOP } from "./outbox-testing";

const A = draftKey(DESKTOP, CONVERSATION_A);
const ROOT = draftKey(DESKTOP, BLANK_ROOT);
const LAPTOP_ROOT = draftKey(LAPTOP, BLANK_ROOT);

function saved(storage: MemoryKeyValue): unknown {
  const json = storage.items.get(DRAFTS_KEY);
  return json === undefined ? undefined : JSON.parse(json);
}

describe("the drafts", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("changes the store at once and saves 200 ms after the typing stops", async () => {
    const storage = new MemoryKeyValue();
    const drafts = createDrafts({ storage });
    drafts.set(A, "h");
    vi.advanceTimersByTime(150);
    drafts.set(A, "he");
    vi.advanceTimersByTime(150);
    drafts.set(A, "hey");
    expect(drafts.text(A)).toBe("hey");
    expect(selectDraft(A)(drafts.store.get())).toBe("hey");
    expect(selectDraft(ROOT)(drafts.store.get())).toBe("");
    vi.advanceTimersByTime(DRAFT_SAVE_MS - 1);
    await drafts.settled();
    expect(storage.writes).toBe(0);
    vi.advanceTimersByTime(1);
    await drafts.settled();
    expect(storage.writes).toBe(1);
    expect(saved(storage)).toEqual({ [A]: "hey" });
  });

  it("keeps one draft per conversation and one for the blank root of each machine", async () => {
    const storage = new MemoryKeyValue();
    const drafts = createDrafts({ storage });
    drafts.set(A, "for a");
    drafts.set(ROOT, "for a new one");
    drafts.set(LAPTOP_ROOT, "on the laptop");
    vi.advanceTimersByTime(DRAFT_SAVE_MS);
    await drafts.settled();
    expect(saved(storage)).toEqual({
      [A]: "for a",
      [ROOT]: "for a new one",
      [LAPTOP_ROOT]: "on the laptop",
    });
    const again = createDrafts({ storage });
    expect(again.text(ROOT)).toBe("for a new one");
  });

  it("keeps the store object when nothing changes", () => {
    const drafts = createDrafts({ storage: new MemoryKeyValue() });
    drafts.set(A, "x");
    const state = drafts.store.get();
    drafts.set(A, "x");
    drafts.clear(ROOT);
    drafts.removeMachine(LAPTOP);
    expect(drafts.store.get()).toBe(state);
  });

  it("removes a cleared draft, and the key when no draft is left", async () => {
    const storage = new MemoryKeyValue();
    const drafts = createDrafts({ storage });
    drafts.set(A, "sent soon");
    drafts.flush();
    await drafts.settled();
    expect(saved(storage)).toEqual({ [A]: "sent soon" });
    drafts.clear(A);
    expect(drafts.text(A)).toBe("");
    vi.advanceTimersByTime(DRAFT_SAVE_MS);
    await drafts.settled();
    expect(storage.items.has(DRAFTS_KEY)).toBe(false);
    drafts.set(A, "x");
    drafts.set(A, "");
    expect(drafts.store.get().texts.size).toBe(0);
  });

  it("saves a waiting change at once on flush, and only once", async () => {
    const storage = new MemoryKeyValue();
    const drafts = createDrafts({ storage });
    drafts.flush();
    await drafts.settled();
    expect(storage.writes).toBe(0);
    drafts.set(A, "going to the background");
    drafts.flush();
    await drafts.settled();
    expect(saved(storage)).toEqual({ [A]: "going to the background" });
    vi.advanceTimersByTime(DRAFT_SAVE_MS);
    await drafts.settled();
    expect(storage.writes).toBe(1);
  });

  it("removes the drafts of a forgotten machine from the disk at once", async () => {
    const storage = new MemoryKeyValue();
    const drafts = createDrafts({ storage });
    drafts.set(A, "a");
    drafts.set(ROOT, "root");
    drafts.set(LAPTOP_ROOT, "laptop");
    drafts.flush();
    await drafts.settled();
    drafts.removeMachine(DESKTOP);
    await drafts.settled();
    expect(saved(storage)).toEqual({ [LAPTOP_ROOT]: "laptop" });
    expect(drafts.text(A)).toBe("");
  });

  it("starts empty when the saved value is broken or the store fails", async () => {
    const broken = new MemoryKeyValue();
    broken.items.set(DRAFTS_KEY, "{");
    expect(createDrafts({ storage: broken }).store.get().texts.size).toBe(0);
    broken.items.set(DRAFTS_KEY, JSON.stringify({ [A]: 3 }));
    expect(createDrafts({ storage: broken }).store.get().texts.size).toBe(0);

    const failing = new MemoryKeyValue();
    failing.failing = true;
    const errors: unknown[] = [];
    const drafts = createDrafts({
      storage: failing,
      onStorageError: (error) => errors.push(error),
    });
    expect(errors).toHaveLength(1);
    drafts.set(A, "still typed");
    drafts.flush();
    await drafts.settled();
    expect(errors).toHaveLength(2);
    expect(drafts.text(A)).toBe("still typed");
  });
});
