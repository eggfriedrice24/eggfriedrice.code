// The schema of the saved model choices: the choice per draft key, and the draft keys
// whose costly switches ask no more ("Don't ask again here").

import { z } from "zod";

/** One choice as `JSON.stringify` wrote it. */
const savedModelChoiceSchema = z.object({
  model: z.string(),
  provider: z.string().exactOptional(),
  effort: z.string().exactOptional(),
});

/** The choices as `JSON.stringify` wrote them. A bad part falls back to none. */
export const savedModelChoicesSchema = z.object({
  choices: z.record(z.string(), savedModelChoiceSchema).catch({}),
  quiet: z.array(z.string()).catch([]),
});
