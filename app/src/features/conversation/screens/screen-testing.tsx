// Helpers for the router tests of the screens of the conversation feature: the routes of
// the app around the real blank root and conversation screens, on the services of the
// app over fake sockets (features/navigation/model/testing.ts). The plain stack of
// expo-router stands in for the root stack, whose headers belong to another feature.
// Only test files import this module.

import { QueryClientProvider } from "@tanstack/react-query";
import { Stack, useLocalSearchParams } from "expo-router";
import { Text } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import type { SubscribeItem } from "@/lib/protocol/wire";
import { flush } from "@/lib/net/testing";
import type { MachineId } from "@/features/machines/model/machine-ref";
import { machineRecord } from "@/features/machines/model/testing";
import { lastSocket, type TestServices } from "@/features/navigation/model/testing";
import { conversationParamsSchema } from "@/features/navigation/model/route-schemas";
import { AppServicesContext } from "@/features/navigation/use-app-services";
import { BlankRootScreen } from "./blank-root-screen";
import { ConversationScreen } from "./conversation-screen";

/** The screen of an iPhone 17 Pro. */
export const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

/** The conversation route, as src/app/c/[machine]/[id].tsx renders it. */
export function ConversationRoute() {
  const params = conversationParamsSchema.safeParse(useLocalSearchParams());
  if (!params.success) {
    return <ConversationScreen conversation={undefined} />;
  }
  const { machine, id, focus } = params.data;
  return <ConversationScreen conversation={{ machine, id }} focus={focus} />;
}

/** The routes of the app around the screens, on `test`. */
export function screenRoutes(test: TestServices) {
  const { services } = test;
  function Layout() {
    return (
      <SafeAreaProvider initialMetrics={METRICS}>
        <QueryClientProvider client={services.queryClient}>
          <AppServicesContext value={services}>
            <Stack />
          </AppServicesContext>
        </QueryClientProvider>
      </SafeAreaProvider>
    );
  }
  return {
    _layout: Layout,
    index: () => <BlankRootScreen noMachine={<Text>no machine root</Text>} />,
    "c/[machine]/[id]": ConversationRoute,
    settings: () => <Text>settings</Text>,
  };
}

/** The socket of `machine` that the app uses now. */
export function socketOf(test: TestServices, machine: MachineId) {
  const record = machineRecord(machine);
  return lastSocket(test.sockets, `ws://${record.host}:${record.port}/`);
}

/** Sends `items` on the newest subscription of `conversation`, then lets the jobs run. */
export async function stream(
  test: TestServices,
  machine: MachineId,
  conversation: string,
  items: readonly SubscribeItem[],
): Promise<void> {
  const socket = socketOf(test, machine);
  let id: unknown;
  for (const frame of socket.requests("conversation.subscribe")) {
    const params = frame["params"];
    if (
      typeof params === "object" &&
      params !== null &&
      "conversation_id" in params &&
      params.conversation_id === conversation
    ) {
      id = frame["id"];
    }
  }
  if (typeof id !== "number") {
    throw new Error(`no subscription of ${conversation}`);
  }
  for (const item of items) {
    socket.receive({ id, item });
  }
  await flush();
}
