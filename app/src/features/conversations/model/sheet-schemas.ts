// The params of the route of the All conversations sheet (`/conversations`), parsed with
// zod as every route param is (plan section 3).
//
// `machine` is the machine on screen when the sheet opens. Its search hits come first.
// It only orders the rows, so a param that is not a machine id is ignored, and the sheet
// still opens.

import { z } from "zod";

/** The params of `/conversations`. */
export const sheetParamsSchema = z.object({
  /** The `daemonId` of the machine on screen. */
  machine: z.guid().exactOptional(),
});

/** The params of the sheet after the parse. */
export type SheetParams = z.infer<typeof sheetParamsSchema>;

/** The params of the sheet from the raw route params; bad params give none. */
export function sheetParams(raw: unknown): SheetParams {
  const parsed = sheetParamsSchema.safeParse(raw);
  return parsed.success ? parsed.data : {};
}
