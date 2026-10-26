// The data of the header title: the rows of the title menu (the recent conversations of
// the machine of the screen) and the title of a conversation.

import { useQuery } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";

import type { ConversationRef, MachineId } from "@/features/machines/model/machine-ref";
import { recentConversationsQueryOptions } from "@/features/conversations/conversations-queries";
import { titleMenuRows, type TitleMenuRow } from "@/features/conversations/model/title-menu";
import { streamTitles } from "./model/stream-titles";
import { useRuntime } from "./use-app-services";
import { useNow } from "./use-now";

/** The rows of the title menu of `machine`; `current` is the conversation on screen. */
export function useTitleMenuRows(
  machine: MachineId,
  current: ConversationRef | undefined,
): TitleMenuRow[] {
  const runtime = useRuntime();
  const { data } = useQuery(recentConversationsQueryOptions(runtime, machine));
  const now = useNow();
  return titleMenuRows({ machine, items: data ?? [], current, now });
}

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
