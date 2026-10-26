import { describe, expect, it } from "vitest";

import type { ConversationSummary } from "@/lib/protocol/generated";
import { conversationItem, newestFirst, sameConversation } from "./conversation-item";
import { item } from "./testing";

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
  it("keeps the machine, the title and the cwd and reads the update time", () => {
    expect(conversationItem("m1", SUMMARY)).toEqual({
      machine: "m1",
      id: SUMMARY.id,
      title: "why is the disk full",
      status: "awaiting_approval",
      updatedAt: Date.parse("2026-10-26T12:00:00Z"),
      cwd: "/var/log",
    });
  });

  it("leaves out a null or empty title and cwd and gives 0 for a time that does not parse", () => {
    const nulls = conversationItem("m1", {
      ...SUMMARY,
      title: null,
      cwd: null,
      updated_at: "later",
    });
    expect(nulls).toEqual({
      machine: "m1",
      id: SUMMARY.id,
      status: "awaiting_approval",
      updatedAt: 0,
    });
    expect(Object.keys(nulls)).not.toContain("title");
    const empty = conversationItem("m1", { ...SUMMARY, title: "", cwd: "" });
    expect(Object.keys(empty)).not.toContain("title");
    expect(Object.keys(empty)).not.toContain("cwd");
  });

  it("reads a status word of a newer efrd as idle", () => {
    const summary: ConversationSummary = JSON.parse(
      JSON.stringify({ ...SUMMARY, status: "compacting" }),
    );
    expect(conversationItem("m1", summary).status).toBe("idle");
  });
});

describe("sameConversation", () => {
  it("needs the same machine and the same id", () => {
    expect(sameConversation({ machine: "a", id: "1" }, { machine: "a", id: "1" })).toBe(true);
    expect(sameConversation({ machine: "a", id: "1" }, { machine: "b", id: "1" })).toBe(false);
    expect(sameConversation({ machine: "a", id: "1" }, { machine: "a", id: "2" })).toBe(false);
  });
});

describe("newestFirst", () => {
  it("sorts by time, then by machine and id, whatever the input order", () => {
    const rows = [item("b", "1", 5), item("a", "2", 5), item("a", "1", 5), item("a", "9", 9)];
    const sorted = [...rows].sort(newestFirst).map((row) => `${row.machine}:${row.id}`);
    expect(sorted).toEqual(["a:9", "a:1", "a:2", "b:1"]);
    const otherOrder = [rows[3], rows[2], rows[1], rows[0]].filter((row) => row !== undefined);
    expect(otherOrder.sort(newestFirst).map((row) => `${row.machine}:${row.id}`)).toEqual(sorted);
  });
});
