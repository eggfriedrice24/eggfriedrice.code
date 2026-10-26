// The S-Loaders board: the one loader of the app (BrailleCell) in each size and tint, its
// still frame, the block cursors, the running line and the loading line (design 3.20).

import { useState } from "react";
import { View } from "react-native";

import { BlockCursor } from "@/components/ui/block-cursor";
import { BrailleCell, type BrailleSize, type BrailleTint } from "@/components/ui/braille-cell";
import { MonoText } from "@/components/ui/mono-text";
import { LoadingLine } from "@/features/conversation/components/loading-line";
import { RunningLineView } from "@/features/conversation/components/running-line";
import { SheetCase, SheetScroll } from "./sheet-layout";

const SIZES: readonly BrailleSize[] = ["inline", "row", "button"];

const TINTS: readonly (readonly [BrailleTint, string])[] = [
  ["work", "yolk: efr works"],
  ["wait", "text_muted: the app waits on the network"],
  ["on_accent", "text_on_accent: inside Send and Allow"],
];

/** The S-Loaders board. */
export function LoadersSheet() {
  // The running line shows its time from 2 s on, as on a real turn.
  const [startedAt] = useState(() => Date.now());
  return (
    <SheetScroll>
      <SheetCase name="BrailleCell: sizes 7 x 13, 8 x 15, 9 x 17">
        <View className="flex-row items-center gap-16">
          {SIZES.map((size) => (
            <BrailleCell key={size} size={size} />
          ))}
        </View>
      </SheetCase>
      {TINTS.map(([tint, words]) => (
        <SheetCase key={tint} name={`tint ${words}`}>
          <View
            className={
              tint === "on_accent"
                ? "self-start rounded-full bg-accent-primary p-8"
                : "flex-row items-center"
            }
          >
            <BrailleCell tint={tint} size="row" />
          </View>
        </SheetCase>
      ))}
      <SheetCase name="still frame (menus, Reduce Motion)">
        <BrailleCell still />
      </SheetCase>
      <SheetCase name="running line">
        <RunningLineView startedAt={startedAt} compacting={false} />
      </SheetCase>
      <SheetCase name="compacting">
        <RunningLineView startedAt={startedAt} compacting />
      </SheetCase>
      <SheetCase name="loading line">
        <LoadingLine machineName="desktop" />
      </SheetCase>
      <SheetCase name="cursors: shell (blinks), prose and code (streaming), idle">
        <View className="flex-row items-center gap-16">
          <View className="flex-row items-center gap-4">
            <MonoText size="monoShell" tone="accent_primary">
              ❯
            </MonoText>
            <BlockCursor size="shell" mode="blink" />
          </View>
          <BlockCursor size="prose" mode="streaming" />
          <BlockCursor size="code" mode="streaming" />
          <BlockCursor size="shell" mode="idle" />
        </View>
      </SheetCase>
    </SheetScroll>
  );
}
