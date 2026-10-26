// The params of the board route of the gallery (`/board?name=...`): the name of a
// component board, the visit that opened it (a new visit gets a new sandbox), and for
// the render counter the conversation that it measures. Pure.

import { z } from "zod";

const boardParamsSchema = z.object({
  name: z.string().min(1),
  visit: z.string().exactOptional(),
  machine: z.guid().exactOptional(),
  id: z.guid().exactOptional(),
});

/** The params of the board route. */
export type BoardParams = z.infer<typeof boardParamsSchema>;

/** The params of the board route, or a board with no name when they are not valid. */
export function boardParams(params: unknown): BoardParams {
  const parsed = boardParamsSchema.safeParse(params);
  return parsed.success ? parsed.data : { name: "" };
}
