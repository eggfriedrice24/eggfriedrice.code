import { describe, expect, it } from "vitest";

import { DEFAULT_PACER_CONFIG } from "@/lib/stream/pacer";
import { pacedArrive, pacedFrame, resumePaced, startPaced } from "./paced_text";

const LAG = DEFAULT_PACER_CONFIG.maxLagMs;

describe("paced text", () => {
  it("paces new text of a live message by word", () => {
    let state = startPaced({ text: "", final: false, generation: 0 }, 0);
    const text = "one two three four";
    state = pacedArrive(state, { text, final: false, generation: 0 }, 0);
    const half = pacedFrame(state, text, LAG / 2, false);
    expect(half.shown).toBe("one two ".length);
    expect(half.settled).toBe(false);
    const all = pacedFrame(half.state, text, LAG, false);
    expect(all.shown).toBe(text.length);
    expect(all.settled).toBe(true);
  });

  it("shows a large text at once when the row mounts", () => {
    const text = "x ".repeat(DEFAULT_PACER_CONFIG.catchUpUnits);
    const state = startPaced({ text, final: false, generation: 0 }, 0);
    expect(pacedFrame(state, text, 0, false).shown).toBe(text.length);
  });

  it("shows the whole text at the end of the message", () => {
    let state = startPaced({ text: "", final: false, generation: 0 }, 0);
    state = pacedArrive(state, { text: "a b c d", final: false, generation: 0 }, 0);
    state = pacedArrive(state, { text: "a b c d e", final: true, generation: 0 }, 10);
    expect(pacedFrame(state, "a b c d e", 10, false).shown).toBe(9);
  });

  it("shows the whole text of a new generation at once", () => {
    let state = startPaced({ text: "", final: false, generation: 0 }, 0);
    state = pacedArrive(state, { text: "draft text", final: false, generation: 0 }, 0);
    state = pacedArrive(state, { text: "other", final: true, generation: 1 }, 5);
    const frame = pacedFrame(state, "other", 5, false);
    expect([frame.state.generation, frame.shown, frame.settled]).toEqual([1, 5, true]);
  });

  it("shows everything on every frame with Reduce Motion on", () => {
    let state = startPaced({ text: "", final: false, generation: 0 }, 0);
    state = pacedArrive(state, { text: "a b c", final: false, generation: 0 }, 0);
    expect(pacedFrame(state, "a b c", 0, true).shown).toBe(5);
  });

  it("changes nothing for a message that did not grow", () => {
    const state = startPaced({ text: "abc", final: false, generation: 0 }, 0);
    expect(pacedArrive(state, { text: "abc", final: false, generation: 0 }, 50)).toBe(state);
    const final = pacedArrive(state, { text: "abc", final: true, generation: 0 }, 60);
    expect(pacedArrive(final, { text: "abc", final: true, generation: 0 }, 70)).toBe(final);
  });

  it("keeps what a row shows when its loop starts again", () => {
    const text = "one two three four";
    const state = resumePaced({ text, final: false, generation: 0 }, 1_000, 8);
    expect(pacedFrame(state, text, 1_000, false).shown).toBe(8);
    expect(pacedFrame(state, text, 1_000 + LAG, false).shown).toBe(text.length);
  });
});
