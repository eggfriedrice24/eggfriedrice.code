import { describe, expect, it } from "vitest";

import type { CatalogStatus } from "@/lib/protocol/generated";

import { accountRows, accountScreen } from "./account-limits";

/** 2026-11-14 at 14:08 local time. */
const NOW = new Date(2026, 10, 14, 14, 8).getTime();
const at = (hours: number, minutes: number, day = 14): string =>
  new Date(2026, 10, day, hours, minutes).toISOString();

/** The accounts of AC-Limits 4. */
const CATALOGS: CatalogStatus[] = [
  { provider: "anthropic-api.work", origin: "backend" },
  { provider: "anthropic-api.personal", origin: "cache", signed_out: true },
  {
    provider: "openai-subscription",
    origin: "backend",
    limits: {
      observed_at: at(14, 6),
      windows: [
        { minutes: 300, used_percent: 92, resets_at: at(16, 0) },
        { minutes: 10_080, used_percent: 41, resets_at: at(9, 0, 16) },
      ],
    },
  },
  {
    provider: "openai-subscription.b",
    origin: "backend",
    limits: {
      observed_at: at(13, 56),
      windows: [
        { minutes: 300, used_percent: 100, resets_at: at(14, 20) },
        { minutes: 10_080, used_percent: 63, resets_at: at(9, 0, 16) },
      ],
    },
  },
  {
    provider: "openai-api",
    origin: "backend",
    limits: { observed_at: at(14, 7), reached: { kind: "requests", resets_at: at(14, 9) } },
  },
];

describe("the accounts in Settings", () => {
  it("lists each account by its sheet name with its fullest limit", () => {
    expect(accountRows(CATALOGS, "desktop", NOW)).toEqual([
      {
        provider: "anthropic-api.work",
        name: "Claude API · work",
        note: undefined,
        value: { kind: "none" },
        label: "Claude API · work",
      },
      {
        provider: "anthropic-api.personal",
        name: "Claude API · personal",
        note: "signed out on desktop",
        value: { kind: "none" },
        label: "Claude API · personal, signed out on desktop",
      },
      {
        provider: "openai-subscription",
        name: "ChatGPT plan",
        note: undefined,
        value: { kind: "warn", text: "92%" },
        label: "ChatGPT plan, 92% used",
      },
      {
        provider: "openai-subscription.b",
        name: "ChatGPT plan · b",
        note: undefined,
        value: { kind: "reached", text: "limit reached" },
        label: "ChatGPT plan · b, limit reached",
      },
      {
        provider: "openai-api",
        name: "OpenAI API",
        note: undefined,
        value: { kind: "reached", text: "limit reached" },
        label: "OpenAI API, limit reached",
      },
    ]);
  });

  it("shows a row per window of a plan, with the reset and the age", () => {
    expect(accountScreen(CATALOGS, "openai-subscription.b", "desktop", NOW)).toEqual({
      title: "ChatGPT plan · b",
      header: "On desktop",
      rows: [
        { key: "w0", name: "5-hour", value: "limit reached · resets 14:20", reached: true },
        { key: "w1", name: "weekly", value: "63% · resets Mon 09:00", reached: false },
      ],
      footer:
        'From the last call on this account, 12m ago. "limit reached" clears after its reset time.',
    });
  });

  it("shows the rate limit of an API key only when it was hit", () => {
    expect(accountScreen(CATALOGS, "openai-api", "desktop", NOW)?.rows).toEqual([
      { key: "r:requests", name: "requests", value: "limit reached · resets 14:09", reached: true },
    ]);
    const quiet = accountScreen(CATALOGS, "anthropic-api.work", "desktop", NOW);
    expect(quiet?.rows).toEqual([]);
    expect(quiet?.footer).toBe("An API key shows a rate limit only when it was hit.");
    expect(accountScreen(CATALOGS, "anthropic-api.personal", "desktop", NOW)?.footer).toBe(
      "Signed out on desktop. Log in on the machine to use it again.",
    );
    expect(accountScreen(CATALOGS, "nothing", "desktop", NOW)).toBeUndefined();
  });
});
