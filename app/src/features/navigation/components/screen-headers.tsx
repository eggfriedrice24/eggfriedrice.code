// The header of each screen of the root stack (design section 2, "Per screen"):
//
// - Blank root: the machine menu on the left, the machine name in mono with the title
//   menu, Settings and `+`. No strip.
// - Root without a machine (the pairing intro): Settings only.
// - Conversation: the native back button (with its history menu), the title of the
//   conversation, which opens the conversations sheet, Settings and `+`, and the slot
//   under the bar.
//
// The machine menu belongs to the machines feature, and a feature does not import the
// components of another one, so the root layout passes it in (`machineMenu`).

import { Stack } from "expo-router";
import type { ReactNode } from "react";

import type { ConversationRef, MachineId } from "@/features/machines/model/machine-ref";
import { rowTitle } from "@/features/conversations/model/list-format";
import { useMachines, useRootMachine } from "../use-machines";
import { useConversationTitle } from "../use-conversation-title";
import { HeaderActions } from "./header-actions";
import { HeaderSlot } from "./header-slot";
import { HeaderTitle } from "./header-title";

/** Renders the machine menu of the blank root of `machine`. */
export type MachineMenuSlot = (machine: MachineId) => ReactNode;

export interface RootHeaderProps {
  readonly machineMenu: MachineMenuSlot;
}

/** The header of the root: the blank root, or the pairing intro without a machine. */
export function RootHeader(props: RootHeaderProps) {
  const machine = useRootMachine();
  if (machine === undefined) {
    return (
      <>
        <Stack.Screen options={{ title: "" }} />
        <HeaderActions rootMachine={undefined} newConversation={false} />
      </>
    );
  }
  return (
    <>
      {props.machineMenu(machine.id)}
      <HeaderTitle machine={machine} current={undefined} title={machine.name} />
      <HeaderActions rootMachine={machine.id} newConversation />
    </>
  );
}

export interface ConversationHeaderProps {
  readonly conversation: ConversationRef;
}

/** The header of a conversation, and the slot under its bar. */
export function ConversationHeader(props: ConversationHeaderProps) {
  const { conversation } = props;
  const machines = useMachines();
  const title = rowTitle(useConversationTitle(conversation.machine, conversation.id));
  const machine = machines.find((one) => one.id === conversation.machine);
  return (
    <>
      {machine === undefined ? (
        <Stack.Screen options={{ title }} />
      ) : (
        <HeaderTitle machine={machine} current={conversation} title={title} />
      )}
      <HeaderActions rootMachine={undefined} newConversation />
      <HeaderSlot current={conversation} />
    </>
  );
}
