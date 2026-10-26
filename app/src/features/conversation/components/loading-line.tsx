// The loading line of a conversation that has no events yet (design item 8.19 and the
// S-Loaders sheet): the braille cell in text_muted, because the app waits on the
// network, and "loading conversation from desktop", with the elapsed time from 2 s on.

import { useState } from "react";
import { Text, View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { elapsed } from "../model/durations";
import { useNow } from "./use-now";

export interface LoadingLineProps {
  /** The name of the machine that the app waits for. */
  readonly machineName: string;
}

/** The loading line. */
export function LoadingLine(props: LoadingLineProps) {
  const [since] = useState(() => Date.now());
  const time = elapsed(since, useNow(true));
  const words = `loading conversation from ${props.machineName}`;
  return (
    <View
      accessible
      accessibilityRole="text"
      accessibilityLabel={words}
      className="min-h-24 flex-row items-center gap-6"
    >
      <BrailleCell tint="wait" label="Loading" />
      <Text className="text-mono-xs text-text-subtle">
        {time === undefined ? words : `${words} · ${time}`}
      </Text>
    </View>
  );
}
