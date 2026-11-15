import { describe, expect, it } from "vitest";

import { MemoryKeyValue } from "@/lib/store/testing";

import { draftKey } from "./drafts";
import {
  createModelChoices,
  effectiveMode,
  favouriteOf,
  MODEL_CHOICES_KEY,
  sameChoice,
  selectFavourites,
  selectMode,
  selectModelChoice,
  selectPlan,
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

describe("the mode, the favourites and the plan", () => {
  it("keeps auto per composer, cautious by default, and saves it", async () => {
    const storage = new MemoryKeyValue();
    const choices = createModelChoices({ storage });
    expect(choices.mode(KEY)).toBe("cautious");
    choices.setMode(KEY, "auto");
    expect(selectMode(KEY)(choices.store.get())).toBe("auto");
    expect(choices.mode(draftKey(LAPTOP, CONVERSATION_A))).toBe("cautious");
    await choices.settled();
    expect(createModelChoices({ storage }).mode(KEY)).toBe("auto");
    choices.setMode(KEY, "cautious");
    await choices.settled();
    expect(storage.getItemSync(MODEL_CHOICES_KEY)).toBeNull();
  });

  it("keeps one favourite for each account of a machine; a new one moves the star", async () => {
    const storage = new MemoryKeyValue();
    const choices = createModelChoices({ storage });
    const plan = "openai-subscription";
    choices.setFavourite(DESKTOP, plan, "gpt-6-luna");
    choices.setFavourite(DESKTOP, "openai-subscription.b", "gpt-6.1-sol");
    choices.setFavourite(DESKTOP, plan, "gpt-6-astra");
    const favourites = selectFavourites(choices.store.get());
    expect(favouriteOf(favourites, DESKTOP, plan)).toBe("gpt-6-astra");
    expect(favouriteOf(favourites, DESKTOP, "openai-subscription.b")).toBe("gpt-6.1-sol");
    expect(favouriteOf(favourites, LAPTOP, plan)).toBeUndefined();
    await choices.settled();
    const again = createModelChoices({ storage });
    expect(favouriteOf(again.store.get().favourites, DESKTOP, plan)).toBe("gpt-6-astra");
    choices.setFavourite(DESKTOP, plan, undefined);
    expect(favouriteOf(choices.store.get().favourites, DESKTOP, plan)).toBeUndefined();
  });

  it("keeps the account of a choice or a message as the last plan of the machine", () => {
    const choices = createModelChoices({ storage: new MemoryKeyValue() });
    expect(selectPlan(DESKTOP)(choices.store.get())).toBeUndefined();
    choices.set(KEY, { model: "gpt-6.1-sol", provider: "openai-subscription.b" });
    expect(selectPlan(DESKTOP)(choices.store.get())).toBe("openai-subscription.b");
    choices.set(KEY, { model: "gpt-5.5" });
    expect(selectPlan(DESKTOP)(choices.store.get())).toBe("openai-subscription.b");
    choices.setPlan(DESKTOP, "anthropic-api");
    expect(selectPlan(DESKTOP)(choices.store.get())).toBe("anthropic-api");
    expect(selectPlan(LAPTOP)(choices.store.get())).toBeUndefined();
  });

  it("forgets the mode, the favourites and the plan of a forgotten machine", () => {
    const choices = createModelChoices({ storage: new MemoryKeyValue() });
    choices.setMode(KEY, "auto");
    choices.setFavourite(DESKTOP, "anthropic-api", "claude-opus-5-5");
    choices.setFavourite(LAPTOP, "anthropic-api", "claude-sonnet-5-5");
    choices.setPlan(DESKTOP, "anthropic-api");
    choices.removeMachine(DESKTOP);
    const state = choices.store.get();
    expect(choices.mode(KEY)).toBe("cautious");
    expect(favouriteOf(state.favourites, DESKTOP, "anthropic-api")).toBeUndefined();
    expect(favouriteOf(state.favourites, LAPTOP, "anthropic-api")).toBe("claude-sonnet-5-5");
    expect(state.plans.get(DESKTOP)).toBeUndefined();
  });
});

describe("auto and the Face ID unlock", () => {
  it("runs auto only while the unlock is on", () => {
    expect(effectiveMode("auto", true)).toBe("auto");
    expect(effectiveMode("auto", false)).toBe("cautious");
    expect(effectiveMode("cautious", true)).toBe("cautious");
    expect(effectiveMode("cautious", false)).toBe("cautious");
  });

  it("sets every composer back to cautious and saves it (D3)", async () => {
    const storage = new MemoryKeyValue();
    const choices = createModelChoices({ storage });
    choices.setMode(KEY, "auto");
    choices.setMode(draftKey(LAPTOP, CONVERSATION_A), "auto");
    choices.clearAuto();
    expect(choices.mode(KEY)).toBe("cautious");
    expect(choices.mode(draftKey(LAPTOP, CONVERSATION_A))).toBe("cautious");
    await choices.settled();
    expect(createModelChoices({ storage }).mode(KEY)).toBe("cautious");
  });
});
