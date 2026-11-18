// The schemas of the attachments: the `data` of a `not_found` error of `prompt.send`
// whose uploads efrd lost (`MissingUploads` of efr-protocol), and the previews of the
// photos that the app keeps.

import { z } from "zod";

/** `error.data` of a `prompt.send` whose uploads are not finished uploads of the phone. */
export const missingUploadsSchema = z.object({ missing_uploads: z.array(z.guid()).min(1) });

/**
 * The saved previews: from the command id of a prompt to its machine and the preview of
 * each of its files, or null for a file with none.
 */
export const savedPreviewsSchema = z.record(
  z.guid(),
  z.object({ machine: z.guid(), uris: z.array(z.string().nullable()) }),
);
