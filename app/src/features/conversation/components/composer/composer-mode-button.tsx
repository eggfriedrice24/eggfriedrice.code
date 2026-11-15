// The mode control left of the model button (S-ComposerR7 1, 3, 4, 8): the mode of the
// next message from this phone, cautious until the user picks auto in this conversation.
// A tap opens a native pull-down (UIMenu, as the Steer/Queue menu) with Cautious and
// Auto. Full is never in the menu: a phone never runs in full mode.
//
// - Auto shows the robot of the terminal, then the word, in text_primary. Cautious is
//   text_muted with no mark. No pill and no colour of its own.
// - On an iPhone without a passcode, Auto cannot be picked, and its line says why.
// - The mode word never gives way on a full line; the path and ctx go first.

import { useQuery } from "@tanstack/react-query";
import { use, useState } from "react";
import { Pressable, Text } from "react-native";

import { cn } from "@/components/ui/cn";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { SymbolIcon } from "@/components/ui/symbol";
import { passcodeQueryOptions } from "../../conversation-queries";
import type { PhoneMode } from "../../model/model-choices";
import type { ComposerDeps } from "../../use-composer-actions";
import { ComposerContext } from "./composer-context";

/** The robot that marks auto, as in the terminal. */
export const AUTO_MARK = "🤖";

/** The line of each mode in the menu. */
export const MODE_LINES: Readonly<Record<PhoneMode, string>> = {
  cautious: "efr asks you before a call leaves the sandbox.",
  auto: "A reviewer decides each call. Face ID first.",
};

/** The line of Auto on an iPhone without a passcode. */
export const NO_PASSCODE_LINE = "Needs a passcode on this iPhone.";

/** The items of the mode menu, with the check on `mode`. */
export function modeEntries(mode: PhoneMode, passcode: boolean): MenuEntry[] {
  return [
    {
      id: "cautious",
      title: "Cautious",
      subtitle: MODE_LINES.cautious,
      state: mode === "cautious" ? "on" : "off",
    },
    {
      id: "auto",
      title: "Auto",
      subtitle: passcode ? MODE_LINES.auto : NO_PASSCODE_LINE,
      state: mode === "auto" ? "on" : "off",
      disabled: !passcode,
    },
  ];
}

/** The words of the control for VoiceOver. */
export function modeLabel(mode: PhoneMode): string {
  return `Mode: ${mode}. ${MODE_LINES[mode]}`;
}

export interface ComposerModeButtonProps {
  readonly deps: ComposerDeps;
}

/** The mode control of the status line. */
export function ComposerModeButton(props: ComposerModeButtonProps) {
  const model = use(ComposerContext);
  // NOTE: until iOS answers, Auto can be picked; the check at Send still needs a passcode.
  const passcode = useQuery(passcodeQueryOptions(props.deps.hasPasscode)).data !== false;
  const [open, setOpen] = useState(false);
  if (model === null) {
    return null;
  }
  const { mode } = model;
  const auto = mode === "auto";
  const tone = auto ? "text_primary" : "text_muted";
  const pick = (id: string): void => {
    if (id === "cautious" || (id === "auto" && passcode)) {
      model.setMode(id);
    }
  };
  return (
    <MenuView
      testID="composer-mode-menu"
      entries={modeEntries(mode, passcode)}
      onAction={pick}
      onOpenChange={setOpen}
    >
      <Pressable
        testID="composer-mode-button"
        accessibilityRole="button"
        accessibilityLabel={modeLabel(mode)}
        accessibilityHint="Opens the modes."
        accessibilityState={{ expanded: open }}
        hitSlop={{ top: 7, bottom: 7 }}
        className="flex-none flex-row items-center gap-4 active:opacity-60"
      >
        {auto ? (
          <Text accessibilityElementsHidden className="text-mono-xs">
            {AUTO_MARK}
          </Text>
        ) : null}
        <Text
          numberOfLines={1}
          className={cn("text-mono-xs", auto ? "text-text-primary" : "text-text-muted")}
        >
          {mode}
        </Text>
        <SymbolIcon name="chevron.down" size={9} weight="semibold" tone={tone} />
      </Pressable>
    </MenuView>
  );
}
