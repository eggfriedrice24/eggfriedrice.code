// The params of the pairing route (plan section 2.3 item 9): `/pair?machine=<daemon id>`
// pairs that machine again; without it, the route pairs a new machine.

import { z } from "zod";

import type { MachineId } from "@/features/machines/model/machine-ref";

/** The params of `/pair`. */
export const pairRouteParamsSchema = z.object({ machine: z.guid().exactOptional() });

/**
 * The machine to pair again that the params name, or null for a new machine. A param that
 * is not one daemon id pairs a new machine.
 */
export function pairRouteMachine(params: unknown): MachineId | null {
  const parsed = pairRouteParamsSchema.safeParse(params);
  return parsed.success ? (parsed.data.machine ?? null) : null;
}
