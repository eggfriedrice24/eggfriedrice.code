import { describe, expect, it } from "vitest";

import { MemoryKeyValue } from "@/lib/store/testing";

import { draftKey } from "./drafts";
import {
  createModelChoices,
  MODEL_CHOICES_KEY,
  sameChoice,
  selectModelChoice,
  selectQuiet,
} from "./model-choices";
import { CONVERSATION_A, DESKTOP, LAPTOP } from "./outbox-testing";

const KEY = draftKey(DESKTOP, CONVERSATION_A);

describe("the model choices", () => {
  it("keeps a choice per composer with its account, saves it at once, and reads it back", async () => {
    const storage = new MemoryKeyValue();
    const choices = createModelChoices({ storage });
    choices.set(KEY, { model: "gpt-6.1-sol", provider: "openai-subscription.b", effort: "medium" });
    expect(selectModelChoice(KEY)(choices.store.get())).toEqual({
      model: "gpt-6.1-sol",
      provider: "openai-subscription.b",
      effort: "medium",
    });
    await choices.settled();
    const again = createModelChoices({ storage });
    expect(again.choice(KEY)).toEqual({
      model: "gpt-6.1-sol",
      provider: "openai-subscription.b",
      effort: "medium",
    });
  });

  it("goes back to what efrd runs, and removes the key when nothing is left", async () => {
    const storage = new MemoryKeyValue();
    const choices = createModelChoices({ storage });
    choices.set(KEY, { model: "gpt-5.5" });
    choices.set(KEY, undefined);
    await choices.settled();
    expect(choices.choice(KEY)).toBeUndefined();
    expect(storage.getItemSync(MODEL_CHOICES_KEY)).toBeNull();
  });

  it("keeps 'Don't ask again here' per composer", async () => {
    const storage = new MemoryKeyValue();
    const choices = createModelChoices({ storage });
    expect(choices.quiet(KEY)).toBe(false);
    choices.setQuiet(KEY, true);
    expect(selectQuiet(KEY)(choices.store.get())).toBe(true);
    expect(choices.quiet(draftKey(LAPTOP, CONVERSATION_A))).toBe(false);
    await choices.settled();
    expect(createModelChoices({ storage }).quiet(KEY)).toBe(true);
    choices.setQuiet(KEY, false);
    await choices.settled();
    expect(storage.getItemSync(MODEL_CHOICES_KEY)).toBeNull();
  });

  it("forgets the choices of a forgotten machine", () => {
    const choices = createModelChoices({ storage: new MemoryKeyValue() });
    choices.set(KEY, { model: "gpt-5.5" });
    choices.setQuiet(KEY, true);
    choices.set(draftKey(LAPTOP, CONVERSATION_A), { model: "gpt-5.5" });
    choices.removeMachine(DESKTOP);
    expect(choices.choice(KEY)).toBeUndefined();
    expect(choices.quiet(KEY)).toBe(false);
    expect(choices.choice(draftKey(LAPTOP, CONVERSATION_A))).toEqual({ model: "gpt-5.5" });
  });

  it("reads none from a broken save, and keeps the good part of a half broken one", () => {
    const storage = new MemoryKeyValue();
    void storage.setItem(MODEL_CHOICES_KEY, "{not json");
    expect(createModelChoices({ storage }).choice(KEY)).toBeUndefined();
    void storage.setItem(MODEL_CHOICES_KEY, JSON.stringify({ choices: 3, quiet: [KEY] }));
    const half = createModelChoices({ storage });
    expect(half.choice(KEY)).toBeUndefined();
    expect(half.quiet(KEY)).toBe(true);
  });

  it("compares the model, the account and the effort", () => {
    expect(sameChoice({ model: "a" }, { model: "a" })).toBe(true);
    expect(sameChoice({ model: "a", provider: "x" }, { model: "a" })).toBe(false);
    expect(sameChoice({ model: "a", effort: "low" }, { model: "a", effort: "high" })).toBe(false);
    expect(sameChoice(undefined, undefined)).toBe(true);
  });
});
