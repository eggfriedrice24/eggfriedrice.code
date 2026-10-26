import { describe, expect, it } from "vitest";

import { DEFAULT_LIVENESS, liveness, nextLivenessCheckAt, replaceOnForeground } from "./liveness";

const PROBE = DEFAULT_LIVENESS.probeAfterMs;
const DEAD = DEFAULT_LIVENESS.deadAfterMs;

describe("liveness", () => {
  it("probes once after the probe time and is dead after the dead time", () => {
    const fresh = { lastFrameAt: 1_000, probeSentAt: undefined };
    expect(liveness(fresh, 1_000 + PROBE - 1)).toBe("alive");
    expect(liveness(fresh, 1_000 + PROBE)).toBe("probe");
    const probed = { lastFrameAt: 1_000, probeSentAt: 1_000 + PROBE };
    expect(liveness(probed, 1_000 + PROBE + 1)).toBe("alive");
    expect(liveness(probed, 1_000 + DEAD)).toBe("dead");
  });

  it("counts a probe only for the silence after the last frame", () => {
    const answered = { lastFrameAt: 50_000, probeSentAt: 40_000 };
    expect(liveness(answered, 50_000 + PROBE)).toBe("probe");
  });

  it("names the next time the verdict can change", () => {
    expect(nextLivenessCheckAt({ lastFrameAt: 5, probeSentAt: undefined })).toBe(5 + PROBE);
    expect(nextLivenessCheckAt({ lastFrameAt: 5, probeSentAt: 6 })).toBe(5 + DEAD);
  });

  it("replaces the socket after 10 s or more in the background", () => {
    expect(replaceOnForeground(undefined, 100_000)).toBe(false);
    expect(replaceOnForeground(0, 9_999)).toBe(false);
    expect(replaceOnForeground(0, 10_000)).toBe(true);
  });
});
