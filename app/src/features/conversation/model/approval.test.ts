import { describe, expect, it } from "vitest";

import type { ApprovalView } from "@/lib/stream/reducer";
import { answerRefusal, localReason } from "./approval";

const BASE: ApprovalView = {
  callId: "c",
  turnId: "t",
  summary: "run `rm -rf build`",
  diffPreview: undefined,
  reason: undefined,
  modelReason: undefined,
  exitKinds: [],
  always: undefined,
  localOnly: false,
  interactive: false,
  askedAt: 0,
  expiresAt: undefined,
  status: "pending",
  decision: undefined,
  answeredBy: undefined,
  closedAt: undefined,
};

describe("the answers of the phone", () => {
  it("refuses an answer that efrd would refuse", () => {
    expect(answerRefusal(undefined, "allow")).toBe("unknown");
    expect(answerRefusal({ ...BASE, status: "expired" }, "allow")).toBe("not_pending");
    expect(answerRefusal({ ...BASE, localOnly: true }, "deny")).toBe("local_only");
    expect(answerRefusal(BASE, "allow_always")).toBe("decision");
    expect(answerRefusal(BASE, "allow")).toBeUndefined();
  });

  it("names why a question is local only", () => {
    expect(localReason(BASE)).toBeUndefined();
    expect(localReason({ ...BASE, localOnly: true, interactive: true })).toBe("privilege");
    expect(localReason({ ...BASE, localOnly: true, expiresAt: 5 })).toBe("delete");
    expect(localReason({ ...BASE, localOnly: true })).toBe("job");
  });
});
