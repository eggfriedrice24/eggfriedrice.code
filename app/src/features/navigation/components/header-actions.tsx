// Settings and `+` on the right of the bar (design section 2): native bar buttons, which
// iOS 26 draws as one glass group. The same on every screen; the root without a machine
// has Settings only.

import { Stack } from "expo-router";

import type { MachineId } from "@/features/machines/model/machine-ref";
import { useHeaderActions } from "../use-header-actions";

export interface HeaderActionsProps {
  /** The machine of the blank root; `+` on the root clears its draft. */
  readonly rootMachine: MachineId | undefined;
  /** False on the root without a machine: Settings only. */
  readonly newConversation: boolean;
}

/** The buttons on the right of the bar. */
export function HeaderActions(props: HeaderActionsProps) {
  const actions = useHeaderActions(props.rootMachine);
  return (
    <Stack.Toolbar placement="right">
      <Stack.Toolbar.Button
        icon="gearshape"
        accessibilityLabel="Settings"
        onPress={actions.openSettings}
      />
      {props.newConversation ? (
        <Stack.Toolbar.Button
          icon="plus"
          accessibilityLabel="New conversation"
          onPress={actions.newConversation}
        />
      ) : null}
    </Stack.Toolbar>
  );
}
