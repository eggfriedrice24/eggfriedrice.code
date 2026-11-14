import { describe, expect, it } from "vitest";

import type { CatalogStatus, ModelInfo } from "@/lib/protocol/generated";
import type { SwitchView } from "@/lib/stream/reducer";

import { switchLine } from "./switch-line";

const PLAN = "openai-subscription";
const PLAN_B = "openai-subscription.b";
const MODELS: ModelInfo[] = [
  { id: "gpt-6.1-sol", provider: PLAN, source: "builtin" },
  { id: "gpt-6.1-sol", provider: PLAN_B, source: "builtin" },
  { id: "gpt-5.5", provider: "openai-api", source: "builtin" },
  { id: "claude-opus-5-5", provider: "anthropic-api", source: "builtin" },
];
const CATALOGS: CatalogStatus[] = [
  { provider: PLAN, origin: "backend" },
  { provider: PLAN_B, origin: "backend" },
];
const CATALOG = { models: MODELS, catalogs: CATALOGS };

function switched(from: SwitchView["from"], to: SwitchView["to"], compacted = false): SwitchView {
  return { from, to, compacted };
}

describe("the switch line", () => {
  it("names the model, the effort and a compaction", () => {
    const line = switchLine(
      switched(
        { model: "claude-opus-5-5", provider: "anthropic-api" },
        { model: "gpt-5.5", provider: "openai-api", effort: "medium" },
        true,
      ),
      undefined,
      CATALOG,
    );
    expect(line.text).toBe("⇄ GPT-5.5 · medium · compacted");
    expect(line.label).toBe("Switched to GPT-5.5, medium effort, the conversation was compacted");
  });

  it("names the account of a model that runs on two", () => {
    const line = switchLine(
      switched(
        { model: "claude-opus-5-5", provider: "anthropic-api" },
        { model: "gpt-6.1-sol", provider: PLAN_B, effort: "medium" },
        true,
      ),
      undefined,
      CATALOG,
    );
    expect(line.text).toBe("⇄ GPT-6.1 Sol on b · medium · compacted");
    const onDefault = switchLine(
      switched({ model: "gpt-5.5" }, { model: "gpt-6.1-sol", provider: PLAN, effort: "medium" }),
      undefined,
      CATALOG,
    );
    expect(onDefault.text).toBe("⇄ GPT-6.1 Sol on ChatGPT plan · medium");
  });

  it("always names the account of a switch of account alone, also without the list", () => {
    const line = switchLine(
      switched(
        { model: "gpt-6.1-sol", provider: PLAN },
        { model: "gpt-6.1-sol", provider: PLAN_B },
      ),
      "medium",
      undefined,
    );
    expect(line.text).toBe("⇄ GPT-6.1 Sol on b · medium");
  });

  it("names the account by its label", () => {
    const labelled = {
      models: MODELS,
      catalogs: [{ ...CATALOGS[1], label: "Work", origin: "backend" as const }],
    };
    const line = switchLine(
      switched(
        { model: "gpt-6.1-sol", provider: PLAN },
        { model: "gpt-6.1-sol", provider: PLAN_B },
      ),
      undefined,
      labelled,
    );
    expect(line.text).toBe("⇄ GPT-6.1 Sol on Work");
  });

  it("takes the default effort of the switch, else the effort of the turn", () => {
    const line = switchLine(
      switched({ model: "gpt-5.5" }, { model: "claude-opus-5-5", default_effort: "high" }),
      "low",
      CATALOG,
    );
    expect(line.text).toBe("⇄ Opus 5.5 · high");
    expect(
      switchLine(switched({ model: "gpt-5.5" }, { model: "claude-opus-5-5" }), "low", CATALOG).text,
    ).toBe("⇄ Opus 5.5 · low");
  });
});
