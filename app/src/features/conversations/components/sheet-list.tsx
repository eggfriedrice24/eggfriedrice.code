// The list of the All conversations sheet (boards S-Rows and SH-Large, a plain list): no
// cell backgrounds, plain section headers that stick, and 0.5 pt separators 52 pt in,
// between the rows of a section only. The sheet holds at most a few hundred rows (the
// recent lists of the paired machines), so the SectionList of React Native is enough; its
// sticky headers are native on iOS.
//
// The banner of the offline machines (SH-Offline) is the header of the list, above the
// first section: the SF Symbol wifi.slash in text_subtle and one line in text_muted, SF
// Pro 13, with no box. The quiet line of the pane (loading, searching, empty) comes
// after the rows.

import { SectionList, View, type SectionListData } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { BrailleCell } from "@/components/ui/braille-cell";
import { Hairline } from "@/components/ui/hairline";
import { MonoText } from "@/components/ui/mono-text";
import { SectionHeader } from "@/components/ui/section-header";
import { SymbolIcon } from "@/components/ui/symbol";
import type { ConversationAddress } from "@/lib/query/keys";

import type { SheetRow as SheetRowModel } from "../model/sheet-sections";
import type { OfflineBanner, SheetNotice } from "../model/sheet-view";
import { SheetRow } from "./sheet-row";

/** One section of a pane: the sections of the sheet, or the results of the search. */
export interface SheetListSection {
  /** A key that stays the same while the section is on screen, such as `today`. */
  readonly key: string;
  /** The header text, such as `Needs you` or `3 results`. */
  readonly title: string;
  readonly rows: readonly SheetRowModel[];
}

export interface SheetListProps {
  readonly sections: readonly SheetListSection[];
  readonly notice: SheetNotice;
  /** The banner of the offline machines; none while every machine answers. */
  readonly banner: OfflineBanner | undefined;
  readonly onOpen: (ref: ConversationAddress) => void;
  readonly testID?: string;
}

interface ListSection {
  readonly key: string;
  readonly title: string;
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
      }) => <SectionHeader title={section.title} />}
      ItemSeparatorComponent={RowSeparator}
      ListHeaderComponent={
        props.banner === undefined ? undefined : <OfflineHeader banner={props.banner} />
      }
      ListFooterComponent={<SheetFooter notice={props.notice} />}
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
  return <Hairline inset="row" />;
}

/** The banner of the offline machines, one VoiceOver element. */
function OfflineHeader(props: { readonly banner: OfflineBanner }) {
  return (
    <View
      className="min-h-30 flex-row items-center gap-8 px-gutter pt-6 pb-2"
      accessible
      accessibilityRole="text"
      accessibilityLabel={props.banner.label}
      testID="sheet-offline"
    >
      <SymbolIcon name="wifi.slash" size={15} tone="text_subtle" />
      <BodyText size="rowDetail" tone="text_muted" numberOfLines={1} ellipsizeMode="tail">
        {props.banner.text}
      </BodyText>
    </View>
  );
}

/** The quiet line under the rows, in mono 11. */
function SheetFooter(props: { readonly notice: SheetNotice }) {
  const notice = props.notice;
  if (notice.kind === "none") {
    return null;
  }
  return (
    <View className="px-gutter pt-14 pb-gutter">
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
    </View>
  );
}
