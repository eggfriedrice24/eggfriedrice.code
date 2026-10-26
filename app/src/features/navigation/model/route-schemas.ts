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

/** The params of `pair`. */
export const pairParamsSchema = z.object({
  /** The daemon id of the machine that the user pairs again. */
  machine: z.guid().exactOptional(),
});
