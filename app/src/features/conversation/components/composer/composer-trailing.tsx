// Slot D of the composer, the trailing action (design 3.3): a 32 pt circle with a 16 pt
// glyph, in the middle of the 40 pt field row, with a 44 pt hit area.
//
// - Send: a yolk circle with arrow.up in text_on_accent; disabled, a 1 pt border_subtle
//   circle with the glyph in text_decorative.
// - Stop: a 1 pt border_default circle with stop.fill in text_bright.
// - The Steer capsule (yolk fill, text_on_accent) or the Queue capsule (1 pt yolk border,
//   yolk text), with chevron.down, after Stop while there is text. A tap sends in the
//   mode of the capsule. A long press opens the native menu ("Steer now", "Queue"); a
//   pick sends in that mode, and the choice sticks.
// - Sending: the yolk circle with the braille cell in text_on_accent.

import { use, useState } from "react";
import { Pressable, Text, View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { cn } from "@/components/ui/cn";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { SymbolIcon } from "@/components/ui/symbol";
import type { SendMode } from "@/features/settings/model/preferences";
import { SPACING } from "@/theme/metrics";

import { capsuleLabel, SEND_MODE_CHOICES } from "../../model/composer-state";
import { ComposerContext } from "./composer-context";

/** The hit slop that takes a 32 pt action to the 44 pt hit area. */
const SLOP = (SPACING.hit - SPACING.action) / 2;

const CIRCLE = "w-action h-action my-4 items-center justify-center rounded-full";

/** The trailing action of the composer. */
export function ComposerTrailing() {
  const model = use(ComposerContext);
  if (model === null) {
    return null;
  }
  const trailing = model.trailing;
  switch (trailing.kind) {
    case "send":
      return <SendButton enabled={trailing.enabled} onPress={model.submit} />;
    case "stop":
      return (
        <>
          <StopButton onPress={model.stop} />
          {trailing.capsule === null ? null : (
            <ModeCapsule mode={trailing.capsule} onPress={model.submit} onPick={model.pick} />
          )}
        </>
      );
    default:
      return (
        <>
          {trailing.stop ? <StopButton onPress={model.stop} /> : null}
          <SendingButton />
        </>
      );
  }
}

interface SendButtonProps {
  readonly enabled: boolean;
  readonly onPress: () => void;
}

function SendButton(props: SendButtonProps) {
  return (
    <Pressable
      testID="composer-send"
      accessibilityRole="button"
      accessibilityLabel="Send"
      accessibilityState={{ disabled: !props.enabled }}
      disabled={!props.enabled}
      hitSlop={SLOP}
      onPress={props.onPress}
      className={cn(
        CIRCLE,
        props.enabled ? "bg-accent-primary active:opacity-80" : "border border-border-subtle",
      )}
    >
      <SymbolIcon
        name="arrow.up"
        size={16}
        weight="semibold"
        tone={props.enabled ? "text_on_accent" : "text_decorative"}
      />
    </Pressable>
  );
}

function SendingButton() {
  return (
    <View
      testID="composer-sending"
      accessible
      accessibilityRole="button"
      accessibilityLabel="Sending"
      accessibilityState={{ disabled: true, busy: true }}
      className={cn(CIRCLE, "bg-accent-primary")}
    >
      <BrailleCell size="row" tint="on_accent" />
    </View>
  );
}

interface StopButtonProps {
  readonly onPress: () => void;
}

function StopButton(props: StopButtonProps) {
  return (
    <Pressable
      testID="composer-stop"
      accessibilityRole="button"
      accessibilityLabel="Stop the turn"
      hitSlop={SLOP}
      onPress={props.onPress}
      className={cn(CIRCLE, "border border-border-default active:opacity-60")}
    >
      <SymbolIcon name="stop.fill" size={13} tone="text_bright" />
    </Pressable>
  );
}

const CAPSULE = {
  steer: { frame: "bg-accent-primary", text: "text-text-on-accent", tone: "text_on_accent" },
  queue: {
    frame: "border border-accent-primary",
    text: "text-accent-primary",
    tone: "accent_primary",
  },
} as const satisfies Record<
  SendMode,
  { frame: string; text: string; tone: "text_on_accent" | "accent_primary" }
>;

/** The items of the Steer/Queue menu, with the check on `mode`. */
export function sendModeEntries(mode: SendMode): MenuEntry[] {
  return SEND_MODE_CHOICES.map((choice) => ({
    id: choice.mode,
    title: choice.title,
    subtitle: choice.subtitle,
    image: choice.image,
    state: choice.mode === mode ? "on" : "off",
  }));
}

function isSendMode(id: string): id is SendMode {
  return id === "steer" || id === "queue";
}

interface ModeCapsuleProps {
  readonly mode: SendMode;
  readonly onPress: () => void;
  readonly onPick: (mode: SendMode) => void;
}

function ModeCapsule(props: ModeCapsuleProps) {
  const [open, setOpen] = useState(false);
  const look = CAPSULE[props.mode];
  const label = capsuleLabel(props.mode);
  const pick = (id: string): void => {
    if (isSendMode(id)) {
      props.onPick(id);
    }
  };
  return (
    <MenuView
      testID="composer-send-mode"
      entries={sendModeEntries(props.mode)}
      onAction={pick}
      onOpenChange={setOpen}
      openOnLongPress
    >
      <Pressable
        testID="composer-capsule"
        accessibilityRole="button"
        accessibilityLabel={`${label}, more options`}
        accessibilityHint="Touch and hold to pick how it is sent."
        accessibilityState={{ expanded: open }}
        hitSlop={{ top: SLOP, bottom: SLOP }}
        onPress={props.onPress}
        className={cn(
          "h-action my-4 flex-row items-center gap-6 rounded-full pr-12 pl-14 active:opacity-80",
          look.frame,
        )}
      >
        <Text numberOfLines={1} className={cn("text-capsule", look.text)}>
          {label}
        </Text>
        <SymbolIcon
          name={open ? "chevron.up" : "chevron.down"}
          size={10}
          weight="heavy"
          tone={look.tone}
        />
      </Pressable>
    </MenuView>
  );
}
