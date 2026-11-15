// The models and the accounts of the demo machines, the example set of the boards
// (S-ModelSheet, AC-Limits, AC-Out): Claude API · work and personal (signed out), the
// ChatGPT plan at 92 % of its 5-hour window, and ChatGPT plan · b at its limit. The demo
// server answers `models.list` with it, and the gallery draws the model sheet from it.
// Pure.

import type { ModelInfo, ModelsListResult } from "@/lib/protocol/generated";

const WORK = "anthropic-api.work";
const PERSONAL = "anthropic-api.personal";
const PLAN = "openai-subscription";
const PLAN_B = "openai-subscription.b";

const CLAUDE: Partial<ModelInfo> = {
  efforts: ["low", "medium", "high"],
  default_effort: "high",
  context_window: 200_000,
};
const SOL: Partial<ModelInfo> = {
  efforts: ["minimal", "low", "medium", "high", "xhigh"],
  default_effort: "medium",
  context_window: 400_000,
};

function model(id: string, provider: string, extra: Partial<ModelInfo>): ModelInfo {
  return { id, provider, source: "builtin", ...extra };
}

/** The list of `models.list` at `now` (ms since the epoch): the resets lie after it. */
export function demoModels(now: number): ModelsListResult {
  const at = (minutes: number): string => new Date(now + minutes * 60_000).toISOString();
  return {
    models: [
      model("claude-opus-5-5", WORK, { ...CLAUDE, default: true }),
      model("claude-sonnet-5", WORK, CLAUDE),
      model("claude-haiku-4-5", WORK, { efforts: [], context_window: 200_000 }),
      model("claude-opus-5-5", PERSONAL, CLAUDE),
      model("gpt-6.1-sol", PLAN, SOL),
      model("gpt-6.1-sol", PLAN_B, SOL),
    ],
    catalogs: [
      { provider: WORK, origin: "backend", label: "work" },
      { provider: PERSONAL, origin: "cache", label: "personal", signed_out: true },
      {
        provider: PLAN,
        origin: "backend",
        limits: {
          observed_at: at(-12),
          windows: [
            { minutes: 300, used_percent: 92, resets_at: at(130) },
            { minutes: 10_080, used_percent: 41, resets_at: at(4_000) },
          ],
        },
      },
      {
        provider: PLAN_B,
        origin: "backend",
        label: "b",
        limits: {
          observed_at: at(-3),
          windows: [
            { minutes: 300, used_percent: 100, resets_at: at(12) },
            { minutes: 10_080, used_percent: 63, resets_at: at(2_500) },
          ],
        },
      },
    ],
  };
}
