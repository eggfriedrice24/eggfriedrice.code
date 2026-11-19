// The drain of the outbox for one machine. The supervisor of the machine makes one, calls
// `start` on each `ready` of its client, and `stop` when the connection goes down. While
// it runs, the drain sends each due entry (`drainPlan` in outbox.ts) with `prompt.send`,
// and sends again when the outbox changes or a wait ends.
//
// - A lost connection (a ConnectionError of a kind in LOST_KINDS) makes the entry
//   pending again with no wait, and holds the drain until the next `start`, so a dead
//   client cannot make a loop of sends that fail at once.
// - Another ConnectionError, such as a result that its schema refused (`malformed`),
//   fails the entry: the connection stays ready, so no `start` comes to free the drain,
//   and a resend with the same command id gets the same answer.
// - `busy` and `internal` are retryable: the entry waits `retry_after_ms` of the error,
//   else the backoff ladder of src/lib/net/backoff.ts with a cap of 16 s (t3code), and
//   fails after MAX_SEND_ATTEMPTS. An error that is not efrd's counts as retryable too.
// - Every other error of efrd fails the entry: a resend with the same command id gets
//   the same answer.
//
// - The files of an entry go first (ADR 0026, docs/protocol.md `attachment.put`): each
//   with `attachment.put`, one chunk in flight, from the bytes that efrd has. A lost link
//   keeps the progress, and the next start goes on there. When `prompt.send` answers
//   `not_found` with `missing_uploads`, efrd lost those uploads: they go again with the
//   same ids and the same command id, at most MAX_REUPLOADS times, and then the entry
//   fails with UPLOAD_LOST. An entry that the user withdraws during its upload stops at
//   the next chunk. A file that iOS purged from the caches fails the entry at once.
//
// - A dev build logs each failure, each wait, a send that gets no answer for
//   STUCK_SEND_MS, and a lane that waits behind a send in flight, so the Metro log
//   tells why a prompt did not go.
//
// Pure TypeScript: the caller gives the send function, the clock and the random value.

import { DEV_WARNINGS, devWarn } from "@/lib/dev/dev-warn";
import { backoffDelayMs, type BackoffConfig } from "@/lib/net/backoff";
import type { Clock, Timer } from "@/lib/net/clock";
import {
  ConnectionError,
  type ConnectionErrorKind,
  ServerError,
  retryAfterMs,
} from "@/lib/net/errors";
import type {
  AttachmentPut,
  AttachmentPutResult,
  CommandId,
  DaemonId,
  PromptSend,
  PromptSendResult,
  UploadId,
} from "@/lib/protocol/generated";
import { FILE_GONE } from "../../../../modules/efr-files/src/EfrFiles.types";
import { missingUploadsSchema } from "./attachments-schemas";
import { UPLOAD_LOST, fileGoneLine } from "./attachments-format";
import { nativeErrorCode } from "./native-error";
import {
  drainPlan,
  laneOf,
  sendParams,
  type Outbox,
  type OutboxAttachment,
  type OutboxEntry,
  type OutboxError,
  type OutboxSnapshot,
} from "./outbox";

/** The wait between the attempts of one entry: 0.5 s, then twice as long, at most 16 s. */
export const OUTBOX_BACKOFF: BackoffConfig = { baseMs: 250, maxMs: 16_000, resetAfterMs: 0 };

/** The kinds of ConnectionError that tell that the client lost its socket. */
const LOST_KINDS: ReadonlySet<ConnectionErrorKind> = new Set([
  "socket",
  "closed",
  "offline",
  "silent",
  "timeout",
]);

/** After this wait for the answer of efrd, a dev build logs the send as stuck. */
export const STUCK_SEND_MS = 15_000;

/** The codes of efrd after which a resend can work. */
const RETRYABLE_CODES: ReadonlySet<string> = new Set(["busy", "internal"]);

/** What the drain needs to upload the files of a prompt. */
export interface OutboxUploads {
  /**
   * The largest chunk of efrd (`chunk_bytes` of the capability `attachments`), or
   * undefined when the connection has no such capability.
   */
  readonly chunkBytes: () => number | undefined;
  /** Sends `attachment.put` on the client of the machine. */
  readonly put: (params: AttachmentPut) => Promise<AttachmentPutResult>;
  /** Sends `attachment.discard` and does not wait. */
  readonly discard: (uploadId: UploadId) => void;
  /** At most `length` bytes of the file at `uri` from `offset`, as standard base64. */
  readonly readChunk: (uri: string, offset: number, length: number) => Promise<string>;
}

