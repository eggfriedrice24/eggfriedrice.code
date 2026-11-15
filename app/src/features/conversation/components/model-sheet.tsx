// The model sheet (S-ModelSheet, round 8 "minimal"): a native form sheet with the grabber
// and no bar. The rows of each account scroll; the slider, the quiet line and the confirm
// stay pinned at the bottom. model/model-sheet.ts says what each state shows.
//
// - A section header is caption text_subtle: the account, then its note. The warning
//   glyph of a limit from 80 % is state_warning and its words text_subtle, so it never
//   reads as the yellow of an action; "limit reached" is state_error.
// - A row is the logo, the name and the yolk check, 44 pt high at least. The selected row
//   has selection_bg and its name in text_bright. A dim row is text_muted with its logo
//   at 40 % and takes no tap.
// - The confirm is the only filled control: a small yolk capsule, only when something
//   changed. A switch that compacts adds one line with the warning glyph above it and the
//   plain toggle "Don't ask again here" on its left.
//
// Round 7 (S-ModelSheet r7), native after the device check:
// - The search, the filter and the confirm are native parts of the sheet, not views of
//   this file: the screen puts the search bar and the filter menu into the bottom
//   toolbar of iOS 26 and "Use" into the header (model-sheet-screen.tsx). The pinned
//   panel keeps the slider, the quiet line, and the warning of a switch that compacts.
// - The list draws no scroll indicator. While rows lie under the pinned panel, the panel
//   has a hairline at its top; at the end of the list the hairline goes. A drag closes
//   the keyboard.
// - A row is logo, name, check, star. The star is 16 pt with a touch of 44 pt: filled in
//   text_primary for the favourite, else border_default (text_subtle on the selected
//   row). A tap on it never selects the row. A row that takes no tap has no star
//   control; a dim favourite keeps its star at 40 %. For VoiceOver the star is an action
//   of the row.
// - The legacy rows of an account stand under "Legacy", a label in the column of the
//   names.
//
// `ModelSheetView` draws a model only, for the tests and the gallery.

