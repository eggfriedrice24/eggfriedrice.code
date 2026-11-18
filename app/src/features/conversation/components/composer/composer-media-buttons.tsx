// The two media buttons of the composer, in both sizes of the pill (the user,
// 2026-11-17, as t3code and Claude Code):
//
// - Attach (plus) in slot B, at the bottom left (board S-Attach): a tap opens the native
//   menu Photo Library, Take Photo, Choose File. It shows only when the build has the
//   pickers and efrd takes attachments; else nothing stands in its place. At the limit
//   of efrd it is off.
// - Voice (mic) before the trailing action (board S-Voice): a tap records, and the
//   words go into the draft. It shows only when the build has efr-speech and the iPhone
//   turns speech into text in its language; else nothing stands in its place and the
//   dictation key of the keyboard stays. While it records, it is a stop button in
//   state_error with the time before it; while it prepares or ends, the braille cell.
//
// Each is a 32 pt circle with no fill and a 44 pt hit area, as the other actions, with
// its glyph in text_muted, or in text_decorative while it is off.

import { use } from "react";
import { Pressable, Text, View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { cn } from "@/components/ui/cn";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { SymbolIcon } from "@/components/ui/symbol";
import { SPACING } from "@/theme/metrics";

import type { AttachSource } from "../../model/attachments";
import { clock } from "../../model/format";
import { MODEL_LINE, stopLabel } from "../../model/voice";
import { useNow } from "../use-now";
import { ComposerContext, ComposerCursorContext } from "./composer-context";

/** The hit slop that takes a 32 pt action to the 44 pt hit area. */
const SLOP = (SPACING.hit - SPACING.action) / 2;

const CIRCLE = "w-action h-action my-4 items-center justify-center rounded-full";

/** The VoiceOver label of the microphone. */
const VOICE_LABEL = "Record a voice message";

/** The items of the plus menu, in the order of the board. */
export const ATTACH_MENU: readonly (MenuEntry & { readonly id: AttachSource })[] = [
  { id: "photos", title: "Photo Library", image: "photo.on.rectangle" },
  { id: "camera", title: "Take Photo", image: "camera" },
  { id: "files", title: "Choose File", image: "folder" },
];

function isSource(id: string): id is AttachSource {
  return id === "photos" || id === "camera" || id === "files";
}

/** Slot B: attach a photo or a file, or nothing when the composer takes none. */
export function ComposerAttachButton() {
  const attach = use(ComposerContext)?.attach;
  if (attach === undefined) {
    return null;
  }
  const full = attach.room === 0;
  const plus = (
    <Pressable
      testID="composer-attach"
      accessibilityRole="button"
      accessibilityLabel="Attach"
      accessibilityHint={
        full
          ? `${attach.limits.max_per_message} attachments is the limit.`
          : "Photo Library, Take Photo or Choose File."
      }
      accessibilityState={{ disabled: full }}
      disabled={full}
      hitSlop={SLOP}
      className={cn(CIRCLE, "active:opacity-60")}
    >
      <SymbolIcon
        name="plus"
        size={17}
        weight="medium"
        tone={full ? "text_decorative" : "text_muted"}
      />
    </Pressable>
  );
  if (full) {
    return plus;
  }
  return (
    <MenuView
      testID="composer-attach-menu"
      entries={ATTACH_MENU}
      onAction={(id) => {
        if (isSource(id)) {
          attach.pick(id);
        }
      }}
    >
      {plus}
    </MenuView>
  );
}

/**
 * Before the trailing action: record a voice message, or nothing where the iPhone has no
 * speech support. While it records, a stop button with the time before it.
 */
export function ComposerVoiceButton() {
  const voice = use(ComposerContext)?.voice;
  const cursor = use(ComposerCursorContext);
  const state = voice?.state;
  const now = useNow(state?.kind === "recording");
  if (voice === undefined || state === undefined) {
    return null;
  }
  switch (state.kind) {
    case "recording": {
      const seconds = Math.max(0, Math.floor((now - state.startedAt) / 1000));
      return (
        <View className="flex-row items-center gap-6">
          <Text accessibilityElementsHidden className="text-mono-xs text-text-muted">
            {clock(seconds)}
          </Text>
          <Pressable
            testID="composer-voice"
            accessibilityRole="button"
            accessibilityLabel={stopLabel(seconds)}
            hitSlop={SLOP}
            onPress={voice.stop}
            className={cn(CIRCLE, "active:opacity-60")}
          >
            <SymbolIcon name="stop.fill" size={13} tone="state_error" />
          </Pressable>
        </View>
      );
    }
    case "preparing":
    case "transcribing":
      return (
        <View
          testID="composer-voice"
          accessible
          accessibilityRole="button"
          accessibilityLabel={state.kind === "preparing" && state.model ? MODEL_LINE : VOICE_LABEL}
          accessibilityState={{ disabled: true, busy: true }}
          className={CIRCLE}
        >
          <BrailleCell size="row" tint="work" />
        </View>
      );
    default:
      return (
        <Pressable
          testID="composer-voice"
          accessibilityRole="button"
          accessibilityLabel={VOICE_LABEL}
          accessibilityState={{ disabled: voice.busy }}
          disabled={voice.busy}
          hitSlop={SLOP}
          onPress={() => voice.start(cursor?.current)}
          className={cn(CIRCLE, "active:opacity-60")}
        >
          <SymbolIcon
            name="mic"
            size={17}
            weight="medium"
            tone={voice.busy ? "text_decorative" : "text_muted"}
          />
        </Pressable>
      );
  }
}
