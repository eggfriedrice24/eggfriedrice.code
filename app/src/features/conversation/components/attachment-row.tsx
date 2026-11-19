// The row of attachment tiles (board S-Attach): one horizontal row that never wraps and
// scrolls sideways when it is full. iOS has no part for it, so it is the one part of
// attach that efr draws; the menus are UIMenus of MenuView.
//
// - A tile is 80 pt, 12 pt corners. A photo shows its preview, aspect fill. A file, and a
//   photo with no preview on this phone, shows its SF Symbol at 28 pt on surface_2 with a
//   border_default line. No badge and no kind label.
// - The caption under it: the name (text_primary, one line, cut in the middle), then the
//   size or the state that takes its place (attachments-format.ts).
// - In the draft each tile has the xmark at its top right (22 pt, 44 pt hit area) and a
//   long press menu: Quick Look for a photo, and Remove. In the transcript a tap on a photo
//   opens Quick Look.
// - From the text size AX1 the tile is 104 pt, and the caption stops at 1.5 times.

import { useState } from "react";
import {
  Image,
  Pressable,
  ScrollView,
  Text,
  View,
  useWindowDimensions,
  type AccessibilityActionInfo,
} from "react-native";

import { cn } from "@/components/ui/cn";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { SymbolIcon, type SymbolName } from "@/components/ui/symbol";
import { TEXT_TONE_CLASS, type TextTone } from "@/components/ui/text-classes";
import { MATERIALS } from "@/theme/materials";
import { SPACING, TILE } from "@/theme/metrics";
import {
  captionLine,
  removeLabel,
  tileLabel,
  type TileKind,
  type TileState,
} from "../model/attachments-format";
import type { TileView } from "../model/attachments-tiles";

/** The glyph of a tile with no photo. */
const GLYPHS: Readonly<Record<TileKind, SymbolName>> = {
  photo: "photo",
  image: "photo",
  pdf: "doc.richtext",
  file: "doc",
};

/** The colour of the size line in each state (board S-Attach, roles). */
const STATE_TONES: Readonly<Record<TileState["kind"], TextTone>> = {
  size: "text_muted",
  preparing: "text_muted",
  uploading: "accent_primary",
  waiting: "text_subtle",
  again: "text_subtle",
  not_sent: "state_error",
  as_path: "state_warning",
  over_limit: "state_error",
};

/** The hit slop that takes the 22 pt xmark to the 44 pt hit area. */
const MARK_SLOP = (SPACING.hit - TILE.mark) / 2;

/** The items of the long press menu of a draft tile. */
function draftMenu(canLook: boolean): MenuEntry[] {
  const remove: MenuEntry = { id: "remove", title: "Remove", image: "trash", destructive: true };
  return canLook ? [{ id: "look", title: "Quick Look", image: "eye" }, remove] : [remove];
}

export interface AttachmentRowProps {
  readonly tiles: readonly TileView[];
  /** The draft: each tile gets the xmark and the long press menu. */
  readonly onRemove?: ((key: string) => void) | undefined;
  /** Quick Look of a photo tile; without it a photo takes no tap. */
  readonly onLook?: ((tile: TileView) => void) | undefined;
  readonly testID?: string;
}

/** The tiles of a draft or of a prompt, in one row. */
export function AttachmentRow(props: AttachmentRowProps) {
  const large = useWindowDimensions().fontScale >= TILE.largeFontScale;
  if (props.tiles.length === 0) {
    return null;
  }
  return (
    <ScrollView
      testID={props.testID ?? "attachment-row"}
      horizontal
      showsHorizontalScrollIndicator={false}
      keyboardShouldPersistTaps="handled"
      contentContainerStyle={{ gap: TILE.gap }}
    >
      {props.tiles.map((tile) => (
        // NOTE: the key holds the preview, so a tile whose preview failed starts again
        // when it gets another one.
        <AttachmentTile
          key={`${tile.key}:${tile.previewUri ?? ""}`}
          tile={tile}
          size={large ? TILE.largeSize : TILE.size}
          onRemove={props.onRemove}
          onLook={props.onLook}
        />
      ))}
    </ScrollView>
  );
}