/** The error of an entry whose uploads efrd lost MAX_REUPLOADS times. */
const UPLOAD_ERROR: OutboxError = { code: "upload", message: UPLOAD_LOST };

/** The error of an entry with files for an efrd that takes none. */
const NO_ATTACHMENTS: OutboxError = {
  code: "upload",
  message: "efrd on this machine takes no attachments.",
};

/** The end of a send that no later step may change: the entry fails with `error`. */
class FinalError extends Error {
  readonly error: OutboxError;

  constructor(error: OutboxError) {
    super(error.message);
    this.error = error;
  }
}

/** The user withdrew the entry during its upload. */
class WithdrawnError extends Error {}

/** The uploads that efrd lost, from the error of `prompt.send`, or undefined. */
function missingUploads(error: unknown): readonly UploadId[] | undefined {
  if (!(error instanceof ServerError) || error.body.code !== "not_found") {
    return undefined;
  }
  const parsed = missingUploadsSchema.safeParse(error.body.data);
  return parsed.success ? parsed.data.missing_uploads : undefined;
}

/** What the drain needs. */
export interface OutboxDrainOptions {
  readonly outbox: Outbox;
  /** The daemon id of the machine. The drain sends only its entries. */
  readonly machine: DaemonId;
  /** Sends `prompt.send` on the client of the machine. */
  readonly send: (params: PromptSend) => Promise<PromptSendResult>;
  readonly clock: Clock;
  /** A random value in [0, 1) for the backoff ladder. */
  readonly random: () => number;
  readonly backoff?: BackoffConfig;
  /** Called after efrd took an entry, with its result. */
  readonly onSent?: (entry: OutboxEntry, result: PromptSendResult) => void;
  /** True while an entry may go out in auto: the Face ID unlock is on. */
  readonly autoAllowed: () => boolean;
  /** The uploads of the files of an entry. Without them, an entry with files fails. */
  readonly uploads?: OutboxUploads;
}

/** The drain of one machine. */
export interface OutboxDrain {
  /** The client is ready: send the due entries, and each new one. */
  start(): void;
  /** The client is not ready: send nothing more. A send in flight still ends. */
  stop(): void;
  /** True while the drain runs and no lost connection holds it. */
  readonly running: boolean;
}

/** The error of an entry for a failure of its send. */
function outboxError(error: unknown): OutboxError {
  if (error instanceof ServerError) {
    return { code: error.body.code, message: error.body.message };
  }
  if (error instanceof ConnectionError) {
    return { code: "connection", message: error.message };
  }
  if (error instanceof Error) {
    return { code: "client", message: error.message };
  }
  return { code: "client", message: String(error) };
}

/**
 * The lanes of `machine` whose head is in flight while other entries wait behind it, as
 * the command id of the head and the number of entries that wait.
 */
export function blockedLanes(
  snapshot: OutboxSnapshot,
  machine: string,
): readonly { readonly head: string; readonly waiting: number }[] {
  const heads = new Map<string, { head: OutboxEntry; waiting: number }>();
  for (const entry of snapshot.entries) {
    if (entry.machine !== machine || entry.state === "sent" || entry.state === "failed") {
      continue;
    }
    const lane = laneOf(snapshot, entry);
    const held = heads.get(lane);
    if (held === undefined) {
      heads.set(lane, { head: entry, waiting: 0 });
    } else {
      held.waiting += 1;
    }
  }
  const blocked: { head: string; waiting: number }[] = [];
  for (const { head, waiting } of heads.values()) {
    if (head.state === "sending" && waiting > 0) {
      blocked.push({ head: head.commandId, waiting });
    }
  }
  return blocked;
}

