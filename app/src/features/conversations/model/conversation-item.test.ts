import { describe, expect, it } from "vitest";

import type { ConversationSummary } from "@/lib/protocol/generated";
import { conversationItem } from "./conversation-item";

const SUMMARY: ConversationSummary = {
  id: "019a9b1c-3d00-7a10-8b20-000000000001",
  title: "why is the disk full",
  status: "awaiting_approval",
  created_at: "2026-10-26T10:00:00Z",
  updated_at: "2026-10-26T12:00:00Z",
  last_seq: 3,
  cwd: "/var/log",
};

describe("conversationItem", () => {
  it("keeps the title and the cwd and reads the update time", () => {
    expect(conversationItem(SUMMARY)).toEqual({
      id: SUMMARY.id,
      title: "why is the disk full",
      status: "awaiting_approval",
      updatedAt: Date.parse("2026-10-26T12:00:00Z"),
      cwd: "/var/log",
    });
  });

  it("leaves out a null title and gives 0 for a time that does not parse", () => {
    const item = conversationItem({ ...SUMMARY, title: null, cwd: null, updated_at: "later" });
    expect(item).toEqual({ id: SUMMARY.id, status: "awaiting_approval", updatedAt: 0 });
  });
});
