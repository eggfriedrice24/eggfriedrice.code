// The schema of the saved preferences. Each field falls back to its default on its own,
// so one bad or new value does not reset the others. The defaults are the same as
// DEFAULT_PREFERENCES in preferences.ts; a test checks it.

import { z } from "zod";

/** The preferences as `JSON.stringify` wrote them; a missing field gets its default. */
export const savedPreferencesSchema = z.object({
  haptics: z.boolean().catch(true),
  sendMode: z.enum(["steer", "queue"]).catch("steer"),
  lastMachine: z.guid().nullable().catch(null),
});
