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
// Round 7 (S-ModelSheet r7):
// - The search field and the filter button are pinned under the grabber. The filter is a
//   native pull-down (UIMenu): one choice of provider, then "Legacy models". Its glyph
//   fills in text_bright while a filter is on.
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
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { MonoText } from "@/components/ui/mono-text";
import { SearchField } from "@/components/ui/search-field";
import { SmallFilledButton } from "@/components/ui/small-buttons";
import { SymbolIcon } from "@/components/ui/symbol";
import {
  COMPACTS_TEXT,
  type FilterView,
  type KindFilter,
  type SheetRow,
  type SheetSection,
  type SheetView,
} from "../model/model-sheet";
import { EffortSlider } from "./effort-slider";
import { ProviderLogo } from "./provider-logo";

/** The smallest space under the confirm, on a screen without a home indicator. */
const MIN_BOTTOM = 12;

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
  readonly onConfirm: () => void;
  /** The words of the search field. */
  readonly query: string;
  readonly onQuery: (query: string) => void;
  /** Called with true when the search field gets the focus and with false when it loses it. */
  readonly onSearchFocus?: ((focused: boolean) => void) | undefined;
  readonly onKind: (kind: KindFilter) => void;
  readonly onLegacy: (on: boolean) => void;
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
  const bottom = Math.max(useSafeAreaInsets().bottom, MIN_BOTTOM);
  // Short local UI state: rows lie under the pinned panel.
  const [under, setUnder] = useState(false);
  const [box, setBox] = useState({ content: 0, frame: 0 });
  return (
    <View testID="model-sheet" className="bg-surface-1">
      <View className="flex-row items-center gap-4 pt-14 pr-8 pl-16">
        <SearchField
          value={props.query}
          onChangeText={props.onQuery}
          placeholder={view.filter.placeholder}
          accessibilityLabel="Search models"
          className="mx-0 flex-1"
          testID="model-search"
          onFocusChange={props.onSearchFocus}
        />
        <FilterButton filter={view.filter} onKind={props.onKind} onLegacy={props.onLegacy} />
      </View>
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
        className="shrink grow-0"
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
          <View testID="model-quiet-line" className="flex-row flex-wrap items-center gap-x-4">
            <Text className="shrink text-caption text-text-subtle">{view.quiet.text}</Text>
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

/** The items of the filter menu. */
export function filterEntries(filter: FilterView): MenuEntry[] {
  const legacy: MenuEntry = {
    id: "legacy",
    title: "Legacy models",
    state: filter.legacy ? "on" : "off",
  };
  if (!filter.kinds) {
    return [legacy];
  }
  const kinds: [KindFilter, string][] = [
    ["all", "All"],
    ["claude", "Claude"],
    ["openai", "OpenAI"],
  ];
  return [
    ...kinds.map(([kind, title]): MenuEntry => ({
      id: `kind:${kind}`,
      title,
      state: filter.kind === kind ? "on" : "off",
    })),
    { section: "legacy", items: [legacy] },
  ];
}

function isKind(id: string): id is KindFilter {
  return id === "all" || id === "claude" || id === "openai";
}

interface FilterButtonProps {
  readonly filter: FilterView;
  readonly onKind: (kind: KindFilter) => void;
  readonly onLegacy: (on: boolean) => void;
}

/** The filter button right of the search field: 44 pt touch, a 22 pt glyph. */
function FilterButton(props: FilterButtonProps) {
  const { filter } = props;
  const pick = (id: string): void => {
    if (id === "legacy") {
      props.onLegacy(!filter.legacy);
      return;
    }
    const kind = id.slice("kind:".length);
    if (isKind(kind)) {
      props.onKind(kind);
    }
  };
  return (
    <MenuView testID="model-filter-menu" entries={filterEntries(filter)} onAction={pick}>
      <View
        testID="model-filter"
        accessible
        accessibilityRole="button"
        accessibilityLabel={filter.label}
        className="h-44 w-44 items-center justify-center"
      >
        <SymbolIcon
          name={
            filter.on
              ? "line.3.horizontal.decrease.circle.fill"
              : "line.3.horizontal.decrease.circle"
          }
          size={22}
          tone={filter.on ? "text_bright" : "text_muted"}
        />
      </View>
    </MenuView>
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

function ConfirmRow(props: ModelSheetViewProps) {
  const confirm = props.view.confirm;
  if (confirm === undefined) {
    return null;
  }
  return (
    <View className="gap-6">
      {confirm.compacts ? (
        <View testID="model-cost-line" className="flex-row items-center gap-7">
          <SymbolIcon name="exclamationmark.triangle" size={13} tone="state_warning" />
          <Text className="flex-1 text-reason text-text-muted">{COMPACTS_TEXT}</Text>
        </View>
      ) : null}
      <View className="min-h-44 flex-row items-center gap-10">
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
        {confirm.next ? <Text className="text-reason text-text-subtle">next message</Text> : null}
        <View className="flex-1" />
        <SmallFilledButton
          label={confirm.label}
          pending={confirm.pending}
          pendingLabel="Asking what the switch costs"
          onPress={props.onConfirm}
        />
      </View>
    </View>
  );
}