/** Makes the drain of `options.machine`. It does nothing before `start`. */
export function createOutboxDrain(options: OutboxDrainOptions): OutboxDrain {
  const { outbox, machine, clock } = options;
  const backoff = options.backoff ?? OUTBOX_BACKOFF;
  let running = false;
  let unsubscribe: (() => void) | undefined;
  let timer: Timer | undefined;
  let pumping = false;
  let again = false;
  // The number of the current start; a send keeps the number of the start that sent it.
  let generation = 0;

  const halt = (): void => {
    running = false;
    unsubscribe?.();
    unsubscribe = undefined;
    timer?.cancel();
    timer = undefined;
  };

  // The heads of the blocked lanes that the log told already.
  const told = new Set<string>();

  const settle = (commandId: CommandId, error: unknown, sentIn: number): void => {
    if (error instanceof WithdrawnError) {
      return;
    }
    if (error instanceof FinalError) {
      devWarn(`the outbox fails prompt ${commandId}: ${error.message}`);
      outbox.fail(commandId, error.error);
      return;
    }
    const reason = outboxError(error);
    if (error instanceof ConnectionError && !LOST_KINDS.has(error.kind)) {
      devWarn(`the outbox fails prompt ${commandId}: ${reason.code}: ${reason.message}`);
      outbox.fail(commandId, reason);
      return;
    }
    if (error instanceof ConnectionError) {
      // The client is down. The supervisor calls `start` on its next ready. The drain
      // halts first: the release changes the outbox, and a running drain would send
      // the entry again at once. A send of an older start is different: its socket
      // closed after a new connection came up, so the drain sends again on the new one.
      if (sentIn === generation) {
        halt();
      }
      devWarn(`prompt ${commandId} waits for the next connection: ${reason.message}`);
      outbox.release(commandId, reason);
      return;
    }
    if (error instanceof ServerError && !RETRYABLE_CODES.has(error.body.code)) {
      devWarn(`efrd refused prompt ${commandId}: ${reason.code}: ${reason.message}`);
      outbox.fail(commandId, reason);
      return;
    }
    const entry = outbox.store.get().entries.find((held) => held.commandId === commandId);
    const attempt = entry?.attempts ?? 0;
    const asked = error instanceof ServerError ? retryAfterMs(error.body) : undefined;
    const wait = asked ?? backoffDelayMs(attempt, options.random(), backoff);
    devWarn(`prompt ${commandId} tries again in ${wait} ms: ${reason.code}: ${reason.message}`);
    outbox.retryLater(commandId, reason, clock.now() + wait);
  };

  const current = (commandId: CommandId): OutboxEntry | undefined =>
    outbox.store.get().entries.find((held) => held.commandId === commandId);

  // Sends the chunks of `file` from the bytes that efrd has, one at a time, until efrd
  // names the upload. The one result rule of `attachment.put`: efrd writes a chunk only
  // at the offset that it has, and else answers with that offset.
  const uploadFile = async (
    commandId: CommandId,
    file: OutboxAttachment,
    uploads: OutboxUploads,
    chunk: number,
  ): Promise<void> => {
    const read = async (offset: number, length: number): Promise<string> => {
      try {
        return await uploads.readChunk(file.uri, offset, length);
      } catch (error) {
        // A new try cannot find the file either.
        if (nativeErrorCode(error) === FILE_GONE) {
          throw new FinalError({ code: "upload", message: fileGoneLine(file.name) });
        }
        throw error;
      }
    };
    const put = async (offset: number): Promise<AttachmentPutResult> => {
      const length = Math.max(0, Math.min(chunk, file.bytes - offset));
      const data = length > 0 ? await read(offset, length) : "";
      return uploads.put({
        upload_id: file.uploadId,
        name: file.name,
        total_bytes: file.bytes,
        offset,
        data,
      });
    };
    let offset = file.received;
    for (;;) {
      // oxlint-disable-next-line no-await-in-loop -- one chunk in flight.
      const answer = await put(offset);
      if (current(commandId) === undefined) {
        // NOTE: the withdraw sent its discard before this chunk arrived.
        uploads.discard(file.uploadId);
        throw new WithdrawnError();
      }
      const uploaded = answer.upload !== undefined && answer.upload !== null;
      outbox.progress(commandId, file.uploadId, answer.received, uploaded);
      if (uploaded) {
        return;
      }
      if (answer.received === offset) {
        throw new Error(`efrd took no bytes of upload ${file.uploadId} at ${offset}`);
      }
      offset = answer.received;
    }
  };

  const uploadFiles = async (
    commandId: CommandId,
    files: readonly OutboxAttachment[],
  ): Promise<void> => {
    const uploads = options.uploads;
    const chunk = uploads?.chunkBytes();
    if (uploads === undefined || chunk === undefined) {
      throw new FinalError(NO_ATTACHMENTS);
    }
    for (const file of files) {
      if (!file.uploaded) {
        // oxlint-disable-next-line no-await-in-loop -- one chunk in flight.
        await uploadFile(commandId, file, uploads, chunk);
      }
    }
  };

  // The files that efrd does not have yet, then `prompt.send`.
  const sendWithFiles = async (
    commandId: CommandId,
    params: PromptSend,
  ): Promise<PromptSendResult> => {
    // NOTE: a prompt with no files goes in the same tick, with no await before it.
    const files = current(commandId)?.attachments;
    if (files !== undefined) {
      await uploadFiles(commandId, files);
    }
    // Only a dev build keeps this timer: it only writes to the log.
    const stuck = DEV_WARNINGS
      ? clock.setTimer(STUCK_SEND_MS, () => {
          devWarn(`prompt ${commandId} has no answer of efrd after ${STUCK_SEND_MS} ms`);
        })
      : undefined;
    try {
      return await options.send(params);
    } finally {
      stuck?.cancel();
    }
  };

  // `sendWithFiles` again for each `missing_uploads`, at most MAX_REUPLOADS times.
  const deliver = async (commandId: CommandId, params: PromptSend): Promise<PromptSendResult> => {
    for (;;) {
      try {
        // oxlint-disable-next-line no-await-in-loop -- a round waits for the uploads that efrd lost.
        return await sendWithFiles(commandId, params);
      } catch (error) {
        const missing = missingUploads(error);
        if (missing === undefined) {
          throw error;
        }
        if (!outbox.reupload(commandId, missing)) {
          throw new FinalError(UPLOAD_ERROR);
        }
        devWarn(`efrd lost ${missing.length} upload(s) of prompt ${commandId}: they go again`);
      }
    }
  };

  const sendOne = (entry: OutboxEntry): void => {
    const params = sendParams(outbox.store.get(), entry, options.autoAllowed());
    if (!outbox.begin(entry.commandId)) {
      return;
    }
    const sentIn = generation;
    const run = async (): Promise<void> => {
      let result: PromptSendResult;
      try {
        // A send that throws at once is a failed send too.
        result = await deliver(entry.commandId, params);
      } catch (error) {
        settle(entry.commandId, error, sentIn);
        return;
      }
      outbox.succeed(entry.commandId, result);
      options.onSent?.(entry, result);
    };
    void run().catch(() => undefined);
  };

  const tellBlocked = (snapshot: OutboxSnapshot): void => {
    for (const lane of blockedLanes(snapshot, machine)) {
      if (!told.has(lane.head)) {
        told.add(lane.head);
        devWarn(`${lane.waiting} prompt(s) wait behind prompt ${lane.head}, which is in flight`);
      }
    }
  };

  const pump = (): void => {
    if (!running) {
      return;
    }
    // A send changes the outbox, which calls pump again; that run waits for this one.
    if (pumping) {
      again = true;
      return;
    }
    pumping = true;
    try {
      for (;;) {
        again = false;
        timer?.cancel();
        timer = undefined;
        const snapshot = outbox.store.get();
        const plan = drainPlan(snapshot, machine, clock.now());
        tellBlocked(snapshot);
        for (const entry of plan.due) {
          if (!running) {
            return;
          }
          sendOne(entry);
        }
        if (running && plan.wakeAt !== undefined) {
          timer = clock.setTimer(plan.wakeAt - clock.now(), pump);
        }
        if (!again || !running) {
          break;
        }
      }
    } finally {
      pumping = false;
    }
  };

  return {
    start(): void {
      if (running) {
        pump();
        return;
      }
      running = true;
      generation += 1;
      const subscription = outbox.store.subscribe(() => pump());
      unsubscribe = () => subscription.unsubscribe();
      pump();
    },
    stop(): void {
      halt();
    },
    get running(): boolean {
      return running;
    },
  };
}
