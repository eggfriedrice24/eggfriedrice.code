// The body of the conversations sheet (boards SH-Large and SH-Search): two panes under
// the native search bar of the screen. Every paired machine is in it. "Needs you" and
// "Running" lead, then Today, Yesterday, This week and Older, newest first. While the
// field has text, one "3 results" section takes their place. The search bar and the
// compose button ("New conversation") are native parts of the screen, in the bottom
// toolbar of iOS 26 (the hard rule of app/AGENTS.md, as the thread list of t3code), so
// this view draws neither.
//
// - Sections pane: the empty search of each machine (`sheetQueriesOptions` with ""),
//   which the persister keeps, so the sheet shows rows before the machines connect.
// - Results pane, while the field has text: the loaded rows that match the text at
//   once (`useDeferredValue`, so a key press never waits for the filter), and the search
//   of efrd on the text debounced 200 ms with Pacer.
// - The two panes are in `Activity`: the hidden one keeps its scroll position and its
//   rows, so a cleared field shows the sections where they were.
// - A machine whose empty search failed did not answer: both panes show its rows as
//   offline, under one banner that tells the age of the kept rows (SH-Offline). Its next
//   `ready` asks again, and then they are live.

import { useQueries } from "@tanstack/react-query";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { Activity, useDeferredValue } from "react";
import { View } from "react-native";

import type { ConversationAddress } from "@/lib/query/keys";
import type { MachineCaller } from "@/lib/query/machine-caller";

import { sheetQueriesOptions } from "../conversations-queries";
import { mergeConversationLists, sheetSections, type RowContext } from "../model/sheet-sections";
import {
  SEARCH_DEBOUNCE_MS,
  machineNameOf,
  offlineBanner,
  offlineMachines,
  resultsNotice,
  resultsTitle,
  searchPaneRows,
  sectionsNotice,
  sheetPane,
  type SheetMachine,
} from "../model/sheet-view";
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
  /** The text of the native search bar of the screen. */
  readonly query: string;
}

/** The two panes of the sheet. */
export function ConversationsSheet(props: ConversationsSheetProps) {
  const text = props.query;
  const typed = useDeferredValue(text);
  const [searched] = useDebouncedValue(text, { wait: SEARCH_DEBOUNCE_MS });
  const ids = props.machines.map((machine) => machine.id);
  const searching = searched.trim().length > 0;
  const loaded = useQueries(sheetQueriesOptions(props.caller, ids, ""));
  const found = useQueries(sheetQueriesOptions(props.caller, searching ? ids : [], searched));

  const context: RowContext = {
    machineName: machineNameOf(props.machines),
    now: props.now,
    offline: offlineMachines(ids, loaded.failed),
  };
  const banner = offlineBanner({
    machines: props.machines,
    failed: loaded.failed,
    updatedAt: loaded.updatedAt,
    now: props.now,
  });
  const sections: SheetListSection[] = sheetSections(
    mergeConversationLists(loaded.lists),
    context,
  ).map((section) => ({ key: section.kind, title: section.title, rows: section.rows }));
  const sectionRows = sections.reduce((count, section) => count + section.rows.length, 0);

  const pane = sheetPane(typed);
  const results =
    pane === "results"
      ? searchPaneRows({
          loaded: loaded.lists,
          found: found.lists,
          text: typed,
          searched,
          context,
        })
      : [];
  const resultSections: SheetListSection[] =
    results.length === 0
      ? []
      : [{ key: "results", title: resultsTitle(results.length), rows: results }];

  return (
    // The views keep their place (`collapsable={false}`), so a list is never a direct
    // child of the formSheet. react-native-screens gives such a scroll view the size of
    // the sheet by force, and then it can cover the views beside it.
    <View collapsable={false} className="flex-1 bg-surface-1">
      <View collapsable={false} className="flex-1">
        <Activity mode={pane === "sections" ? "visible" : "hidden"}>
          <SheetList
            sections={sections}
            notice={sectionsNotice({
              machines: props.machines.length,
              pending: loaded.isPending,
              rows: sectionRows,
            })}
            banner={banner}
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
            banner={banner}
            onOpen={props.onOpen}
            testID="sheet-results"
          />
        </Activity>
      </View>
    </View>
  );
}
