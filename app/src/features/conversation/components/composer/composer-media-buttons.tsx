// The two media buttons of the composer, in both sizes of the pill (the user,
// 2026-11-17, as t3code and Claude Code):
//
// - Attach (plus) in slot B, at the bottom left (board S-Attach): a tap opens the native
//   menu Photo Library, Take Photo, Choose File. It shows only when the build has the
//   pickers and efrd takes attachments; else nothing stands in its place. At the limit
//   of efrd it is off.
// - Voice (mic) before the trailing action: later it records a voice message, which
//   needs a native audio module.
//
// Each is a 32 pt circle with no fill and a 44 pt hit area, as the other actions, with
// its glyph in text_muted, or in text_decorative while it is off.

import { use } from "react";
import { Pressable } from "react-native";

import { cn } from "@/components/ui/cn";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { SymbolIcon, type SymbolName } from "@/components/ui/symbol";
import { SPACING } from "@/theme/metrics";

import type { AttachSource } from "../../model/attachments";
import { ComposerContext } from "./composer-context";

/** The hit slop that takes a 32 pt action to the 44 pt hit area. */
const SLOP = (SPACING.hit - SPACING.action) / 2;

const CIRCLE = "w-action h-action my-4 items-center justify-center rounded-full";

/** The VoiceOver hint of a media button that is not available yet. */
export const NOT_YET_HINT = "Not available yet.";

/** The items of the plus menu, in the order of the board. */
export const ATTACH_MENU: readonly (MenuEntry & { readonly id: AttachSource })[] = [
  { id: "photos", title: "Photo Library", image: "photo.on.rectangle" },
  { id: "camera", title: "Take Photo", image: "camera" },
  { id: "files", title: "Choose File", image: "folder" },
];

function isSource(id: string): id is AttachSource {
  return id === "photos" || id === "camera" || id === "files";
}

interface MediaButtonProps {
  readonly testID: string;
  readonly icon: SymbolName;
  readonly label: string;
}

/** A media button that is off until its feature ships. */
function MediaButton(props: MediaButtonProps) {
  return (
    <Pressable
      testID={props.testID}
      accessibilityRole="button"
      accessibilityLabel={props.label}
      accessibilityHint={NOT_YET_HINT}
      accessibilityState={{ disabled: true }}
      disabled
      hitSlop={SLOP}
      className={CIRCLE}
    >
      <SymbolIcon name={props.icon} size={17} weight="medium" tone="text_decorative" />
    </Pressable>
  );
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

/** Before the trailing action: record a voice message (off for now). */
export function ComposerVoiceButton() {
  return <MediaButton testID="composer-voice" icon="mic" label="Record a voice message" />;
}
