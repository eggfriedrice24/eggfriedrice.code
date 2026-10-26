// The key and value box of the pairing confirm (design 3.17): "device iPhone 17 Pro" and
// "key 7F3A 91C2 4D0E B6A1", so the user can compare them with the terminal. A box with
// a radius of 12 and a 1 pt border_subtle edge; rows in mono with a 56 pt key column. A
// long press on a row copies its value, with the light haptic.

import { Fragment } from "react";
import { Pressable, Text, View } from "react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import { FINGERPRINT_LETTER_SPACING } from "@/theme/type";

import { Hairline } from "./hairline";

/** One row of the box. */
export interface KeyValueRow {
  readonly key: string;
  readonly value: string;
  /** True for a key fingerprint: the value gets the letter spacing of the design. */
  readonly fingerprint?: boolean;
}

export interface KeyValueBoxProps {
  readonly rows: readonly KeyValueRow[];
}

/** A box of keys and values. */
export function KeyValueBox(props: KeyValueBoxProps) {
  return (
    <View className="rounded-control border border-border-subtle">
      {props.rows.map((row, index) => (
        <Fragment key={row.key}>
          {index > 0 ? <Hairline inset="inset" /> : null}
          <Pressable
            accessibilityLabel={`${row.key}, ${row.value}`}
            accessibilityHint="Long press to copy."
            onLongPress={() => {
              void copyText(row.value);
              haptic("light");
            }}
            className="flex-row items-baseline gap-12 px-16 py-13"
          >
            <Text className="w-56 text-mono-xs text-text-subtle">{row.key}</Text>
            <Text
              selectable
              className="flex-1 text-mono-value text-text-bright"
              style={
                row.fingerprint === true ? { letterSpacing: FINGERPRINT_LETTER_SPACING } : undefined
              }
            >
              {row.value}
            </Text>
          </Pressable>
        </Fragment>
      ))}
    </View>
  );
}
