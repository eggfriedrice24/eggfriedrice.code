import { describe, expect, it } from "vitest";

import type { AccountOut } from "@/lib/protocol/generated";

import { accountOutReason, accountOutView } from "./account-out";

/** 2026-11-14 at 14:02 local time. */
const NOW = new Date(2026, 10, 14, 14, 2).getTime();
const at = (hours: number, minutes: number): string =>
  new Date(2026, 10, 14, hours, minutes).toISOString();

const PLAN = "openai-subscription";
const PLAN_B = "openai-subscription.b";
const LIVE = { newest: true, now: NOW, choice: undefined };

/** AC-Out 1: the ChatGPT plan at its usage limit, b ready. */
const ONE: AccountOut = {
  provider: PLAN,
  model: "gpt-6.1-sol",
  reached: { kind: "usage", resets_at: at(14, 20) },
  others: [{ provider: PLAN_B, state: "ready" }],
};

describe("the end of a turn whose account is out", () => {
  it("says why with the reset, and offers the same model on the other account", () => {
    const view = accountOutView(ONE, "the ChatGPT plan reached its usage limit", LIVE);
    expect(view).toEqual({
      reason: "the ChatGPT plan reached its usage limit · resets 14:20",
      switches: [{ label: "switch to ChatGPT plan · b", model: "gpt-6.1-sol", provider: PLAN_B }],
      notes: [],
      retry: false,
    });
  });

  it("offers each account that serves the model, and names no time without a reset", () => {
    const out: AccountOut = {
      provider: "openai-api",
      model: "gpt-6.1",
      reached: { kind: "credit" },
      others: [
        { provider: PLAN, state: "ready" },
        { provider: PLAN_B, state: "full", resets_at: at(14, 10) },
      ],
    };
    const view = accountOutView(out, "the OpenAI API key has no credit left.", LIVE);
    expect(view.reason).toBe("the OpenAI API key has no credit left");
    expect(view.switches.map((action) => action.label)).toEqual([
      "switch to ChatGPT plan",
      "switch to ChatGPT plan · b",
    ]);
  });

  it("says when no other account runs the model, and keeps retry", () => {
    const out: AccountOut = {
      provider: "openai-api",
      model: "gpt-5.5",
      reached: { kind: "requests", resets_at: at(14, 3) },
    };
    const view = accountOutView(out, "the OpenAI API key reached its rate limit", LIVE);
    expect(view).toEqual({
      reason: "the OpenAI API key reached its rate limit · resets 14:03",
      switches: [],
      notes: ["no other account runs GPT-5.5"],
      retry: true,
    });
  });

  it("never offers a signed-out account: one quiet line with its login", () => {
    const out: AccountOut = {
      provider: "anthropic-api.work",
      model: "claude-opus-5-5",
      reached: { kind: "too_many_requests" },
      others: [{ provider: "anthropic-api.personal", state: "signed_out" }],
    };
    const view = accountOutView(out, 'the account work got "too many requests"', LIVE);
    expect(view.switches).toEqual([]);
    expect(view.notes).toEqual(["personal is signed out: efr login anthropic --account personal"]);
    expect(view.retry).toBe(true);
  });

  it("names a labelled account by its label", () => {
    const out: AccountOut = {
      ...ONE,
      others: [{ provider: PLAN_B, label: "Family", state: "ready" }],
    };
    expect(accountOutView(out, "x", LIVE).switches[0]?.label).toBe(
      "switch to ChatGPT plan · Family",
    );
  });

  it("offers retry after a switch to an offered account", () => {
    const view = accountOutView(ONE, "x", {
      ...LIVE,
      choice: { model: "gpt-6.1-sol", provider: PLAN_B, effort: "medium" },
    });
    expect(view.switches).toEqual([]);
    expect(view.retry).toBe(true);
  });

  it("keeps only the reason in the history, and offers retry after the reset", () => {
    expect(accountOutView(ONE, "x", { ...LIVE, newest: false })).toEqual({
      reason: "x · resets 14:20",
      switches: [],
      notes: [],
      retry: false,
    });
    const later = accountOutView(ONE, "x", {
      ...LIVE,
      now: new Date(2026, 10, 14, 14, 21).getTime(),
    });
    expect(later.switches).toEqual([]);
    expect(later.retry).toBe(true);
  });

  it("writes a reset on another day with its day", () => {
    const out: AccountOut = {
      ...ONE,
      reached: { kind: "usage", resets_at: new Date(2026, 10, 16, 9, 0).toISOString() },
    };
    expect(accountOutReason(out, "the plan is full", NOW)).toBe(
      "the plan is full · resets Mon 09:00",
    );
    expect(accountOutReason(out, undefined, NOW)).toBe(
      "the account cannot serve this model now · resets Mon 09:00",
    );
  });
});
