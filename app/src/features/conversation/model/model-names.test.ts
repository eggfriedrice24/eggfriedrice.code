import { describe, expect, it } from "vitest";

import { modelName, windowLabel } from "./model-names";

describe("the names of the models", () => {
  it("names the models from their ids", () => {
    expect(modelName("claude-opus-5-5")).toBe("Opus 5.5");
    expect(modelName("claude-sonnet-5")).toBe("Sonnet 5");
    expect(modelName("claude-haiku-4-5-20251001")).toBe("Haiku 4.5");
    expect(modelName("gpt-5.5")).toBe("GPT-5.5");
    expect(modelName("gpt-6.1-sol")).toBe("GPT-6.1 Sol");
    expect(modelName("gpt-5.5-codex")).toBe("GPT-5.5 Codex");
    expect(modelName("llama-local")).toBe("llama-local");
  });

  it("writes a context window in short words", () => {
    expect(windowLabel(200_000)).toBe("200k");
    expect(windowLabel(400_000)).toBe("400k");
    expect(windowLabel(1_000_000)).toBe("1M");
    expect(windowLabel(1_500_000)).toBe("1.5M");
    expect(windowLabel(128_400)).toBe("128k");
  });
});
