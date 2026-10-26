// The counts of the render counter (lib/dev/render-counts.ts) over the screen that it
// measures: for each part, the commits, the commits per second, and the time of the last
// and of the longest commit. It reads the counts twice a second, so it adds no render to
// the part that it measures. React counts only in a dev build.

import { useSyncExternalStore } from "react";
import { Pressable, View } from "react-native";

import { MonoText } from "@/components/ui/mono-text";
import {
  commitsPerSecond,
  renderCounts,
  type RenderCounts,
  type RenderSample,
} from "@/lib/dev/render-counts";

/** One line of the overlay. */
function lineOf(id: string, sample: RenderSample): string {
  const rate = commitsPerSecond(sample).toFixed(1);
  const mean = (sample.totalMs / sample.commits).toFixed(2);
  return `${id} ${sample.commits} commits · ${rate}/s · mean ${mean} ms · max ${sample.maxMs.toFixed(1)} ms`;
}

/** Checks the counts twice a second. */
function everyHalfSecond(onChange: () => void): () => void {
  const timer = setInterval(onChange, 500);
  return () => clearInterval(timer);
}

/** The counts of `counts`, read twice a second. */
function useCounts(counts: RenderCounts): readonly (readonly [string, RenderSample])[] {
  return useSyncExternalStore(everyHalfSecond, () => counts.snapshot());
}

export interface RenderOverlayProps {
  /** The distance from the top of the screen, under the bar and the strip. */
  readonly top: number;
}

/** The overlay of the render counter. A tap resets the counts. */
export function RenderOverlay(props: RenderOverlayProps) {
  const samples = useCounts(renderCounts);
  const lines =
    samples.length === 0
      ? ["no commit yet (React counts in a dev build)"]
      : samples.map(([id, sample]) => lineOf(id, sample));
  return (
    <View pointerEvents="box-none" className="absolute right-12 left-12" style={{ top: props.top }}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel="Reset the render counts"
        onPress={() => renderCounts.reset()}
        className="rounded-inner border-hairline border-border-subtle bg-surface-2 px-8 py-4"
      >
        {lines.map((line) => (
          <MonoText key={line} tone="text_muted">
            {line}
          </MonoText>
        ))}
      </Pressable>
    </View>
  );
}
