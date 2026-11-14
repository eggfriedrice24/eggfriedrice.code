import { describe, expect, it } from "vitest";

import type { AccountLimits, ModelInfo } from "@/lib/protocol/generated";

import {
  accountId,
  accountKind,
  accountName,
  accountShort,
  ageText,
  kindName,
  limitLevel,
  limitsNow,
  loginCommand,
  providerLogo,
  reachedWords,
  resetText,
  runsOnMany,
  windowName,
} from "./accounts";

/** 2026-11-14 at 14:08 local time. */
const NOW = new Date(2026, 10, 14, 14, 8).getTime();
const at = (hours: number, minutes: number, day = 14): string =>
  new Date(2026, 10, day, hours, minutes).toISOString();

describe("the names of the accounts", () => {
  it("names the kind, then the label or the id; the default account is the kind alone", () => {
    expect(accountName("anthropic-api.work")).toBe("Claude API · work");
    expect(accountName("openai-api")).toBe("OpenAI API");
    expect(accountName("openai-subscription")).toBe("ChatGPT plan");
    expect(accountName("openai-subscription.b")).toBe("ChatGPT plan · b");
    expect(accountName("openai-subscription.b", "Family")).toBe("ChatGPT plan · Family");
    expect(accountName("openai-subscription", "Mine")).toBe("ChatGPT plan · Mine");
    expect(accountName("openai-subscription", "  ")).toBe("ChatGPT plan");
    expect(accountName("ollama")).toBe("ollama");
  });

  it("gives the short name, the parts and the login command", () => {
    expect(accountShort("openai-subscription.b")).toBe("b");
    expect(accountShort("openai-subscription")).toBe("ChatGPT plan");
    expect(accountShort("anthropic-api.work", "Work")).toBe("Work");
    expect(accountKind("anthropic-api.work")).toBe("anthropic-api");
    expect(accountId("anthropic-api.work")).toBe("work");
    expect(accountId("anthropic-api")).toBeUndefined();
    expect(kindName("openai-api")).toBe("OpenAI API");
    expect(loginCommand("anthropic-api.personal")).toBe("efr login anthropic --account personal");
    expect(loginCommand("openai-subscription")).toBe("efr login openai");
    expect(loginCommand("openai-api.x")).toBe("efr login openai-api --account x");
  });

  it("picks the logo by the kind", () => {
    expect(providerLogo("anthropic-api.work")).toBe("claude");
    expect(providerLogo("openai-subscription")).toBe("openai");
    expect(providerLogo(undefined)).toBe("none");
  });

  it("tells when a model runs on two accounts", () => {
    const models: ModelInfo[] = [
      { id: "gpt-6.1-sol", provider: "openai-subscription", source: "builtin" },
      { id: "gpt-6.1-sol", provider: "openai-subscription.b", source: "builtin" },
      { id: "gpt-5.5", provider: "openai-api", source: "builtin" },
    ];
    expect(runsOnMany(models, "gpt-6.1-sol")).toBe(true);
    expect(runsOnMany(models, "gpt-5.5")).toBe(false);
    expect(runsOnMany(models, "nothing")).toBe(false);
  });
});

/** A ChatGPT plan whose 5-hour window is at `used` percent. */
function plan(used: number): AccountLimits {
  return {
    observed_at: at(13, 56),
    windows: [
      { minutes: 300, used_percent: used, resets_at: at(14, 20) },
      { minutes: 10_080, used_percent: 41, resets_at: at(9, 0, 16) },
    ],
  };
}

describe("the limits of an account", () => {
  it("names the windows", () => {
    expect(windowName(300)).toBe("5-hour");
    expect(windowName(1440)).toBe("daily");
    expect(windowName(10_080)).toBe("weekly");
    expect(windowName(43_200)).toBe("monthly");
    expect(windowName(2880)).toBe("2-day");
    expect(windowName(90)).toBe("90-minute");
    expect(windowName(undefined)).toBe("window");
  });

  it("is silent below 80 %, warns from 80 %, and is reached at 100 %", () => {
    expect(limitLevel(plan(79), NOW)).toEqual({ kind: "none" });
    expect(limitLevel(plan(92), NOW)).toEqual({ kind: "warn", window: "5-hour", percent: 92 });
    expect(limitLevel(plan(92), NOW, 93)).toEqual({ kind: "none" });
    expect(limitLevel(plan(100), NOW)).toEqual({
      kind: "reached",
      resetsAt: Date.parse(at(14, 20)),
    });
    expect(limitLevel(undefined, NOW)).toEqual({ kind: "none" });
  });

  it("is reached when a limit stops an API key", () => {
    const key: AccountLimits = {
      observed_at: at(14, 7),
      reached: { kind: "requests", resets_at: at(14, 9) },
    };
    expect(limitLevel(key, NOW)).toEqual({ kind: "reached", resetsAt: Date.parse(at(14, 9)) });
    expect(limitLevel({ ...key, reached: { kind: "credit" } }, NOW)).toEqual({
      kind: "reached",
      resetsAt: undefined,
    });
  });

  it("forgets a value after its reset time", () => {
    const later = new Date(2026, 10, 14, 14, 21).getTime();
    expect(limitLevel(plan(100), later)).toEqual({ kind: "none" });
    expect(limitsNow(plan(100), later)?.windows).toHaveLength(1);
    expect(
      limitsNow(
        { observed_at: at(14, 0), reached: { kind: "requests", resets_at: at(14, 1) } },
        NOW,
      ),
    ).toBeUndefined();
  });

  it("writes the reset and the age", () => {
    expect(resetText(Date.parse(at(14, 20)), NOW)).toBe("resets 14:20");
    expect(resetText(Date.parse(at(9, 0, 16)), NOW)).toBe("resets Mon 09:00");
    expect(ageText(NOW - 30_000, NOW)).toBe("just now");
    expect(ageText(NOW - 12 * 60_000, NOW)).toBe("12m ago");
    expect(ageText(NOW - 3 * 3_600_000, NOW)).toBe("3h ago");
    expect(ageText(NOW - 50 * 3_600_000, NOW)).toBe("2d ago");
    const reached = { kind: "reached", resetsAt: Date.parse(at(14, 20)) } as const;
    expect(reachedWords(reached, Date.parse(at(13, 56)), NOW)).toBe(
      "limit reached · resets 14:20 · 12m ago",
    );
    expect(reachedWords(reached, NOW - 60_000, NOW)).toBe("limit reached · resets 14:20");
    expect(reachedWords({ kind: "reached", resetsAt: undefined }, undefined, NOW)).toBe(
      "limit reached",
    );
  });
});
