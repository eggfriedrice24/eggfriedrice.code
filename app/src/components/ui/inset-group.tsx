// The inset grouped list of Settings (design 3.16): a group on surface_2 with a radius
// of 22, a header above it in caption text_muted and a footer under it in caption
// text_subtle. Rows are 46 pt high at least, with hairlines 16 pt in between them. The
// list puts 26 pt between groups (`gap-26` on the list).

import { Children, Fragment, type ReactNode } from "react";
import { Pressable, Switch, Text, View } from "react-native";

import { roles } from "@/theme/roles";

import { cn } from "./cn";
import { Hairline } from "./hairline";
import { SymbolIcon, type SymbolName } from "./symbol";

export interface InsetGroupProps {
  /** The header above the group, such as "Machines". */
  readonly header?: string;
  /** The footer under the group. A text node may hold a nested mono Text. */
  readonly footer?: ReactNode;
  /** The rows. A hairline goes between each two. */
  readonly children: ReactNode;
}

/** A group of rows with a header and a footer. */
export function InsetGroup(props: InsetGroupProps) {
  const rows = Children.toArray(props.children);
  const header = props.header;
  const footer = props.footer;
  return (
    <View>
      {header === undefined ? null : (
        <Text accessibilityRole="header" className="px-20 pb-7 text-caption text-text-muted">
          {header}
        </Text>
      )}
      <View className="overflow-hidden rounded-group bg-surface-2">
        {rows.map((row, index) => (
          // Children.toArray gives each row a stable key of its own.
          <Fragment key={keyOf(row, index)}>
            {index > 0 ? <Hairline inset="inset" /> : null}
            {row}
          </Fragment>
        ))}
      </View>
      {footer === undefined ? null : (
        <Text className="px-20 pt-7 text-caption text-text-subtle">{footer}</Text>
      )}
    </View>
  );
}

function keyOf(row: ReturnType<typeof Children.toArray>[number], index: number): string {
  if (typeof row === "object" && "key" in row && row.key !== null) {
    return row.key;
  }
  return `row-${String(index)}`;
}

/** The colour of a row title: text_primary, or yolk for "Pair a new machine". */
export type InsetRowTone = "text_primary" | "accent_primary";

const TITLE_TONE = {
  text_primary: "text-text-primary",
  accent_primary: "text-accent-primary",
} as const satisfies Record<InsetRowTone, string>;

export interface InsetRowProps {
  /** The title. Leave it out when `children` draws the whole row (a machine row). */
  readonly title?: string;
  readonly tone?: InsetRowTone;
  /** An SF Symbol before the title (20 pt, in the colour of the title). */
  readonly symbol?: SymbolName;
  /** The value at the end, in text_muted. */
  readonly value?: string;
  /** True for a value in mono, such as the version "0.1.0 (14)". */
  readonly valueMono?: boolean;
  /** A view at the end, such as a switch or an ellipsis button. */
  readonly accessory?: ReactNode;
  /** The row content in place of the title. */
  readonly children?: ReactNode;
  /** Makes the row a button. */
  readonly onPress?: () => void;
  readonly accessibilityLabel?: string;
}

/** One row of an inset group. */
export function InsetRow(props: InsetRowProps) {
  const tone = props.tone ?? "text_primary";
  const title = props.title;
  const value = props.value;
  const symbol = props.symbol;
  const content = (
    <>
      {symbol === undefined ? null : <SymbolIcon name={symbol} size={20} tone={tone} />}
      {title === undefined ? null : (
        <Text numberOfLines={1} className={cn("flex-1 text-row", TITLE_TONE[tone])}>
          {title}
        </Text>
      )}
      {props.children}
      {value === undefined ? null : (
        <Text
          numberOfLines={1}
          className={cn(
            props.valueMono === true ? "text-mono-shell" : "text-row",
            "text-text-muted",
          )}
        >
          {value}
        </Text>
      )}
      {props.accessory}
    </>
  );
  const frame = "min-h-46 flex-row items-center gap-12 px-16 py-10";
  const onPress = props.onPress;
  if (onPress === undefined) {
    return (
      <View accessibilityLabel={props.accessibilityLabel} className={frame}>
        {content}
      </View>
    );
  }
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={props.accessibilityLabel}
      onPress={onPress}
      className={cn(frame, "active:bg-selection-bg")}
    >
      {content}
    </Pressable>
  );
}

export interface SwitchRowProps {
  readonly title: string;
  readonly value: boolean;
  readonly onValueChange: (value: boolean) => void;
}

/** A row with a switch, such as "Haptics". The thumb is the system white (design note 10). */
export function SwitchRow(props: SwitchRowProps) {
  return (
    <InsetRow
      title={props.title}
      accessory={
        <Switch
          accessibilityLabel={props.title}
          value={props.value}
          onValueChange={props.onValueChange}
          trackColor={{ true: roles.accent_primary, false: roles.selection_bg }}
          ios_backgroundColor={roles.selection_bg}
        />
      }
    />
  );
}
