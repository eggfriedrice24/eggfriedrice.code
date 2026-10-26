// The body of the conversations sheet (design-spec 3.15, board R-AllSheet): the "New
// conversation" row, then the search field over two panes. Every paired machine is in it; the waiting conversations
// are in their own section on top, then today, this week and older, newest first.
//
// - Sections pane: the empty search of each machine (`sheetQueriesOptions` with ""),
//   which the persister keeps, so the sheet shows rows before the machines connect.
// - Results pane, while the field has text: the loaded rows that match the text at
//   once (`useDeferredValue`, so a key press never waits for the filter), and the search
//   of efrd on the text debounced 200 ms with Pacer.
// - The two panes are in `Activity`: the hidden one keeps its scroll position and its
//   rows, so a cleared field shows the sections where they were.

import { useQueries } from "@tanstack/react-query";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { Activity, useDeferredValue, useState } from "react";
import { Pressable, View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { SymbolIcon } from "@/components/ui/symbol";

import type { ConversationAddress } from "@/lib/query/keys";
import type { MachineCaller } from "@/lib/query/machine-caller";

import { sheetQueriesOptions } from "../conversations-queries";
import { mergeConversationLists, sheetSections } from "../model/sheet-sections";
import {
  SEARCH_DEBOUNCE_MS,
  failedText,
  machineNameOf,
  resultsNotice,
  resultsTitle,
  searchPaneRows,
  sectionsNotice,
  sheetPane,
  type SheetMachine,
} from "../model/sheet-view";
import { SearchField } from "./search-field";
import { SheetList, type SheetListSection } from "./sheet-list";

export interface ConversationsSheetProps {
  /** Asks the machines for their lists. */
  readonly caller: MachineCaller;
  /** The paired machines, the machine on screen first. */
  readonly machines: readonly SheetMachine[];
  /** The time of the list times, in ms since the epoch. */
  readonly now: number;
  /** Opens a picked conversation. */
  readonly onOpen: (ref: ConversationAddress) => void;
  /** Goes back to the blank root for a new conversation. */
  readonly onNew: () => void;
}

/** The search field and the two panes of the sheet. */
export function ConversationsSheet(props: ConversationsSheetProps) {
  const [text, setText] = useState("");
  const typed = useDeferredValue(text);
  const [searched] = useDebouncedValue(text, { wait: SEARCH_DEBOUNCE_MS });
  const ids = props.machines.map((machine) => machine.id);
  const searching = searched.trim().length > 0;
  const loaded = useQueries(sheetQueriesOptions(props.caller, ids, ""));
  const found = useQueries(sheetQueriesOptions(props.caller, searching ? ids : [], searched));

  const machineName = machineNameOf(props.machines);
  const sections: SheetListSection[] = sheetSections(
    mergeConversationLists(loaded.lists),
    machineName,
    props.now,
  ).map((section) => ({
    key: section.kind,
    title: section.title,
    waiting: section.kind === "waiting",
    rows: section.rows,
  }));
  const sectionRows = sections.reduce((count, section) => count + section.rows.length, 0);

  const pane = sheetPane(typed);
  const results =
    pane === "results"
      ? searchPaneRows({
          loaded: loaded.lists,
          found: found.lists,
          text: typed,
          searched,
          machineName,
          now: props.now,
        })
      : [];
  const resultSections: SheetListSection[] =
    results.length === 0
      ? []
      : [{ key: "results", title: resultsTitle(results.length), waiting: false, rows: results }];

  return (
    // The views keep their place (`collapsable={false}`), so a list is never a direct
    // child of the formSheet. react-native-screens gives such a scroll view the size of
    // the sheet by force, and then it can cover the views beside it.
    <View collapsable={false} className="flex-1 bg-surface-1">
      <NewConversationRow onPress={props.onNew} />
      <SearchField value={text} onChangeText={setText} />
      <View collapsable={false} className="flex-1">
        <Activity mode={pane === "sections" ? "visible" : "hidden"}>
          <SheetList
            sections={sections}
            notice={sectionsNotice({
              machines: props.machines.length,
              pending: loaded.isPending,
              rows: sectionRows,
            })}
            failed={failedText(loaded.failed)}
            onOpen={props.onOpen}
            testID="sheet-sections"
          />
        </Activity>
        <Activity mode={pane === "results" ? "visible" : "hidden"}>
          <SheetList
            sections={resultSections}
            notice={resultsNotice({
              rows: results.length,
              fetching: found.isFetching,
              waiting: searched.trim() !== typed.trim(),
            })}
            failed={failedText(found.failed)}
            onOpen={props.onOpen}
            testID="sheet-results"
          />
        </Activity>
      </View>
    </View>
  );
}

interface NewConversationRowProps {
  readonly onPress: () => void;
}

/** The first row of the sheet: a new conversation on the blank root. */
function NewConversationRow(props: NewConversationRowProps) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel="New conversation"
      onPress={props.onPress}
      className="mb-8 min-h-52 flex-row items-center gap-12 px-gutter active:bg-selection-bg"
      testID="sheet-new"
    >
      <View className="w-20 items-center">
        <SymbolIcon name="square.and.pencil" size={17} tone="accent_primary" />
      </View>
      <BodyText size="row" tone="text_bright">
        New conversation
      </BodyText>
    </Pressable>
  );
}
