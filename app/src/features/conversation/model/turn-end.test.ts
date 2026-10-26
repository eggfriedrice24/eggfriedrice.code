import { describe, expect, it } from "vitest";

import type { TurnEnd } from "@/lib/stream/reducer";
import { errorText, turnEndView } from "./turn-end";

const START = new Date(2026, 9, 26, 14, 29, 24).getTime();

function end(overrides: Partial<TurnEnd>): TurnEnd {
  return {
    outcome: "completed",
    at: START + 188_000,
    context: undefined,
    error: undefined,
    stop: undefined,
    usage: undefined,
    ...overrides,
  };
}

describe("turnEndView", () => {
  it("writes the time, the duration and the tokens of a done turn", () => {
    const view = turnEndView(
      end({ usage: { input_tokens: 15_000, output_tokens: 3_400 } }),
      START,
      undefined,
      "desktop",
    );
    expect(view).toMatchObject({
      glyph: undefined,
      lead: undefined,
      detail: "14:32 · 3m 08s · 18k tokens",
      canRetry: false,
    });
  });

  it("says why the model stopped early", () => {
    const view = turnEndView(end({ stop: "max_tokens" }), START, undefined, "desktop");
    expect(view.detail).toBe("14:32 · 3m 08s · the model hit its token limit");
  });

  it("says who interrupted the turn", () => {
    const at = START + 62_000;
    expect(
      turnEndView(end({ outcome: "interrupted", at }), START, "phone", "desktop"),
    ).toMatchObject({ glyph: "■", detail: "interrupted by you · 14:30 · 1m 02s" });
    expect(turnEndView(end({ outcome: "interrupted", at }), START, "shell", "desktop").detail).toBe(
      "interrupted in a terminal on desktop · 14:30 · 1m 02s",
    );
    expect(turnEndView(end({ outcome: "interrupted", at }), START, undefined, "x").detail).toBe(
      "interrupted · 14:30 · 1m 02s",
    );
  });

  it("gives a failed turn its reason, retry and copy error", () => {
    const view = turnEndView(
      end({ outcome: "failed", at: START + 22_000, error: "model error: overloaded" }),
      START,
      undefined,
      "desktop",
    );
    expect(view).toEqual({
      glyph: "✗",
      lead: "turn failed",
      detail: "· 14:29 · 22s",
      reason: "model error: overloaded",
      canRetry: true,
      label: "Turn failed, 14:29, 22s, model error: overloaded",
    });
    expect(errorText(view)).toBe("model error: overloaded");
  });

  it("reads a cancelled turn as a failure that efrd caused", () => {
    const view = turnEndView(end({ outcome: "cancelled" }), START, undefined, "desktop");
    expect(view).toMatchObject({
      lead: "turn cancelled",
      reason: "efrd stopped before the turn ran to its end",
      canRetry: true,
    });
  });

  it("copies the line when a failure has no reason", () => {
    const view = turnEndView(end({ outcome: "failed", at: undefined }), undefined, undefined, "d");
    expect(view.detail).toBe("");
    expect(errorText(view)).toBe("turn failed");
  });

  it("says a withdrawn prompt did not run", () => {
    expect(turnEndView(end({ outcome: "withdrawn" }), START, undefined, "d").detail).toBe(
      "withdrawn · the prompt did not run",
    );
  });
});
