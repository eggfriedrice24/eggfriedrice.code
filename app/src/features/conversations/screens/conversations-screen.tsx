// The conversations sheet (plan 2.5, design-spec 3.15, board R-AllSheet), the one place
// to switch conversations: the title of the bar opens it. The root stack presents it as
// a formSheet with the detents [0.5, 1] and the grabber, and gives it an opaque bar
// (CONVERSATIONS_OPTIONS); it opens at the medium detent.
//
// - Header: the native title "All conversations" in the system size (design item 8.4),
//   on surface_1 with no hairline, and the close button (UIBarButtonItem, xmark) on the
//   right. A swipe down also dismisses the sheet.
// - The native search bar and the compose button (square.and.pencil, "New conversation",
//   back to the blank root) stand in the bottom toolbar of iOS 26, as the thread list of
//   t3code (the hard rule of app/AGENTS.md: native parts over JS copies).
// - A pick dismisses the sheet, then opens the conversation: a push, or a pop back to it
//   when it is in the stack. A conversation of another machine carries its machine. The
//   history helper does both, because it plans over the stack with the sheet on top.

import { useSelector } from "@tanstack/react-store";
import { Stack, useRouter } from "expo-router";
import { useState } from "react";

import { SHEET_SEARCH_BAR } from "@/components/ui/native/sheet-search-bar";
import type { ConversationAddress } from "@/lib/query/keys";

import { ConversationsSheet } from "../components/conversations-sheet";
import { sheetMachines } from "../model/sheet-view";
import { useSheetDeps } from "./use-sheet-deps";

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
  const open = (ref: ConversationAddress): void => deps.open(ref);
  const [query, setQuery] = useState("");
  return (
    <>
      <Stack.SearchBar
        {...SHEET_SEARCH_BAR}
        placeholder="Search"
        onChangeText={(event) => setQuery(event.nativeEvent.text)}
        onCancelButtonPress={() => setQuery("")}
      />
      <Stack.Toolbar placement="bottom">
        <Stack.Toolbar.SearchBarSlot />
        <Stack.Toolbar.Button
          icon="square.and.pencil"
          accessibilityLabel="New conversation"
          separateBackground
          onPress={deps.newConversation}
        />
      </Stack.Toolbar>
      <Stack.Toolbar placement="right">
        <Stack.Toolbar.Button
          icon="xmark"
          accessibilityLabel="Close"
          onPress={() => router.back()}
        />
      </Stack.Toolbar>
      <ConversationsSheet
        caller={deps.caller}
        machines={machines}
        now={now}
        onOpen={open}
        query={query}
      />
    </>
  );
}
