// The schemas of the route params (plan section 2.3, item 9). A route param comes from a
// URL, so the app checks it before it uses it: the machine and the conversation are UUIDs.
// A new conversation gets the local id of its outbox target (also a UUID) until efrd
// names it.

import { z } from "zod";

/** The params of `c/[machine]/[id]`. */
export const conversationParamsSchema = z.object({
  /** The daemon id of the machine. */
  machine: z.guid(),
  /** The conversation id, or the local id of a new conversation. */
  id: z.guid(),
  /** `instant`: the first send on the blank root pushed it without an animation. */
  enter: z.literal("instant").exactOptional(),
  /** `approval`: the screen scrolls to the approval that waits. */
  focus: z.literal("approval").exactOptional(),
});

/** The params of `conversations` (the All conversations sheet). */
export const conversationsParamsSchema = z.object({
  /** `waiting`: the sheet shows the conversations that wait for an answer on top. */
  section: z.literal("waiting").exactOptional(),
});

/** The params of `approval` (the approval sheet). */
export const approvalParamsSchema = z.object({
  /** The daemon id of the machine. */
  machine: z.guid(),
  /** The conversation id. */
  id: z.guid(),
  /** The tool call whose question the sheet shows. */
  call: z.string().min(1),
});

/** The params of `model` (the model sheet of a composer). */
export const modelSheetParamsSchema = z.object({
  /** The daemon id of the machine. */
  machine: z.guid(),
  /** The id of the composer: a conversation, or the local id of the blank root. */
  id: z.guid(),
  /** `1`: the composer of the blank root of the machine. */
  root: z.literal("1").exactOptional(),
  /** The model that the sheet selects when it opens, as a switch offer names it. */
  model: z.string().min(1).exactOptional(),
  /** The account of `model`, such as `openai-subscription.b`. */
  provider: z.string().min(1).exactOptional(),
});

/** The params of `account` (the limits of one account in Settings). */
export const accountParamsSchema = z.object({
  /** The daemon id of the machine. */
  machine: z.guid(),
  /** The provider id of the account, such as `openai-subscription.b`. */
  provider: z.string().min(1),
});

/** The params of `pair`. */
export const pairParamsSchema = z.object({
  /** The daemon id of the machine that the user pairs again. */
  machine: z.guid().exactOptional(),
});

/**
 * The data of a push notice of efrd: the daemon id and the conversation id, the only parts
 * of a notice that the app reads.
 */
export const noticeDataSchema = z.object({
  d: z.guid(),
  c: z.guid(),
});

/** The params of `settings`. */
export const settingsParamsSchema = z.object({
  /** `unlock`: Settings scrolls to the row "Face ID unlock" and marks it for a moment. */
  focus: z.literal("unlock").exactOptional(),
});
