// One row of the All conversations sheet (board S-Rows, a subtitleCell of a plain list):
// at least 58 pt high, padding 8 18 8 16, gap 12.
//
// - Icon: a column 24 pt wide. The yolk exclamationmark.circle.fill (19 pt) when the
//   conversation waits for the user (an approval, an answer or the terminal), the yolk
//   braille loader when it runs, else a quiet bubble.left (17 pt) in border_subtle.
// - Title: SF Pro 15/20 in text_primary, one line with an ellipsis at the end; text_bright
//   semibold in the Needs you section. The words of a search are text_bright semibold.
// - Second line: SF Pro 13/18 in text_subtle, "[status word] · machine · folder", with
//   the word in yolk ("✗ failed" in state_error) and the folder in SF Mono 12. A row of a
//   machine that did not answer (SH-Offline) has its title in text_muted and
//   "machine · offline · folder".
// - Time: on the right, 13/18 in text_subtle, at the top, for every row.
// - VoiceOver reads the row as one element. A tap opens the conversation. A long press
//   opens the native menu with Open and Copy ID; VoiceOver gets the same two as actions
//   of the row.

import { Pressable, Text, View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { BrailleCell } from "@/components/ui/braille-cell";
import type { MenuItem } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { SymbolIcon, type SymbolName } from "@/components/ui/symbol";
import { TEXT_TONE_CLASS, type TextTone } from "@/components/ui/text-classes";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type { ConversationAddress } from "@/lib/query/keys";
import type { Role } from "@/theme/roles";

import type { SheetRow as SheetRowModel } from "../model/sheet-sections";

/** The ids of the long press menu, and of the VoiceOver actions. */
type RowAction = "open" | "copy-id";

/** The actions of a row (SH-Context), in the order of the menu. */
export const ROW_ACTIONS = [
  { id: "open", title: "Open", image: "bubble.left" },
  { id: "copy-id", title: "Copy ID", image: "doc.on.doc" },
] as const satisfies readonly (MenuItem & { readonly id: RowAction })[];

const ACCESSIBILITY_ACTIONS = ROW_ACTIONS.map((action) => ({
  name: action.id,
  label: action.title,
}));

/** The hint of a row with actions (S-Rows "a11y"). */
const ACTIONS_HINT = "Swipe up or down for actions.";

function isRowAction(id: string): id is RowAction {
  return ROW_ACTIONS.some((action) => action.id === id);
}

/** The colour of the title of `row`. */
function titleTone(row: SheetRowModel): TextTone {
  if (row.bright) {
    return "text_bright";
  }
  return row.offline ? "text_muted" : "text_primary";
}

export interface SheetRowProps {
  readonly row: SheetRowModel;
  readonly onOpen: (ref: ConversationAddress) => void;
}

/** One conversation of the sheet. */
export function SheetRow(props: SheetRowProps) {
  const row = props.row;
  const open = (): void => props.onOpen({ machine: row.machine, id: row.id });
  const act = (id: string): void => {
    if (!isRowAction(id)) {
      return;
    }
    if (id === "open") {
      open();
      return;
    }
    void copyText(row.id);
    haptic("light");
  };
  return (
    <MenuView title={row.title} entries={ROW_ACTIONS} onAction={act} openOnLongPress>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={row.label}
        accessibilityHint={ACTIONS_HINT}
        accessibilityActions={ACCESSIBILITY_ACTIONS}
        onAccessibilityAction={(event) => act(event.nativeEvent.actionName)}
        onPress={open}
        className="min-h-58 flex-row items-center gap-12 py-8 pr-18 pl-16 active:bg-selection-bg"
        testID="sheet-row"
      >
        <RowIcon mark={row.mark} />
        <View className="min-w-0 flex-1">
          <BodyText
            size={row.bright ? "rowStrong" : "row"}
            tone={titleTone(row)}
            numberOfLines={1}
            ellipsizeMode="tail"
          >
            {row.titleRuns.map((run) =>
              run.match ? (
                <Text key={run.start} className="text-row-strong text-text-bright">
                  {run.text}
                </Text>
              ) : (
                run.text
              ),
            )}
          </BodyText>
          <BodyText size="rowDetail" tone="text_subtle" numberOfLines={1} ellipsizeMode="tail">
            {row.status === undefined ? null : (
              <>
                <Text className={TEXT_TONE_CLASS[row.statusTone]}>{row.status}</Text>
                {" · "}
              </>
            )}
            {row.machineName}
            {row.offline ? " · offline" : null}
            {row.folder === undefined ? null : (
              <>
                {" · "}
                <Text className="text-mono-code">{row.folder}</Text>
              </>
            )}
          </BodyText>
        </View>
        <BodyText size="rowDetail" tone="text_subtle" className="mt-1 self-start">
          {row.time}
        </BodyText>
      </Pressable>
    </MenuView>
  );
}

/** The SF Symbol of each still mark: its name, its size in points and its colour role. */
export const ROW_SYMBOLS = {
  approval: { name: "exclamationmark.circle.fill", size: 19, tone: "accent_primary" },
  none: { name: "bubble.left", size: 17, tone: "border_subtle" },
} as const satisfies Record<
  Exclude<SheetRowModel["mark"], "running">,
  { name: SymbolName; size: number; tone: Role }
>;

/** The icon column. The row is one VoiceOver element, so the icon has no label. */
function RowIcon(props: { readonly mark: SheetRowModel["mark"] }) {
  const mark = props.mark;
  return (
    <View testID={`row-icon-${mark}`} className="w-24 items-center">
      {mark === "running" ? (
        <BrailleCell size="button" />
      ) : (
        <SymbolIcon
          name={ROW_SYMBOLS[mark].name}
          size={ROW_SYMBOLS[mark].size}
          tone={ROW_SYMBOLS[mark].tone}
        />
      )}
    </View>
  );
}
