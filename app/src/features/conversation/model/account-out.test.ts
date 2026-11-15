import { describe, expect, it } from "vitest";

import type { AccountOut } from "@/lib/protocol/generated";
import { fixtureEvent } from "@/lib/protocol/testing";

import { accountOutReason, accountOutView } from "./account-out";

/** 2026-11-14 at 14:02 local time. */
const NOW = new Date(2026, 10, 14, 14, 2).getTime();
const at = (hours: number, minutes: number): string =>
  new Date(2026, 10, 14, hours, minutes).toISOString();

const PLAN = "openai-subscription";
const PLAN_B = "openai-subscription.b";
const LIVE = { newest: true, now: NOW, choice: undefined };

/** The reason of a turn on `provider` whose account reached a limit of kind `kind`. */
function reasonOf(kind: AccountOut["reached"]["kind"], provider = "anthropic-api"): string {
  return accountOutReason({ provider, model: "m", reached: { kind } }, NOW);
}

/** AC-Out 1: the ChatGPT plan at its usage limit, b ready. */
const ONE: AccountOut = {
  provider: PLAN,
  model: "gpt-6.1-sol",
  reached: { kind: "usage", resets_at: at(14, 20) },
  others: [{ provider: PLAN_B, state: "ready" }],
};

describe("the end of a turn whose account is out", () => {
  it("says why with the reset, and offers the same model on the other account", () => {
    const view = accountOutView(ONE, LIVE);
    expect(view).toEqual({
      reason: "the ChatGPT plan reached its usage limit · resets 14:20",
      switches: [{ label: "switch to ChatGPT plan · b", model: "gpt-6.1-sol", provider: PLAN_B }],
      notes: [],
      retry: false,
    });
  });

  it("names the account from account_out, not from the message of efrd", () => {
    // NOTE: the message as efrd sends it: the Display of ProviderError::LimitReached,
    // which names no account and carries the provider's own text.
    const message =
      "the account reached the usage limit of its plan: The usage limit has been reached";
    const envelope = fixtureEvent("turn_failed_account_out", 1, {
      error: { code: "busy", message },
    });
    const event = envelope.event;
    if (event.kind !== "turn_failed" || event.account_out == null) {
      throw new Error("the fixture is a turn_failed with account_out");
    }
    const now = Date.parse("2026-10-03T12:00:00Z");
    const reason = accountOutView(event.account_out, { ...LIVE, now }).reason;
    expect(reason).toMatch(/^the ChatGPT plan reached its usage limit · resets /u);
    expect(reason).not.toContain("The usage limit has been reached");
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
    const view = accountOutView(out, LIVE);
    expect(view.reason).toBe("the OpenAI API key has no credit left");
    // NOTE: D6, TM-Accounts 17: the full account b is no ready one; it says when it
    // runs again, and the tap still switches.
    expect(view.switches).toEqual([
      { label: "switch to ChatGPT plan", model: "gpt-6.1", provider: PLAN },
      {
        label: "b runs again 14:10",
        model: "gpt-6.1",
        provider: PLAN_B,
        accessibilityLabel: "Switch to ChatGPT plan · b, it runs again at 14:10",
      },
    ]);
  });

  it("names the day when a full account runs again on another day", () => {
    const out: AccountOut = {
      provider: PLAN_B,
      model: "gpt-6.1-sol",
      reached: { kind: "usage", resets_at: new Date(2026, 10, 17, 9, 0).toISOString() },
      others: [
        { provider: PLAN, state: "full", resets_at: new Date(2026, 10, 16, 9, 0).toISOString() },
      ],
    };
    expect(accountOutView(out, LIVE).switches.map((action) => action.label)).toEqual([
      "ChatGPT plan runs again Mon 09:00",
    ]);
  });

  it("says when no other account runs the model, and keeps retry", () => {
    const out: AccountOut = {
      provider: "openai-api",
      model: "gpt-5.5",
      reached: { kind: "requests", resets_at: at(14, 3) },
    };
    const view = accountOutView(out, LIVE);
    expect(view).toEqual({
      reason: "the OpenAI API key reached its rate limit · resets 14:03",
      switches: [],
      notes: ["no other account runs GPT-5.5"],
      retry: true,
    });
  });

  it("says that no other account has room before the soonest reset, not that none runs it", () => {
    // NOTE: D3: b runs the model too, but it is full until after the plan.
    const out: AccountOut = {
      provider: PLAN,
      model: "gpt-6.1-sol",
      reached: { kind: "usage", resets_at: at(14, 20) },
      others_full: { resets_at: at(16, 5) },
    };
    expect(accountOutView(out, LIVE)).toEqual({
      reason: "the ChatGPT plan reached its usage limit · resets 14:20",
      switches: [],
      notes: ["no other account has room for GPT-6.1 Sol before 16:05"],
      retry: true,
    });
    const nextDay = new Date(2026, 10, 16, 9, 0).toISOString();
    const withSignedOut: AccountOut = {
      ...out,
      others: [{ provider: "openai-subscription.c", state: "signed_out" }],
      others_full: { resets_at: nextDay },
    };
    expect(accountOutView(withSignedOut, LIVE).notes).toEqual([
      "no other account has room for GPT-6.1 Sol before Mon 09:00",
      "c is signed out: efr login openai --account c",
    ]);
    const noTime: AccountOut = { ...out, others_full: {} };
    expect(accountOutView(noTime, LIVE).notes).toEqual([
      "no other account has room for GPT-6.1 Sol",
    ]);
    const ready: AccountOut = { ...out, others: [{ provider: PLAN_B, state: "ready" }] };
    expect(accountOutView(ready, LIVE).notes).toEqual([]);
  });

  it("never offers a signed-out account: one quiet line with its login", () => {
    const out: AccountOut = {
      provider: "anthropic-api.work",
      model: "claude-opus-5-5",
      reached: { kind: "too_many_requests" },
      others: [{ provider: "anthropic-api.personal", state: "signed_out" }],
    };
    const view = accountOutView(out, LIVE);
    // NOTE: errata 5 and 12: no count of tries, and the account by its name alone.
    expect(view.reason).toBe('work got "too many requests", so efr stopped trying');
    expect(view.switches).toEqual([]);
    expect(view.notes).toEqual(["personal is signed out: efr login anthropic --account personal"]);
    expect(view.retry).toBe(true);
  });

  it("leaves out the reset of too many requests, and keeps the offer after it", () => {
    // NOTE: efrd sends resets_at = now + retry-after for this kind: a wait of some
    // seconds. Errata 5: the line names no reset, and the offer does not go old at it.
    const out: AccountOut = {
      provider: "anthropic-api.work",
      model: "claude-opus-5-5",
      reached: { kind: "too_many_requests", resets_at: new Date(NOW + 1000).toISOString() },
      others: [{ provider: "anthropic-api.personal", state: "signed_out" }],
    };
    const after = { ...LIVE, now: NOW + 5000 };
    expect(accountOutReason(out, NOW)).toBe('work got "too many requests", so efr stopped trying');
    expect(accountOutView(out, after)).toEqual({
      reason: 'work got "too many requests", so efr stopped trying',
      switches: [],
      notes: ["personal is signed out: efr login anthropic --account personal"],
      retry: true,
    });
    const ready: AccountOut = { ...out, others: [{ provider: "anthropic-api", state: "ready" }] };
    expect(accountOutView(ready, after).switches).toEqual([
      { label: "switch to Claude API", model: "claude-opus-5-5", provider: "anthropic-api" },
    ]);
  });

  it("names a labelled account by its label", () => {
    const out: AccountOut = {
      ...ONE,
      others: [{ provider: PLAN_B, label: "Family", state: "ready" }],
    };
    expect(accountOutView(out, LIVE).switches[0]?.label).toBe("switch to ChatGPT plan · Family");
    const labelled: AccountOut = { ...ONE, provider: PLAN_B, label: "Family" };
    expect(accountOutReason(labelled, NOW)).toBe("Family reached its usage limit · resets 14:20");
  });

  it("offers retry after a switch to an offered account", () => {
    const view = accountOutView(ONE, {
      ...LIVE,
      choice: { model: "gpt-6.1-sol", provider: PLAN_B, effort: "medium" },
    });
    expect(view.switches).toEqual([]);
    expect(view.retry).toBe(true);
  });

  it("keeps only the reason in the history, and offers retry after the reset", () => {
    expect(accountOutView(ONE, { ...LIVE, newest: false })).toEqual({
      reason: "the ChatGPT plan reached its usage limit · resets 14:20",
      switches: [],
      notes: [],
      retry: false,
    });
    const later = accountOutView(ONE, {
      ...LIVE,
      now: new Date(2026, 10, 14, 14, 21).getTime(),
    });
    expect(later.switches).toEqual([]);
    expect(later.retry).toBe(true);
  });

  it("writes a reset on another day with its day", () => {
    const out: AccountOut = {
      ...ONE,
      provider: PLAN_B,
      reached: { kind: "usage", resets_at: new Date(2026, 10, 16, 9, 0).toISOString() },
    };
    expect(accountOutReason(out, NOW)).toBe("b reached its usage limit · resets Mon 09:00");
  });

  it("names each other limit in the words of the terminal", () => {
    expect(reasonOf("credit")).toBe("the Claude API key has no credit left");
    expect(reasonOf("spend_limit")).toBe("the Claude API key reached its spend limit");
    expect(reasonOf("tokens", "openai-api.ci")).toBe("ci reached its rate limit");
    expect(reasonOf("unknown", "other-kind")).toBe("the account other-kind reached a limit");
  });
});