import { useState } from "react";
import { Pressable, ScrollView, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { BrailleCell } from "@/components/ui/braille-cell";
import { cn } from "@/components/ui/cn";
import { MonoText } from "@/components/ui/mono-text";
import { SymbolIcon } from "@/components/ui/symbol";
import {
  COMPACTS_TEXT,
  type SheetRow,
  type SheetSection,
  type SheetView,
} from "../model/model-sheet";
import { EffortSlider } from "./effort-slider";
import { ProviderLogo } from "./provider-logo";

/** The smallest space under the panel, on a screen without a home indicator. */
const MIN_BOTTOM = 12;

/** The room of the bottom toolbar of iOS 26 (the search bar and the filter) over the panel. */
export const TOOLBAR_ROOM = 56;

/** The 16 pt line of "Use default" with a touch of 44 pt, as the other controls. */
const USE_DEFAULT_SLOP = { top: 14, bottom: 14, left: 8, right: 8 } as const;

/** What the sheet shows while it has no list. */
export type SheetEmpty = "loading" | "offline" | undefined;

export interface ModelSheetViewProps {
  readonly view: SheetView;
  /** No list yet: the braille cell while it loads, one line when the machine is away. */
  readonly empty: SheetEmpty;
  readonly machineName: string;
  /** "Don't ask again here" is on. */
  readonly quiet: boolean;
  readonly onRow: (row: SheetRow) => void;
  readonly onEffort: (effort: string) => void;
  readonly onUseDefault: () => void;
  readonly onQuiet: () => void;
  /** A tap on the star of `row`: it sets, moves or removes the favourite. */
  readonly onStar: (row: SheetRow) => void;
  /** True to draw the effort steps as buttons, as a build without the native slider. */
  readonly fallbackSlider?: boolean;
}

/** The space under the end of the list in which the pinned panel needs no hairline. */
const END_SLACK = 1;

/** The sheet. */
export function ModelSheetView(props: ModelSheetViewProps) {
  const { view } = props;
  const bottom = Math.max(useSafeAreaInsets().bottom, MIN_BOTTOM) + TOOLBAR_ROOM;
  // Short local UI state: rows lie under the pinned panel.
  const [under, setUnder] = useState(false);
  const [box, setBox] = useState({ content: 0, frame: 0 });
  return (
    <View testID="model-sheet" collapsable={false} className="flex-1 bg-surface-1">
      <ScrollView
        bounces={false}
        testID="model-list"
        showsVerticalScrollIndicator={false}
        keyboardDismissMode="on-drag"
        keyboardShouldPersistTaps="handled"
        scrollEventThrottle={32}
        onLayout={(event) => {
          const frame = event.nativeEvent.layout.height;
          setBox((now) => ({ ...now, frame }));
          setUnder(box.content - frame > END_SLACK);
        }}
        onContentSizeChange={(_width, content) => {
          setBox((now) => ({ ...now, content }));
          setUnder(content - box.frame > END_SLACK);
        }}
        onScroll={(event) => {
          const { contentOffset, contentSize, layoutMeasurement } = event.nativeEvent;
          setUnder(contentSize.height - contentOffset.y - layoutMeasurement.height > END_SLACK);
        }}
        contentInsetAdjustmentBehavior="automatic"
        className="flex-1"
        contentContainerClassName="px-16 pt-6"
      >
        {props.empty === undefined ? null : (
          <EmptyLine empty={props.empty} machineName={props.machineName} />
        )}
        {view.noMatch === undefined ? null : (
          <View testID="model-no-match" className="min-h-44 justify-center px-12">
            <MonoText tone="text_muted">{view.noMatch}</MonoText>
          </View>
        )}
        {view.sections.map((section) => (
          <Section key={section.key} section={section} onRow={props.onRow} onStar={props.onStar} />
        ))}
      </ScrollView>
      <View
        testID="model-panel"
        className={cn("gap-10 px-20 pt-14", under && "border-t-hairline border-border-subtle")}
        style={{ paddingBottom: bottom }}
      >
        {view.effort === undefined ? null : (
          <EffortSlider
            effort={view.effort}
            onChange={props.onEffort}
            fallback={props.fallbackSlider === true}
          />
        )}
        {view.quiet === undefined ? null : (
          // NOTE: two lines of room at all times, so a change of the effort never moves the
          // slider above it.
          <View
            testID="model-quiet-line"
            className="min-h-32 flex-row flex-wrap items-start gap-x-4"
          >
            <Text className="shrink text-caption text-text-subtle">
              {view.confirm?.next === true ? `${view.quiet.text} · next message` : view.quiet.text}
            </Text>
            {view.quiet.useDefault ? (
              // A control of its own, so the touch reaches 44 pt like every other one.
              <Pressable
                accessibilityRole="button"
                hitSlop={USE_DEFAULT_SLOP}
                onPress={props.onUseDefault}
                className="active:opacity-60"
              >
                <Text className="text-caption-strong text-text-bright">Use default</Text>
              </Pressable>
            ) : null}
          </View>
        )}
        <ConfirmRow {...props} />
      </View>
    </View>
  );
}

function EmptyLine(props: { readonly empty: "loading" | "offline"; readonly machineName: string }) {
  if (props.empty === "loading") {
    return (
      <View
        accessible
        accessibilityLabel="Loading the models"
        className="min-h-44 flex-row items-center gap-8 px-12"
      >
        <BrailleCell size="row" tint="wait" />
        <MonoText tone="text_muted">loading the models</MonoText>
      </View>
    );
  }
  return (
    <View className="min-h-44 justify-center px-12">
      <MonoText tone="text_muted">{`${props.machineName} is offline`}</MonoText>
    </View>
  );
}

interface SectionProps {
  readonly section: SheetSection;
  readonly onRow: (row: SheetRow) => void;
  readonly onStar: (row: SheetRow) => void;
}

function Section(props: SectionProps) {
  const { section } = props;
  const note = section.note;
  return (
    <View>
      {section.title === undefined ? null : (
        <View
          accessible
          accessibilityRole="header"
          className="flex-row flex-wrap items-baseline px-12 pt-14 pb-4"
        >
          <Text className="text-caption text-text-subtle">{section.title}</Text>
          {note === undefined ? null : (
            <View className="ml-4 flex-row items-center gap-4">
              <Text className="text-caption text-text-subtle">·</Text>
              {note.kind === "warn" ? (
                <SymbolIcon name="exclamationmark.triangle" size={11} tone="state_warning" />
              ) : null}
              <Text
                className={cn(
                  "text-caption",
                  note.kind === "reached" ? "text-state-error" : "text-text-subtle",
                )}
              >
                {note.text}
              </Text>
            </View>
          )}
        </View>
      )}
      {section.rows.map((row) => (
        <ModelRow
          key={row.key}
          row={row}
          onPress={() => props.onRow(row)}
          onStar={() => props.onStar(row)}
        />
      ))}
      {section.legacy.length === 0 ? null : (
        // The label stands in the column of the names: 12 pt inset, 24 pt logo, 12 pt gap.
        <Text
          accessibilityRole="header"
          testID="model-legacy-label"
          className="pt-8 pb-2 pl-48 text-caption text-text-subtle"
        >
          Legacy
        </Text>
      )}
      {section.legacy.map((row) => (
        <ModelRow
          key={row.key}
          row={row}
          onPress={() => props.onRow(row)}
          onStar={() => props.onStar(row)}
        />
      ))}
    </View>
  );
}

interface ModelRowProps {
  readonly row: SheetRow;
  readonly onPress: () => void;
  readonly onStar: () => void;
}

/** The colour role of the star of `row`. */
function starTone(row: SheetRow): "text_primary" | "text_subtle" | "border_default" {
  if (row.favourite) {
    return "text_primary";
  }
  return row.selected ? "text_subtle" : "border_default";
}

/** The star at the right end of a row: a control, or a dim mark of a favourite. */
function Star(props: { readonly row: SheetRow; readonly onPress: () => void }) {
  const { row } = props;
  const glyph = (
    <SymbolIcon name={row.favourite ? "star.fill" : "star"} size={16} tone={starTone(row)} />
  );
  if (!row.starrable) {
    return row.favourite ? (
      <View
        accessibilityElementsHidden
        className="h-44 w-44 items-center justify-center opacity-40"
      >
        {glyph}
      </View>
    ) : null;
  }
  // VoiceOver reaches the star as an action of the row, so the control itself is hidden.
  return (
    <Pressable
      testID={`model-star-${row.key}`}
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
      onPress={props.onPress}
      className="-mr-12 h-44 w-44 items-center justify-center active:opacity-60"
    >
      {glyph}
    </Pressable>
  );
}

/** The style of the name of `row`. */
function nameClass(row: SheetRow): string {
  if (row.gone !== undefined) {
    return "text-row text-text-subtle line-through";
  }
  if (row.disabled) {
    return "text-row text-text-muted";
  }
  return row.selected ? "text-row-strong text-text-bright" : "text-row text-text-primary";
}

function ModelRow(props: ModelRowProps) {
  const { row } = props;
  return (
    <Pressable
      testID={`model-row-${row.key}`}
      accessibilityRole="button"
      accessibilityLabel={row.label}
      accessibilityState={{ selected: row.selected, disabled: row.disabled }}
      accessibilityActions={row.starrable ? [{ name: "star", label: row.starLabel }] : []}
      onAccessibilityAction={(event) => {
        if (event.nativeEvent.actionName === "star") {
          props.onStar();
        }
      }}
      disabled={row.disabled}
      onPress={props.onPress}
      className={cn(
        "min-h-44 flex-row items-center gap-12 rounded-control px-12",
        row.selected && "bg-selection-bg",
      )}
    >
      <ProviderLogo logo={row.logo} dim={row.disabled} />
      <Text numberOfLines={1} className={cn("flex-1", nameClass(row))}>
        {row.name}
      </Text>
      {/* A model that is gone (S-ModelSheet, cell "gone"): the name struck through, and the
        words at the right end, so a long name never cuts them. */}
      {row.gone === undefined ? null : (
        <Text testID="model-row-gone" className="flex-none text-caption text-text-subtle">
          {row.gone}
        </Text>
      )}
      {row.checked ? (
        <SymbolIcon name="checkmark" size={17} tone="accent_primary" weight="semibold" />
      ) : null}
      <Star row={row} onPress={props.onStar} />
    </Pressable>
  );
}

/**
 * The warning of a switch that compacts, with "Don't ask again here". The confirm itself
 * is the native "Use" of the header, so a change of the effort alone adds no row here.
 */
function ConfirmRow(props: ModelSheetViewProps) {
  const confirm = props.view.confirm;
  if (confirm?.compacts !== true) {
    return null;
  }
  return (
    <View className="gap-6">
      <View testID="model-cost-line" className="flex-row items-center gap-7">
        <SymbolIcon name="exclamationmark.triangle" size={13} tone="state_warning" />
        <Text className="flex-1 text-reason text-text-muted">{COMPACTS_TEXT}</Text>
      </View>
      <View className="min-h-28 flex-row items-center gap-10">
        {confirm.compacts ? (
          <Pressable
            accessibilityRole="switch"
            accessibilityLabel="Don't ask again in this conversation"
            accessibilityState={{ checked: props.quiet }}
            hitSlop={{ top: 8, bottom: 8 }}
            onPress={props.onQuiet}
            className="min-h-28 justify-center active:opacity-60"
          >
            <Text
              className={cn(
                "text-reason",
                props.quiet ? "text-accent-primary" : "text-text-subtle",
              )}
            >
              {props.quiet ? "✓ Don't ask again here" : "Don't ask again here"}
            </Text>
          </Pressable>
        ) : null}
      </View>
    </View>
  );
}
