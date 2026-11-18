// The schema of an outbox entry on the disk. The outbox reads each row with it, so a row
// of an older build or a broken row never reaches the drain.

import { z } from "zod";

import type { OutboxEntry } from "./outbox";

const outboxTargetSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("conversation"), conversationId: z.guid() }),
  z.object({ kind: z.literal("new"), localId: z.guid() }),
]);

/** One entry of the outbox, as `JSON.stringify` wrote it. */
export const outboxEntrySchema = z.object({
  commandId: z.guid(),
  machine: z.guid(),
  target: outboxTargetSchema,
  text: z.string(),
  createdAt: z.number().nonnegative(),
  order: z.number().nonnegative(),
  attempts: z.int().nonnegative(),
  state: z.enum(["pending", "sending", "sent", "failed"]),
  retryAt: z.number().nonnegative(),
  error: z.object({ code: z.string(), message: z.string() }).nullable(),
  sent: z
    .object({
      conversationId: z.guid(),
      turnId: z.guid(),
      seq: z.int().nonnegative(),
      queued: z.boolean(),
    })
    .nullable(),
  settings: z
    .object({
      model: z.string(),
      provider: z.string().exactOptional(),
      effort: z.string().exactOptional(),
    })
    .exactOptional(),
  auto: z.literal(true).exactOptional(),
  attachments: z
    .array(
      z.object({
        uploadId: z.guid(),
        kind: z.enum(["photo", "pdf", "file"]),
        name: z.string().min(1),
        uri: z.string(),
        bytes: z.int().nonnegative(),
        previewUri: z.string().exactOptional(),
        received: z.int().nonnegative(),
        uploaded: z.boolean(),
      }),
    )
    .exactOptional(),
  reuploads: z.int().nonnegative().exactOptional(),
}) satisfies z.ZodType<OutboxEntry>;
