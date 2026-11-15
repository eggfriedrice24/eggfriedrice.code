import { describe, expect, it } from "vitest";

import type { ModelInfo, ModelsListResult } from "@/lib/protocol/generated";

import {
  choiceOf,
  COMPACTS_TEXT,
  currentSettings,
  defaultSelection,
  modelButton,
  normalEffort,
  openingSelection,
  pickRow,
  selectionChanged,
  sheetView,
  type SheetInput,
} from "./model-sheet";

const WORK = "anthropic-api.work";
const PERSONAL = "anthropic-api.personal";
const PLAN = "openai-subscription";
const PLAN_B = "openai-subscription.b";

/** 2026-11-14 at 14:08 local time. */
const NOW = new Date(2026, 10, 14, 14, 8).getTime();
const at = (hours: number, minutes: number): string =>
  new Date(2026, 10, 14, hours, minutes).toISOString();

function model(id: string, provider: string, extra: Partial<ModelInfo> = {}): ModelInfo {
  return { id, provider, source: "builtin", ...extra };
}

const OPUS: Partial<ModelInfo> = {
  efforts: ["low", "medium", "high"],
  default_effort: "high",
  context_window: 200_000,
};
const SONNET: Partial<ModelInfo> = {
  efforts: ["low", "medium", "high"],
  default_effort: "high",
  context_window: 200_000,
};
const SOL: Partial<ModelInfo> = {
  efforts: ["minimal", "low", "medium", "high", "xhigh"],
  default_effort: "medium",
  context_window: 400_000,
};

/** The accounts of the boards: work, personal (signed out), the ChatGPT plan and b. */
const LIST: ModelsListResult = {
  models: [
    model("claude-opus-5-5", WORK, { ...OPUS, default: true }),
    model("claude-sonnet-5", WORK, SONNET),
    model("claude-haiku-4-5", WORK, { efforts: [], context_window: 200_000 }),
    model("claude-opus-5-5", PERSONAL, OPUS),
    model("gpt-6.1-sol", PLAN, SOL),
    model("gpt-6.1-sol", PLAN_B, SOL),
  ],
  catalogs: [
    { provider: WORK, origin: "backend" },
    { provider: PERSONAL, origin: "cache", signed_out: true },
    { provider: PLAN, origin: "backend" },
    { provider: PLAN_B, origin: "backend" },
  ],
};

function input(extra: Partial<SheetInput>): SheetInput {
  return {
    list: LIST,
    current: currentSettings(LIST, undefined, undefined),
    selection: undefined,
    machineName: "desktop",
    now: NOW,
    running: false,
    asks: true,
    cost: undefined,
    costPending: false,
    ...extra,
  };
}

describe("what the next message uses", () => {
  it("is the choice, else the model of the conversation, else the machine's default", () => {
    expect(currentSettings(LIST, undefined, undefined)).toEqual({
      model: "claude-opus-5-5",
      provider: WORK,
      effort: "high",
      source: "default",
    });
    const turn = { model: "gpt-6.1-sol", provider: PLAN_B, effort: undefined };
    expect(currentSettings(LIST, undefined, turn)).toEqual({
      model: "gpt-6.1-sol",
      provider: PLAN_B,
      effort: "medium",
      source: "turn",
    });
    expect(currentSettings(LIST, { model: "claude-sonnet-5", effort: "medium" }, turn)).toEqual({
      model: "claude-sonnet-5",
      provider: WORK,
      effort: "medium",
      source: "choice",
    });
    expect(currentSettings(undefined, undefined, undefined)).toBeUndefined();
  });

  it("puts the thumb on the effort, else the default, else the middle step", () => {
    const sol = LIST.models[4];
    expect(normalEffort(sol, "high")).toBe("high");
    expect(normalEffort(sol, "max")).toBe("medium");
    expect(normalEffort({ ...model("x", PLAN), efforts: ["a", "b", "c"] }, undefined)).toBe("b");
    expect(normalEffort(LIST.models[2], "high")).toBeUndefined();
  });
});