interface AttachmentTileProps {
  readonly tile: TileView;
  readonly size: number;
  readonly onRemove: ((key: string) => void) | undefined;
  readonly onLook: ((tile: TileView) => void) | undefined;
}

function AttachmentTile(props: AttachmentTileProps) {
  const { tile, size, onRemove, onLook } = props;
  // Short local UI state: iOS purged the preview, so the tile shows its glyph.
  const [gone, setGone] = useState(false);
  const photo = gone ? undefined : tile.previewUri;
  const look = photo === undefined || onLook === undefined ? undefined : () => onLook(tile);
  const remove = onRemove === undefined ? undefined : () => onRemove(tile.key);
  const actions: AccessibilityActionInfo[] = [];
  if (remove !== undefined && look !== undefined) {
    actions.push({ name: "look", label: "Quick Look" });
  }
  if (remove !== undefined) {
    actions.push({ name: "remove", label: "Remove" });
  }
  // A draft tile opens its menu on a long press; a tile of the transcript opens on a tap.
  const tap = remove === undefined ? look : undefined;
  const square = (
    <Pressable
      testID={`attachment-${tile.key}`}
      accessible
      accessibilityRole={tap === undefined ? "image" : "button"}
      accessibilityLabel={tileLabel(tile)}
      accessibilityHint={tap === undefined ? undefined : "Opens Quick Look."}
      accessibilityActions={actions}
      onAccessibilityAction={(event) => {
        if (event.nativeEvent.actionName === "look") {
          look?.();
        } else if (event.nativeEvent.actionName === "remove") {
          remove?.();
        }
      }}
      onPress={tap}
      style={{ width: size, height: size, borderCurve: "continuous" }}
      className={cn(
        "items-center justify-center overflow-hidden rounded-control",
        photo === undefined && "border border-border-default bg-surface-2",
      )}
    >
      {photo === undefined ? (
        <SymbolIcon name={GLYPHS[tile.kind]} size={TILE.glyph} tone="text_muted" />
      ) : (
        <Image
          source={{ uri: photo }}
          resizeMode="cover"
          onError={() => setGone(true)}
          style={{ width: size, height: size }}
        />
      )}
    </Pressable>
  );
  return (
    <View style={{ width: size }} className="gap-4">
      <View>
        {remove === undefined ? (
          square
        ) : (
          <MenuView
            testID={`attachment-menu-${tile.key}`}
            entries={draftMenu(look !== undefined)}
            onAction={(id) => (id === "look" ? look?.() : remove())}
            openOnLongPress
          >
            {square}
          </MenuView>
        )}
        {remove === undefined ? null : (
          <Pressable
            testID={`attachment-remove-${tile.key}`}
            accessibilityRole="button"
            accessibilityLabel={removeLabel(tile.name)}
            hitSlop={MARK_SLOP}
            onPress={remove}
            style={{
              top: TILE.markInset,
              right: TILE.markInset,
              width: TILE.mark,
              height: TILE.mark,
              backgroundColor: MATERIALS.tileMark,
            }}
            className="absolute items-center justify-center rounded-full"
          >
            <SymbolIcon name="xmark" size={TILE.markGlyph} weight="bold" tone="text_bright" />
          </Pressable>
        )}
      </View>
      <View accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
        <Text
          numberOfLines={1}
          ellipsizeMode="middle"
          maxFontSizeMultiplier={TILE.maxFontScale}
          className="text-caption text-text-primary"
        >
          {tile.name}
        </Text>
        <Text
          numberOfLines={1}
          maxFontSizeMultiplier={TILE.maxFontScale}
          className={cn("text-caption-small", TEXT_TONE_CLASS[STATE_TONES[tile.state.kind]])}
        >
          {captionLine(tile)}
        </Text>
      </View>
    </View>
  );
}
