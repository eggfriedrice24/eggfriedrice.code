// A machine row of Settings (design 3.16): three mono lines and an ellipsis button that
// opens the row menu (Copy node name, Re-pair, Forget this machine).
//
// The ellipsis is drawn 32 pt with a 44 pt hit area. While its menu is open, its fill is
// selection_bg and its icon text_bright. The row gives the button's view with each
// action, so the Forget sheet can come out of it on iOS 26.

import { useRef, useState } from "react";
import { Text, View, type HostInstance } from "react-native";

import { cn } from "@/components/ui/cn";
import { InsetRow } from "@/components/ui/inset-group";
import { MenuView } from "@/components/ui/menu-view";
import { MonoText } from "@/components/ui/mono-text";
import { SymbolIcon } from "@/components/ui/symbol";
import { TEXT_TONE_CLASS } from "@/components/ui/text-classes";
import {
  MACHINE_MENU,
  machineAction,
  type MachineAction,
  type MachineRowText,
} from "../model/settings-format";

export interface MachineRowProps {
  readonly text: MachineRowText;
  /** Called with the picked action and the view of the ellipsis button. */
  readonly onAction: (action: MachineAction, anchor: HostInstance | null) => void;
}

/** One paired machine. */
export function MachineRow(props: MachineRowProps) {
  const { text } = props;
  const [open, setOpen] = useState(false);
  const anchor = useRef<HostInstance>(null);
  const node = text.node;
  const version = text.version;
  return (
    <InsetRow
      accessory={
        <MenuView
          entries={MACHINE_MENU}
          onOpenChange={setOpen}
          onAction={(id) => {
            const action = machineAction(id);
            if (action !== undefined) {
              props.onAction(action, anchor.current);
            }
          }}
          testID={`machine-menu-${text.name}`}
        >
          <View
            ref={anchor}
            accessible
            accessibilityRole="button"
            accessibilityLabel={text.actionsLabel}
            accessibilityState={{ expanded: open }}
            className="w-hit h-hit items-center justify-center"
          >
            <View
              className={cn(
                "w-32 h-32 items-center justify-center rounded-full",
                open && "bg-selection-bg",
              )}
            >
              <SymbolIcon name="ellipsis" size={18} tone={open ? "text_bright" : "text_muted"} />
            </View>
          </View>
        </MenuView>
      }
    >
      <View className="min-w-0 flex-1">
        <MonoText size="monoName" tone={text.dim ? "text_muted" : "text_bright"} numberOfLines={1}>
          {text.name}
        </MonoText>
        <MonoText numberOfLines={1}>
          <Text className={TEXT_TONE_CLASS[text.stateTone]}>{text.state}</Text>
          {node === undefined ? null : ` · ${node}`}
        </MonoText>
        {version === undefined ? null : <MonoText numberOfLines={1}>{version}</MonoText>}
      </View>
    </InsetRow>
  );
}
