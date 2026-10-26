// The frame of a component sheet of the gallery: a list under the transparent bar, with
// the name of each state above it in mono, as the canvas labels its parts.

import type { ReactNode } from "react";
import { ScrollView, View } from "react-native";

import { MonoText } from "@/components/ui/mono-text";

export interface SheetScrollProps {
  readonly children: ReactNode;
}

/** The list of a sheet. */
export function SheetScroll(props: SheetScrollProps) {
  return (
    <ScrollView
      contentInsetAdjustmentBehavior="automatic"
      className="flex-1 bg-surface-1"
      contentContainerClassName="gap-26 px-gutter pt-20 pb-20"
    >
      {props.children}
    </ScrollView>
  );
}

export interface SheetCaseProps {
  /** The name of the state, such as `waiting, with a reason`. */
  readonly name: string;
  /** What to do to see the state, when it needs a tap. */
  readonly hint?: string | undefined;
  readonly children: ReactNode;
}

/** One state of a sheet, under its name. */
export function SheetCase(props: SheetCaseProps) {
  return (
    <View className="gap-8">
      <MonoText tone="text_muted">{props.name}</MonoText>
      {props.hint === undefined ? null : <MonoText>{props.hint}</MonoText>}
      {props.children}
    </View>
  );
}
