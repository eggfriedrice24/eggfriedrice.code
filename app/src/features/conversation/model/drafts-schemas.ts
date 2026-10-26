// The schema of the saved drafts: one JSON object from draft key to text.

import { z } from "zod";

/** The drafts as `JSON.stringify` wrote them. */
export const savedDraftsSchema = z.record(z.string(), z.string());
