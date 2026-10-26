// One row of the All conversations sheet (design-spec 3.15, a subtitleCell): at least
// 52 pt high, padding 9 20, gap 12.
//
// - Title: the system font 15/20 in text_primary, text_bright in the waiting section,
//   one line with an ellipsis.
// - Subtitle: mono 11/16 in text_subtle, "[status word in yolk] · machine · cwd · time".
// - Leading: an SF Symbol of the status, as a native list has it: the yolk
//   exclamationmark.circle.fill for an approval, the yolk circle.dotted for a running
//   turn (a still glyph, as the old title menu had), and a quiet bubble.left in
//   text_subtle for the others. The two yolk glyphs have their label, and the words of
//   the subtitle say the same.
// - A tap opens the conversation. A long press opens a menu with "Copy title" and "Copy
//   conversation ID"; VoiceOver gets the same two as actions of the row.

import { Pressable, Text, View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { MonoText } from "@/components/ui/mono-text";
import { SymbolIcon, type SymbolName } from "@/components/ui/symbol";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type { ConversationAddress } from "@/lib/query/keys";
import type { Role } from "@/theme/roles";

import { joinParts } from "../model/list-format";
import type { SheetRow as SheetRowModel } from "../model/sheet-sections";

/** The ids of the long press menu, and of the VoiceOver actions. */
type RowAction = "copy-title" | "copy-id";

const ROW_MENU: readonly MenuEntry[] = [
  { id: "copy-title", title: "Copy title", image: "doc.on.doc" },
  { id: "copy-id", title: "Copy conversation ID", image: "number" },
];

const ACCESSIBILITY_ACTIONS = [
  { name: "copy-title", label: "Copy title" },
  { name: "copy-id", label: "Copy conversation ID" },
] as const satisfies readonly { readonly name: RowAction; readonly label: string }[];

function isRowAction(id: string): id is RowAction {
  return id === "copy-title" || id === "copy-id";
}

/** The text that action `action` copies from `row`. */
export function copiedText(row: SheetRowModel, action: RowAction): string {
  return action === "copy-title" ? row.title : row.id;
}

/** The VoiceOver label of a row: its title, then its subtitle. */
export function rowLabel(row: SheetRowModel): string {
  return `${row.title}, ${joinParts([row.status, row.detail])}`;
}

export interface SheetRowProps {
  readonly row: SheetRowModel;
  readonly onOpen: (ref: ConversationAddress) => void;
}

/** One conversation of the sheet. */
export function SheetRow(props: SheetRowProps) {
  const row = props.row;
  const copy = (id: string): void => {
    if (!isRowAction(id)) {
      return;
    }
    void copyText(copiedText(row, id));
    haptic("light");
  };
  return (
    <MenuView title={row.title} entries={ROW_MENU} onAction={copy} openOnLongPress>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={rowLabel(row)}
        accessibilityActions={ACCESSIBILITY_ACTIONS}
        onAccessibilityAction={(event) => copy(event.nativeEvent.actionName)}
        onPress={() => props.onOpen({ machine: row.machine, id: row.id })}
        className="min-h-52 flex-row items-center gap-12 px-gutter py-9 active:bg-selection-bg"
        testID="sheet-row"
      >
        <RowGlyph mark={row.mark} />
        <View className="min-w-0 flex-1">
          <BodyText
            size="row"
            tone={row.bright ? "text_bright" : "text_primary"}
            numberOfLines={1}
            ellipsizeMode="tail"
          >
            {row.title}
          </BodyText>
          <MonoText numberOfLines={1} ellipsizeMode="tail">
            {row.status === undefined ? null : (
              <Text className="text-accent-primary">{row.status}</Text>
            )}
            {row.status === undefined ? row.detail : ` · ${row.detail}`}
          </MonoText>
        </View>
      </Pressable>
    </MenuView>
  );
}

/** The glyph of each mark: its SF Symbol, its colour role and its VoiceOver label. */
export const ROW_GLYPHS = {
  approval: {
    name: "exclamationmark.circle.fill",
    tone: "accent_primary",
    label: "Needs approval",
  },
  running: { name: "circle.dotted", tone: "accent_primary", label: "Running" },
  none: { name: "bubble.left", tone: "text_subtle", label: undefined },
} as const satisfies Record<
  SheetRowModel["mark"],
  { name: SymbolName; tone: Role; label: string | undefined }
>;

function RowGlyph(props: { readonly mark: SheetRowModel["mark"] }) {
  const glyph = ROW_GLYPHS[props.mark];
  return (
    <View testID={`row-glyph-${props.mark}`} className="w-20 items-center">
      {glyph.label === undefined ? (
        <SymbolIcon name={glyph.name} size={17} tone={glyph.tone} />
      ) : (
        <SymbolIcon name={glyph.name} size={17} tone={glyph.tone} label={glyph.label} />
      )}
    </View>
  );
}
