// The machine menu on the left of the bar of the blank root (design sections 2 and 3.13,
// board R-MachineMenu): a native bar button with a UIMenu of the paired machines, each
// with its state, and "Pair a new machine". A pick makes the blank root start on that
// machine; the root reads the choice from the preferences, so its shell block and its
// title change with it.

import { Stack } from "expo-router";

import { usePreferences } from "@/features/navigation/use-app-services";
import { useHeaderActions } from "@/features/navigation/use-header-actions";
import { useMachineConnections, useMachines } from "@/features/navigation/use-machines";
import { useNow, zoneAt } from "@/features/navigation/use-now";
import type { MachineId } from "../model/machine-ref";
import {
  MACHINE_MENU_TITLE,
  PAIR_ITEM,
  machineButtonLabel,
  machineMenuItems,
} from "./machine-menu-items";

export interface MachineMenuProps {
  /** The machine of the blank root. */
  readonly current: MachineId;
}

/** The machine button of the blank root and its menu. */
export function MachineMenu(props: MachineMenuProps) {
  const machines = useMachines();
  const connections = useMachineConnections();
  const preferences = usePreferences();
  const actions = useHeaderActions(props.current);
  const now = useNow();
  const items = machineMenuItems({
    machines,
    connections,
    current: props.current,
    now,
    zone: zoneAt(now),
  });
  const name = machines.find((machine) => machine.id === props.current)?.name ?? "";
  return (
    <Stack.Toolbar placement="left">
      <Stack.Toolbar.Menu
        icon="line.3.horizontal.decrease"
        title={MACHINE_MENU_TITLE}
        accessibilityLabel={machineButtonLabel(name)}
      >
        {items.map((item) => (
          <Stack.Toolbar.MenuAction
            key={item.id}
            icon={item.symbol}
            isOn={item.current}
            subtitle={item.subtitle}
            onPress={() => preferences.setLastMachine(item.id)}
          >
            {item.title}
          </Stack.Toolbar.MenuAction>
        ))}
        <Stack.Toolbar.Menu inline title="">
          <Stack.Toolbar.MenuAction icon={PAIR_ITEM.symbol} onPress={() => actions.pairMachine()}>
            {PAIR_ITEM.title}
          </Stack.Toolbar.MenuAction>
        </Stack.Toolbar.Menu>
      </Stack.Toolbar.Menu>
    </Stack.Toolbar>
  );
}
