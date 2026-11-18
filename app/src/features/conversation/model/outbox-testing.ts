// Helpers for the tests of the outbox: ids that are valid UUIDs, a wall clock that a test
// sets, and a fake efrd that takes `prompt.send` and `attachment.put` as efrd does
// (idempotent by command id, one result rule of an upload). Only test files import this
// module.

import { ServerError } from "@/lib/net/errors";
import { base64Decode, base64Encode } from "@/lib/protocol/bytes";
import type {
  AttachmentPut,
  AttachmentPutResult,
  ConversationId,
  PromptSend,
  PromptSendResult,
} from "@/lib/protocol/generated";
import { createOutbox, type Outbox, type OutboxAttachment, type OutboxStorage } from "./outbox";
import type { OutboxUploads } from "./outbox-drain";

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

  /** The uploads of `attachment.put`; a prompt takes its finished uploads. */
  readonly uploads = new FakeUploads();

  /** Takes a send, or returns the receipt of its command id. */
  take(params: PromptSend): PromptSendResult {
    this.calls.push(params);
    const receipt = this.receipts.get(params.command_id);
    if (receipt !== undefined) {
      return receipt;
    }
    const files = params.attachments ?? [];
    const missing = files.filter((id) => !this.uploads.finished(id));
    if (missing.length > 0) {
      // No receipt: the client uploads the files again and sends the same command id.
      throw new ServerError({
        code: "not_found",
        message: "an upload is missing",
        data: { missing_uploads: missing },
      });
    }
    this.uploads.take(files);
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

/**
 * The uploads of efrd as `attachment.put` keeps them: efrd writes a chunk only at the
 * offset that it has, and else answers with what it has.
 */
export class FakeUploads {
  /** Every put, in order. */
  readonly puts: AttachmentPut[] = [];
  private readonly held = new Map<string, { received: number; total: number }>();

  /** The bytes that efrd has of upload `id`, or undefined for none. */
  received(id: string): number | undefined {
    return this.held.get(id)?.received;
  }

  /** True when efrd has every byte of upload `id`. */
  finished(id: string): boolean {
    const upload = this.held.get(id);
    return upload !== undefined && upload.received === upload.total;
  }

  put(params: AttachmentPut): AttachmentPutResult {
    this.puts.push(params);
    const upload = this.held.get(params.upload_id) ?? { received: 0, total: params.total_bytes };
    const bytes = base64Decode(params.data)?.length ?? 0;
    if (params.offset === upload.received) {
      upload.received += bytes;
    }
    this.held.set(params.upload_id, upload);
    if (upload.received < upload.total) {
      return { received: upload.received };
    }
    return {
      received: upload.received,
      upload: {
        name: params.name,
        bytes: upload.total,
        media_type: "image/jpeg",
        kind: { kind: "image", width: 2, height: 1 },
      },
    };
  }

  /** Removes uploads, as a restart of efrd or a prompt that takes them. */
  take(ids: readonly string[]): void {
    for (const id of ids) {
      this.held.delete(id);
    }
  }
}

/**
 * The uploads of a drain on `efrd`, with `chunk` as the largest chunk. `reads` holds each
 * chunk read as `uri@offset`, and `discards` each discarded upload.
 */
export function fakeUploads(
  efrd: FakeEfrd,
  chunk = 4,
): OutboxUploads & { readonly discards: string[]; readonly reads: string[] } {
  const discards: string[] = [];
  const reads: string[] = [];
  return {
    discards,
    reads,
    chunkBytes: () => chunk,
    put: (params) => Promise.resolve(efrd.uploads.put(params)),
    discard: (uploadId) => {
      discards.push(uploadId);
      efrd.uploads.take([uploadId]);
    },
    readChunk: (uri, offset, length) => {
      reads.push(`${uri}@${offset}`);
      return Promise.resolve(base64Encode(new Uint8Array(length)));
    },
  };
}

/** A file of `bytes` bytes for an outbox entry, with no byte uploaded yet. */
export function outboxFile(uploadId: string, bytes: number, name = "shot.jpg"): OutboxAttachment {
  return {
    uploadId,
    kind: "photo",
    name,
    uri: `file:///caches/${name}`,
    bytes,
    previewUri: `file:///caches/${name}-preview`,
    received: 0,
    uploaded: false,
  };
}
