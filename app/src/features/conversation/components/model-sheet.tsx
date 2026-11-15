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
// `ModelSheetView` draws a model only, for the tests and the gallery.

import { Pressable, ScrollView, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { BrailleCell } from "@/components/ui/braille-cell";
import { cn } from "@/components/ui/cn";
import { MonoText } from "@/components/ui/mono-text";
import { SmallFilledButton } from "@/components/ui/small-buttons";
import { SymbolIcon } from "@/components/ui/symbol";
import {
  COMPACTS_TEXT,
  type SheetRow,
  type SheetSection,
  type SheetView,
} from "../model/model-sheet";
import { EffortSlider } from "./effort-slider";
import { ProviderLogo } from "./provider-logo";

/** The smallest space under the confirm, on a screen without a home indicator. */
const MIN_BOTTOM = 12;

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
  /** True to draw the effort steps as buttons, as a build without the native slider. */
  readonly fallbackSlider?: boolean;
}

/** The sheet. */
export function ModelSheetView(props: ModelSheetViewProps) {
  const { view } = props;
  const bottom = Math.max(useSafeAreaInsets().bottom, MIN_BOTTOM);
  return (
    <View testID="model-sheet" className="bg-surface-1">
      <ScrollView bounces={false} className="shrink grow-0" contentContainerClassName="px-16 pt-14">
        {props.empty === undefined ? null : (
          <EmptyLine empty={props.empty} machineName={props.machineName} />
        )}
        {view.sections.map((section) => (
          <Section key={section.key} section={section} onRow={props.onRow} />
        ))}
      </ScrollView>
      <View className="gap-10 px-20 pt-14" style={{ paddingBottom: bottom }}>
        {view.effort === undefined ? null : (
          <EffortSlider
            effort={view.effort}
            onChange={props.onEffort}
            fallback={props.fallbackSlider === true}
          />
        )}
        {view.quiet === undefined ? null : (
          <Text testID="model-quiet-line" className="text-caption text-text-subtle">
            {view.quiet.text}
            {view.quiet.useDefault ? (
              <>
                {"  "}
                <Text
                  accessibilityRole="button"
                  onPress={props.onUseDefault}
                  className="text-caption text-text-bright"
                >
                  Use default
                </Text>
              </>
            ) : null}
          </Text>
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
        <ModelRow key={row.key} row={row} onPress={() => props.onRow(row)} />
      ))}
    </View>
  );
}

interface ModelRowProps {
  readonly row: SheetRow;
  readonly onPress: () => void;
}

/** The style of the name of `row`. */
function nameClass(row: SheetRow): string {
  if (row.gone !== undefined) {
    return "text-row text-text-subtle line-through";
  }
  if (row.disabled) {
    return "text-row text-text-muted";
  }
  return row.selected ? "text-row text-text-bright" : "text-row text-text-primary";
}

function ModelRow(props: ModelRowProps) {
  const { row } = props;
  return (
    <Pressable
      testID={`model-row-${row.key}`}
      accessibilityRole="button"
      accessibilityLabel={row.label}
      accessibilityState={{ selected: row.selected, disabled: row.disabled }}
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
