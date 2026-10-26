// Helpers for the tests of the outbox: ids that are valid UUIDs, a wall clock that a test
// sets, and a fake efrd that takes `prompt.send` as efrd does (idempotent by command id).
// Only test files import this module.

import type { ConversationId, PromptSend, PromptSendResult } from "@/lib/protocol/generated";
import { createOutbox, type Outbox, type OutboxStorage } from "./outbox";

export const DESKTOP = "019a9b1c-3d00-7a10-8b20-000000000001";
export const LAPTOP = "019a9b1c-3d00-7a10-8b20-000000000002";
export const CONVERSATION_A = "019a9b1c-3d00-7a10-8b20-0000000000a1";
export const CONVERSATION_B = "019a9b1c-3d00-7a10-8b20-0000000000b2";

/** A function that gives a new v4-shaped UUID on each call: ...-000000000001 and up. */
export function idMaker(prefix = "00000000"): () => string {
  let next = 0;
  return () => {
    next += 1;
    return `${prefix}-0000-4000-8000-${String(next).padStart(12, "0")}`;
  };
}

/** A wall clock that moves only when a test sets `time`. */
export class WallClock {
  time = 1_700_000_000_000;
  readonly now = (): number => this.time;
}

/**
 * An outbox with test ids and a test wall clock. Two outboxes on one storage, as before
 * and after a restart, need two `prefix` values, so their ids differ.
 */
export function testOutbox(
  storage?: OutboxStorage,
  prefix = "00000000",
): {
  readonly outbox: Outbox;
  readonly wall: WallClock;
  readonly errors: unknown[];
} {
  const wall = new WallClock();
  const errors: unknown[] = [];
  const newId = idMaker(prefix);
  const onStorageError = (error: unknown): void => {
    errors.push(error);
  };
  if (storage === undefined) {
    return { outbox: createOutbox({ now: wall.now, newId, onStorageError }), wall, errors };
  }
  const outbox = createOutbox({ now: wall.now, newId, onStorageError, storage });
  return { outbox, wall, errors };
}

/** One prompt that the fake efrd took. */
export interface TakenPrompt {
  readonly commandId: string;
  readonly conversationId: ConversationId;
  readonly text: string;
}

/**
 * `prompt.send` as efrd answers it: a retry with the same command id gets the first
 * result again, `new_conversation` makes a conversation, and a conversation id must be
 * known.
 */
export class FakeEfrd {
  /** Every prompt that efrd took, in the order it took them. */
  readonly taken: TakenPrompt[] = [];
  /** Every call, also the retries. */
  readonly calls: PromptSend[] = [];
  private readonly receipts = new Map<string, PromptSendResult>();
  private readonly conversations = new Set<ConversationId>();
  private readonly newId = idMaker("0c0c0c0c");
  private seq = 0;

  constructor(...known: ConversationId[]) {
    for (const id of known) {
      this.conversations.add(id);
    }
  }

  /** True when efrd took a send with this command id. */
  has(commandId: string): boolean {
    return this.receipts.has(commandId);
  }

  /** Takes a send, or returns the receipt of its command id. */
  take(params: PromptSend): PromptSendResult {
    this.calls.push(params);
    const receipt = this.receipts.get(params.command_id);
    if (receipt !== undefined) {
      return receipt;
    }
    let conversationId: ConversationId;
    if (params.new_conversation === true) {
      conversationId = this.newId();
      this.conversations.add(conversationId);
    } else if (
      typeof params.conversation_id === "string" &&
      this.conversations.has(params.conversation_id)
    ) {
      conversationId = params.conversation_id;
    } else {
      throw new Error("the fake efrd does not know the conversation");
    }
    this.seq += 1;
    const result: PromptSendResult = {
      conversation_id: conversationId,
      queued: false,
      seq: this.seq,
      turn_id: this.newId(),
    };
    this.receipts.set(params.command_id, result);
    this.taken.push({ commandId: params.command_id, conversationId, text: params.text });
    return result;
  }
}
