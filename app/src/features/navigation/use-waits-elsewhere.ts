// The waits in other conversations (plan section 2.2, "Approvals elsewhere"): an
// approval, a question of the model or a wait for the terminal. They feed the dot on the
// title chevron and the strip under the header.
//
// The source is the recent list of each connected machine. The watch of the lists of the
// machine (`conversations.watch`) writes it as efrd commits; for an efrd without the
// watch, the list polls every 10 s while the app is active (D7). The list of a machine
// that is not connected is left out: its rows are the last ones that the phone saw, and
// an old "waiting" must not show.
//
// NOTE: The plan also names the streams of the stacked conversations as a source. Only
// the conversation on screen has a subscription (the others are held without one), and
// that one never counts, so a stacked stream is older than the list. The hook
// gives no live streams to the model for that reason.

import { useQueries } from "@tanstack/react-query";

import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { recentConversationsQueryOptions } from "@/features/conversations/conversations-queries";
import type { ConversationItem } from "@/features/conversations/model/conversation-item";
import { waitsElsewhere, type WaitsElsewhere } from "@/features/conversations/model/waiting";
import { onlineMachines } from "./model/machine-connections";
import { useRuntime } from "./use-app-services";
import { useMachineConnections, useMachines } from "./use-machines";

/** The waits of the conversations other than `current`, on every machine. */
export function useWaitsElsewhere(current: ConversationRef | undefined): WaitsElsewhere {
  const runtime = useRuntime();
  const machines = useMachines();
  const online = onlineMachines(useMachineConnections());
  const listed = useQueries({
    queries: machines.map((machine) => recentConversationsQueryOptions(runtime, machine.id)),
    combine: (results): ConversationItem[] => results.flatMap((result) => result.data ?? []),
  });
  return waitsElsewhere({
    current,
    live: [],
    listed: listed.filter((item) => online.has(item.machine)),
  });
}