describe("the sections", () => {
  it("lists the accounts in the order of models.list, by kind and id", () => {
    const view = sheetView(input({}));
    expect(view.sections.map((section) => section.title)).toEqual([
      "Claude API · work",
      "Claude API · personal",
      "ChatGPT plan",
      "ChatGPT plan · b",
    ]);
    expect(view.sections[0]?.rows.map((row) => row.name)).toEqual([
      "Opus 5.5",
      "Sonnet 5",
      "Haiku 4.5",
    ]);
    expect(view.sections[0]?.rows[0]?.checked).toBe(true);
    expect(view.sections[0]?.rows[0]?.logo).toBe("claude");
    expect(view.sections[2]?.rows[0]?.logo).toBe("openai");
  });

  it("dims a signed-out account and says so", () => {
    const personal = sheetView(input({})).sections[1];
    expect(personal?.note).toEqual({ kind: "signed_out", text: "signed out on desktop" });
    expect(personal?.dim).toBe(true);
    expect(personal?.rows[0]?.disabled).toBe(true);
  });

  it("shows a limit from 80 %, and dims a full account with its reset and age", () => {
    const list: ModelsListResult = {
      ...LIST,
      catalogs: [
        { provider: WORK, origin: "backend" },
        {
          provider: PLAN,
          origin: "backend",
          limits: {
            observed_at: at(14, 0),
            windows: [{ minutes: 300, used_percent: 92, resets_at: at(16, 0) }],
          },
        },
        {
          provider: PLAN_B,
          origin: "backend",
          limits: {
            observed_at: at(13, 56),
            windows: [{ minutes: 300, used_percent: 100, resets_at: at(14, 20) }],
          },
        },
      ],
    };
    const view = sheetView(input({ list }));
    const plan = view.sections.find((section) => section.key === PLAN);
    const b = view.sections.find((section) => section.key === PLAN_B);
    expect(plan?.note).toEqual({ kind: "warn", text: "5-hour 92%" });
    expect(plan?.dim).toBe(false);
    expect(b?.note).toEqual({ kind: "reached", text: "limit reached · resets 14:20 · 12m ago" });
    expect(b?.rows[0]?.disabled).toBe(true);
    // After the reset the limit counts no more.
    const later = sheetView(input({ list, now: new Date(2026, 10, 14, 14, 21).getTime() }));
    expect(later.sections.find((section) => section.key === PLAN_B)?.note).toBeUndefined();
  });

  it("keeps a model that the machine lists no more as the one row with words", () => {
    const current = currentSettings(
      LIST,
      { model: "claude-sonnet-4-5", provider: WORK },
      undefined,
    );
    const view = sheetView(input({ current }));
    const row = view.sections[0]?.rows[0];
    expect(row?.name).toBe("Sonnet 4.5");
    expect(row?.gone).toBe("gone from desktop");
    expect(row?.checked).toBe(true);
    expect(row?.disabled).toBe(true);
  });
});

describe("the effort and the quiet line", () => {
  it("has the efforts of the selected model, its default marked", () => {
    const current = currentSettings(LIST, undefined, undefined);
    const view = sheetView(input({ selection: openingSelection(LIST, current, undefined) }));
    expect(view.effort).toEqual({ steps: ["low", "medium", "high"], index: 2, defaultIndex: 2 });
    // Opus runs on work and on personal, so the line names the account.
    expect(view.quiet).toEqual({
      text: "Opus 5.5 · 200k context · on Claude API · work · desktop default",
      useDefault: false,
    });
    expect(view.confirm).toBeUndefined();
  });

  it("says 'chosen here' with 'Use default' for a chosen model", () => {
    const current = currentSettings(
      LIST,
      { model: "claude-sonnet-5", effort: "medium" },
      undefined,
    );
    const view = sheetView(
      input({ current, selection: openingSelection(LIST, current, undefined) }),
    );
    expect(view.quiet).toEqual({
      text: "Sonnet 5 · 200k context · chosen here",
      useDefault: true,
    });
    expect(view.effort?.index).toBe(1);
    expect(view.effort?.defaultIndex).toBe(2);
    const back = defaultSelection(LIST);
    expect(back).toEqual({ model: "claude-opus-5-5", provider: WORK, effort: "high" });
    expect(sheetView(input({ current, selection: back })).confirm?.label).toBe(
      "Use Opus 5.5 · work · high",
    );
  });

  it("has no slider for a model without efforts", () => {
    const selection = pickRow(LIST, undefined, "claude-haiku-4-5", WORK);
    const view = sheetView(input({ selection }));
    expect(view.effort).toBeUndefined();
    expect(view.quiet?.text).toBe("Haiku 4.5 · 200k context · no effort setting");
    expect(view.confirm?.label).toBe("Use Haiku 4.5");
  });

  it("names the account of a model that runs on two", () => {
    const selection = pickRow(LIST, undefined, "gpt-6.1-sol", PLAN_B);
    const view = sheetView(input({ selection }));
    expect(view.quiet?.text).toBe("GPT-6.1 Sol · 400k context · on ChatGPT plan · b");
    expect(view.confirm?.label).toBe("Use GPT-6.1 Sol · b · medium");
  });
});

