// The title of a conversation for the bar and the back menu.

import { useQuery } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";

import type { MachineId } from "@/features/machines/model/machine-ref";
import { recentConversationsQueryOptions } from "@/features/conversations/conversations-queries";
import { streamTitles } from "./model/stream-titles";
import { useRuntime } from "./use-app-services";

/**
 * The title of conversation `id` on `machine`: the title of its stream, else the one of
 * the recent list, else undefined (a new conversation has no title yet).
 */
export function useConversationTitle(machine: MachineId, id: string): string | undefined {
  const runtime = useRuntime();
  const titles = streamTitles(runtime);
  const fromStream = useSyncExternalStore(
    (onChange) => titles.subscribe({ machine, id }, onChange),
    () => titles.title({ machine, id }),
  );
  const { data } = useQuery(recentConversationsQueryOptions(runtime, machine));
  return fromStream ?? data?.find((item) => item.id === id)?.title;
}
