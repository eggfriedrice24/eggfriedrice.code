// The All conversations sheet (plan 2.5, design-spec 3.15, board R-AllSheet). The root
// stack presents it as a formSheet with the detents [0.5, 1] and the grabber; it opens
// at the medium detent.
//
// - Header: the native title "All conversations" in the system size (design item 8.4),
//   on surface_1 with no hairline, and the close button (UIBarButtonItem, xmark) on the
//   right. A swipe down also dismisses the sheet.
// - A pick dismisses the sheet, then opens the conversation: a push, or a pop back to it
//   when it is in the stack. A conversation of another machine carries its machine.

import { useSelector } from "@tanstack/react-store";
import { Stack, useRouter, type NativeStackNavigationOptions } from "expo-router";
import { useState } from "react";

import type { ConversationAddress } from "@/lib/query/keys";
import { roles } from "@/theme/roles";

import { ConversationsSheet } from "../components/conversations-sheet";
import { sheetMachines } from "../model/sheet-view";
import { useSheetDeps } from "./use-sheet-deps";

const SHEET_HEADER: NativeStackNavigationOptions = {
  headerTransparent: false,
  headerStyle: { backgroundColor: roles.surface_1 },
  headerShadowVisible: false,
};

export interface ConversationsScreenProps {
  /** The machine on screen when the sheet opened; its search hits come first. */
  readonly machine?: string;
}

/** The All conversations sheet. */
export function ConversationsScreen(props: ConversationsScreenProps) {
  const deps = useSheetDeps();
  const router = useRouter();
  const [now] = useState(deps.now);
  const registered = useSelector(deps.registry, (state) => state.machines);
  const machines = sheetMachines(
    registered.map((machine) => ({ id: machine.id, name: machine.name })),
    props.machine,
  );
  const open = (ref: ConversationAddress): void => {
    router.back();
    deps.open(ref);
  };
  return (
    <>
      <Stack.Screen options={SHEET_HEADER} />
      <Stack.Toolbar placement="right">
        <Stack.Toolbar.Button
          icon="xmark"
          accessibilityLabel="Close"
          onPress={() => router.back()}
        />
      </Stack.Toolbar>
      <ConversationsSheet caller={deps.caller} machines={machines} now={now} onOpen={open} />
    </>
  );
}