describe("the confirm", () => {
  it("shows only when something changed", () => {
    const current = currentSettings(LIST, undefined, undefined);
    expect(selectionChanged(current, openingSelection(LIST, current, undefined))).toBe(false);
    const sonnet = pickRow(LIST, current, "claude-sonnet-5", WORK);
    expect(sonnet.effort).toBe("high");
    expect(selectionChanged(current, sonnet)).toBe(true);
    expect(selectionChanged(current, { ...sonnet, model: "claude-opus-5-5" })).toBe(false);
  });

  it("names the account for a switch of account alone", () => {
    const turn = { model: "gpt-6.1-sol", provider: PLAN, effort: "medium" };
    const current = currentSettings(LIST, undefined, turn);
    const selection = openingSelection(LIST, current, { model: "gpt-6.1-sol", provider: PLAN_B });
    expect(selection).toEqual({ model: "gpt-6.1-sol", provider: PLAN_B, effort: "medium" });
    expect(sheetView(input({ current, selection })).confirm?.label).toBe("Use ChatGPT plan · b");
  });

  it("warns of a compaction while the phone asks, and says nothing of a cold cache", () => {
    const selection = pickRow(LIST, undefined, "gpt-6.1-sol", PLAN);
    const compacts = sheetView(input({ selection, cost: "compacts" })).confirm;
    expect(compacts).toEqual({
      label: "Switch and compact",
      next: false,
      compacts: true,
      pending: false,
    });
    expect(COMPACTS_TEXT).toBe("Compacts this conversation and starts a cold cache.");
    expect(sheetView(input({ selection, cost: "cold_cache" })).confirm?.compacts).toBe(false);
    expect(sheetView(input({ selection, cost: "compacts", asks: false })).confirm?.label).toBe(
      "Use GPT-6.1 Sol · ChatGPT plan · medium",
    );
    expect(sheetView(input({ selection, costPending: true })).confirm?.pending).toBe(true);
    expect(sheetView(input({ selection, running: true })).confirm?.next).toBe(true);
  });

  it("stores the model, the account and the effort", () => {
    expect(choiceOf({ model: "gpt-6.1-sol", provider: PLAN_B, effort: "medium" })).toEqual({
      model: "gpt-6.1-sol",
      provider: PLAN_B,
      effort: "medium",
    });
    expect(choiceOf({ model: "x", provider: undefined, effort: undefined })).toEqual({
      model: "x",
    });
  });
});

describe("the model button", () => {
  it("names the model and the effort, and the account of a model that runs on two", () => {
    const opus = currentSettings(LIST, undefined, undefined);
    expect(modelButton(LIST, opus)?.text).toBe("Opus 5.5 · work · high");
    const b = currentSettings(LIST, undefined, {
      model: "gpt-6.1-sol",
      provider: PLAN_B,
      effort: "medium",
    });
    expect(modelButton(LIST, b)).toEqual({
      text: "GPT-6.1 Sol · b · medium",
      label: "Model: GPT-6.1 Sol on b, medium effort",
      chosen: false,
    });
    const plan = currentSettings(LIST, undefined, {
      model: "gpt-6.1-sol",
      provider: PLAN,
      effort: "medium",
    });
    expect(modelButton(LIST, plan)?.text).toBe("GPT-6.1 Sol · ChatGPT plan · medium");
    const haiku = currentSettings(LIST, { model: "claude-haiku-4-5" }, undefined);
    expect(modelButton(LIST, haiku)?.text).toBe("Haiku 4.5");
    expect(modelButton(LIST, undefined)).toBeUndefined();
  });

  it("says when the user chose the model for this conversation, as MS-Switched does", () => {
    const chosen = currentSettings(
      LIST,
      { model: "gpt-6.1-sol", provider: PLAN_B, effort: "medium" },
      undefined,
    );
    expect(modelButton(LIST, chosen)).toEqual({
      text: "GPT-6.1 Sol · b · medium",
      label: "Model: GPT-6.1 Sol on b, medium effort, chosen for this conversation",
      chosen: true,
    });
    expect(modelButton(LIST, currentSettings(LIST, undefined, undefined))?.chosen).toBe(false);
  });
});
