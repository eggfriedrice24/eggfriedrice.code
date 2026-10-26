// The list of the All conversations sheet (design-spec 3.15, a plain list): no cell
// backgrounds, sticky mono section headers, and 0.5 pt separators 20 pt in. The sheet
// holds at most a few hundred rows (the recent lists of the paired machines), so the
// SectionList of React Native is enough; its sticky headers are native on iOS.
//
// The quiet line of the pane (loading, searching, empty) and the line of the machines
// that did not answer come after the rows.

import { SectionList, View, type SectionListData } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { Hairline } from "@/components/ui/hairline";
import { MonoText } from "@/components/ui/mono-text";
import { SectionHeader } from "@/components/ui/section-header";
import type { ConversationAddress } from "@/lib/query/keys";

import type { SheetRow as SheetRowModel } from "../model/sheet-sections";
import type { SheetNotice } from "../model/sheet-view";
import { SheetRow } from "./sheet-row";

/** One section of a pane: the sections of the sheet, or the results of the search. */
export interface SheetListSection {
  /** A key that stays the same while the section is on screen, such as `today`. */
  readonly key: string;
  /** The header text, such as `waiting · 2` or `results · 3`. */
  readonly title: string;
  /** True for the waiting section: the header is yolk. */
  readonly waiting: boolean;
  readonly rows: readonly SheetRowModel[];
}

export interface SheetListProps {
  readonly sections: readonly SheetListSection[];
  readonly notice: SheetNotice;
  /** The line of the machines that did not answer, such as `1 machine did not answer`. */
  readonly failed: string | undefined;
  readonly onOpen: (ref: ConversationAddress) => void;
  readonly testID?: string;
}

interface ListSection {
  readonly key: string;
  readonly title: string;
  readonly waiting: boolean;
  readonly data: readonly SheetRowModel[];
}

function rowKey(row: SheetRowModel): string {
  return `${row.machine}:${row.id}`;
}

/** The list of one pane of the sheet. */
export function SheetList(props: SheetListProps) {
  const sections: ListSection[] = props.sections.map((section) => ({
    key: section.key,
    title: section.title,
    waiting: section.waiting,
    data: section.rows,
  }));
  const onOpen = props.onOpen;
  return (
    <SectionList<SheetRowModel, ListSection>
      sections={sections}
      keyExtractor={rowKey}
      renderItem={({ item }) => <SheetRow row={item} onOpen={onOpen} />}
      renderSectionHeader={({
        section,
      }: {
        section: SectionListData<SheetRowModel, ListSection>;
      }) => <SectionHeader title={section.title} waiting={section.waiting} />}
      ItemSeparatorComponent={RowSeparator}
      ListFooterComponent={<SheetFooter notice={props.notice} failed={props.failed} />}
      stickySectionHeadersEnabled
      keyboardDismissMode="on-drag"
      keyboardShouldPersistTaps="handled"
      contentInsetAdjustmentBehavior="automatic"
      initialNumToRender={20}
      className="flex-1"
      testID={props.testID}
    />
  );
}

function RowSeparator() {
  return <Hairline inset="gutter" />;
}

interface SheetFooterProps {
  readonly notice: SheetNotice;
  readonly failed: string | undefined;
}

/** The quiet lines under the rows, in mono 11. */
function SheetFooter(props: SheetFooterProps) {
  const notice = props.notice;
  if (notice.kind === "none" && props.failed === undefined) {
    return null;
  }
  return (
    <View className="gap-6 px-gutter pt-14 pb-gutter">
      {notice.kind === "none" ? null : (
        <View
          className="flex-row items-center gap-8"
          accessible
          accessibilityRole={notice.kind === "empty" ? "text" : "progressbar"}
          accessibilityLabel={notice.text}
          accessibilityLiveRegion="polite"
          testID="sheet-notice"
        >
          {notice.kind === "empty" ? null : <BrailleCell size="row" tint="wait" />}
          <MonoText tone={notice.kind === "empty" ? "text_subtle" : "text_muted"}>
            {notice.text}
          </MonoText>
        </View>
      )}
      {props.failed === undefined ? null : (
        <MonoText testID="sheet-failed">{props.failed}</MonoText>
      )}
    </View>
  );
}
