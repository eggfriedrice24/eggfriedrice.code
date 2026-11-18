import { describe, expect, it } from "vitest";

import { UPLOAD_LOST } from "./attachments-format";
import {
  CONVERSATION_A,
  DESKTOP,
  LAPTOP,
  outboxFile,
  outboxPrompt,
  testOutbox,
} from "./outbox-testing";
import { followAlias, outboxPrompts, routeConversation, sameOutboxPrompts } from "./screen-outbox";

const TURN = "019a9b1c-3d00-7a10-8b20-0000000000c1";

/** An outbox with one prompt for a new conversation of desktop. */
function newConversation() {
  const { outbox } = testOutbox();
  const target = outbox.newConversation();
  const localId = target.kind === "new" ? target.localId : "";
  const entry = outbox.enqueue({ machine: DESKTOP, target, text: "Why is the fan loud?" });
  return { outbox, localId, entry };
}

/** efrd takes the entry `commandId` into CONVERSATION_A. */
function take(outbox: ReturnType<typeof testOutbox>["outbox"], commandId: string): void {
  outbox.begin(commandId);
  outbox.succeed(commandId, {
    conversation_id: CONVERSATION_A,
    queued: false,
    seq: 1,
    turn_id: TURN,
  });
}

describe("outboxPrompts", () => {
  it("lists the prompts of a conversation that efrd did not echo, failed ones marked", () => {
    const { outbox } = testOutbox();
    const target = { kind: "conversation", conversationId: CONVERSATION_A } as const;
    const first = outbox.enqueue({ machine: DESKTOP, target, text: "one" });
    outbox.enqueue({ machine: DESKTOP, target, text: "two" });
    outbox.enqueue({ machine: LAPTOP, target, text: "other machine" });
    outbox.begin(first.commandId);
    outbox.fail(first.commandId, { code: "invalid", message: "no" });
    const prompts = outboxPrompts(outbox.store.get(), DESKTOP, CONVERSATION_A);
    expect(prompts.map((prompt) => [prompt.text, prompt.failed])).toEqual([
      ["one", true],
      ["two", false],
    ]);
    expect(prompts[0]?.key).toBe(first.commandId);
  });

  it("finds the prompts of a new conversation by its local id and by its alias", () => {
    const { outbox, localId, entry } = newConversation();
    expect(outboxPrompts(outbox.store.get(), DESKTOP, localId)).toHaveLength(1);
    take(outbox, entry.commandId);
    expect(outboxPrompts(outbox.store.get(), DESKTOP, localId)).toHaveLength(1);
    expect(outboxPrompts(outbox.store.get(), DESKTOP, CONVERSATION_A)).toHaveLength(1);
  });

  it("compares two lists by what they show", () => {
    const a = [outboxPrompt({})];
    expect(sameOutboxPrompts(a, [outboxPrompt({})])).toBe(true);
    expect(sameOutboxPrompts(a, [outboxPrompt({ failed: true })])).toBe(false);
    expect(sameOutboxPrompts(a, [outboxPrompt({ withdrawable: true })])).toBe(false);
    expect(sameOutboxPrompts(a, [])).toBe(false);
  });

  it("shows the upload of each file, and the withdraw until the send goes", () => {
    const { outbox } = testOutbox();
    const file = outboxFile("0f0f0f0f-0000-4000-8000-00000000000a", 10);
    const entry = outbox.enqueue({
      machine: DESKTOP,
      target: { kind: "conversation", conversationId: CONVERSATION_A },
      text: "look",
      attachments: [file],
    });
    const shown = () => outboxPrompts(outbox.store.get(), DESKTOP, CONVERSATION_A)[0];
    expect(shown()).toMatchObject({
      withdrawable: true,
      tiles: [{ state: { kind: "uploading" } }],
    });
    outbox.begin(entry.commandId);
    outbox.progress(entry.commandId, file.uploadId, 10, true);
    expect(shown()).toMatchObject({ withdrawable: false, tiles: [{ state: { kind: "size" } }] });
    outbox.fail(entry.commandId, { code: "upload", message: UPLOAD_LOST });
    expect(shown()).toMatchObject({
      failed: true,
      reason: UPLOAD_LOST,
      withdrawable: true,
      tiles: [{ state: { kind: "not_sent" } }],
    });
  });
});

describe("routeConversation", () => {
  it("names a new conversation until efrd made it, then the conversation", () => {
    const { outbox, localId, entry } = newConversation();
    expect(routeConversation(outbox.store.get(), DESKTOP, localId)).toEqual({
      kind: "new",
      localId,
    });
    expect(routeConversation(outbox.store.get(), LAPTOP, localId)).toEqual({
      kind: "conversation",
      id: localId,
    });
    take(outbox, entry.commandId);
    expect(routeConversation(outbox.store.get(), DESKTOP, localId)).toEqual({
      kind: "conversation",
      id: CONVERSATION_A,
    });
  });

  it("names a conversation of efrd as itself", () => {
    const { outbox } = testOutbox();
    expect(routeConversation(outbox.store.get(), DESKTOP, CONVERSATION_A)).toEqual({
      kind: "conversation",
      id: CONVERSATION_A,
    });
  });
});

describe("followAlias", () => {
  it("tells the conversation once, when efrd made it", () => {
    const { outbox, localId, entry } = newConversation();
    const told: string[] = [];
    const stop = followAlias(outbox.store, DESKTOP, localId, (id) => told.push(id));
    expect(told).toEqual([]);
    take(outbox, entry.commandId);
    outbox.enqueue({ machine: DESKTOP, target: { kind: "new", localId }, text: "more" });
    expect(told).toEqual([CONVERSATION_A]);
    stop();
  });

  it("tells it at once when the outbox knows it, and nothing after a stop", () => {
    const { outbox, localId, entry } = newConversation();
    take(outbox, entry.commandId);
    const told: string[] = [];
    followAlias(outbox.store, DESKTOP, localId, (id) => told.push(id))();
    expect(told).toEqual([CONVERSATION_A]);

    const second = newConversation();
    const stop = followAlias(second.outbox.store, DESKTOP, second.localId, (id) => told.push(id));
    stop();
    take(second.outbox, second.entry.commandId);
    expect(told).toEqual([CONVERSATION_A]);
  });
});
