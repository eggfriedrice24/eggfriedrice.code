// Small builders for the tests of the list models. Only test files import this module.

import type { ConversationItem } from "./conversation-item";

/** A conversation item of `machine` with the members of `extra`. */
export function item(
  machine: string,
  id: string,
  updatedAt: number,
  extra: {
    readonly title?: string;
    readonly cwd?: string;
    readonly status?: ConversationItem["status"];
  } = {},
): ConversationItem {
  const built: {
    machine: string;
    id: string;
    title?: string;
    status: ConversationItem["status"];
    updatedAt: number;
    cwd?: string;
  } = { machine, id, status: extra.status ?? "idle", updatedAt };
  if (extra.title !== undefined) {
    built.title = extra.title;
  }
  if (extra.cwd !== undefined) {
    built.cwd = extra.cwd;
  }
  return built;
}

/** Local time `day` of October 2026 at `hour:minute`, in ms since the epoch. */
export function october(day: number, hour = 12, minute = 0): number {
  return new Date(2026, 9, day, hour, minute).getTime();
}
