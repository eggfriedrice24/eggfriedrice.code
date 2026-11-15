// The mode control left of the model button (S-ComposerR7 1, 3, 4, 8, and round 9 frames
// 12a, 13, 14a): the mode of the next message from this phone, cautious until the user
// picks auto in this conversation. A tap opens a native pull-down (UIMenu, as the
// Steer/Queue menu) with Cautious and Auto. Full is never in the menu: a phone never runs
// in full mode.
//
// - Auto shows the robot of the terminal, then the word, in text_primary. Cautious is
//   text_muted with no mark. No pill and no colour of its own.
// - Auto from the phone needs the Face ID unlock of the app. While it is off, Auto cannot
//   be picked, its line says what to do, and a third item under a divider, "Turn on Face
//   ID unlock", opens Settings. That item does not turn the unlock on: the switch does,
//   with its check. With the unlock off the control shows cautious, also for a
//   conversation that was in auto.
// - The mode word never gives way on a full line; the path and ctx go first.

import { useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import { useRouter } from "expo-router";
import { use, useState } from "react";
import { Pressable, Text } from "react-native";

import { cn } from "@/components/ui/cn";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { SymbolIcon } from "@/components/ui/symbol";
import { DEFAULT_FACTS, type OwnerMethod } from "@/lib/lock/owner-check";
import { selectLockOn } from "@/features/navigation/model/app-lock";
import { autoNeedsLine, turnOnTitle } from "@/features/navigation/model/owner-check-format";
import { SETTINGS_UNLOCK_HREF } from "@/features/navigation/model/routes";
import { ownerFactsQueryOptions } from "@/features/navigation/navigation-queries";
import type { PhoneMode } from "../../model/model-choices";
import type { ComposerDeps } from "../../use-composer-actions";
import { ComposerContext } from "./composer-context";

/** The robot that marks auto, as in the terminal. */
export const AUTO_MARK = "🤖";

/** The line of each mode in the menu. */
export const MODE_LINES: Readonly<Record<PhoneMode, string>> = {
  cautious: "efr asks you before a call leaves the sandbox.",
  auto: "A reviewer decides each call.",
};

/** The id of the third item, which opens Settings. */
const TURN_ON = "turn_on_unlock";

/** The state of the Face ID unlock that the menu needs. */
export interface MenuLock {
  readonly on: boolean;
  readonly method: OwnerMethod;
}

/** The items of the mode menu, with the check on `mode`. */
export function modeEntries(mode: PhoneMode, lock: MenuLock): MenuEntry[] {
  const entries: MenuEntry[] = [
    {
      id: "cautious",
      title: "Cautious",
      subtitle: MODE_LINES.cautious,
      state: mode === "cautious" ? "on" : "off",
    },
    {
      id: "auto",
      title: "Auto",
      subtitle: lock.on ? MODE_LINES.auto : autoNeedsLine(lock.method),
      state: mode === "auto" ? "on" : "off",
      disabled: !lock.on,
    },
  ];
  if (!lock.on) {
    const image = lock.method === "the passcode" ? "lock" : "faceid";
    entries.push({
      section: "unlock",
      items: [{ id: TURN_ON, title: turnOnTitle(lock.method), image }],
    });
  }
  return entries;
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
  const router = useRouter();
  const lockOn = useSelector(props.deps.lock.store, selectLockOn);
  const method =
    useQuery(ownerFactsQueryOptions(props.deps.lock)).data?.method ?? DEFAULT_FACTS.method;
  const [open, setOpen] = useState(false);
  if (model === null) {
    return null;
  }
  const { mode } = model;
  const auto = mode === "auto";
  const tone = auto ? "text_primary" : "text_muted";
  const pick = (id: string): void => {
    if (id === TURN_ON) {
      router.push(SETTINGS_UNLOCK_HREF);
    } else if (id === "cautious" || (id === "auto" && lockOn)) {
      model.setMode(id);
    }
  };
  return (
    <MenuView
      testID="composer-mode-menu"
      entries={modeEntries(mode, { on: lockOn, method })}
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
