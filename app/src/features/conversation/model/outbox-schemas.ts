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
}) satisfies z.ZodType<OutboxEntry>;
