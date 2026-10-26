// The thought row (design 3.6 and the S-Messages sheet): one mono 11/16 line in
// text_subtle, collapsed by default. A tap toggles it.
//
// - Collapsed: chevron.right and "thought · Planning the migration".
// - Expanded and live: chevron.down, the braille cell in yolk, "thinking · Planning the
//   migration · 12s", then the reasoning in SF Pro 13/19, 16 pt in.
// - Expanded with no text: efrd keeps only the title after the turn, so the row says
//   "the reasoning isn't kept after the turn ends".
//
// The open state lives in the row state of the conversation (the list recycles rows).
// The row does not animate, so Reduce Motion changes nothing here.

import { useSelector } from "@tanstack/react-store";
import { Pressable, View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { BrailleCell } from "@/components/ui/braille-cell";
import { MonoText } from "@/components/ui/mono-text";
import { SymbolIcon } from "@/components/ui/symbol";
import { elapsed } from "../model/durations";
import { paragraphs } from "../model/paragraphs";
import { rowFlag } from "../model/row-ui-state";
import { compareShallow, selectReasoning } from "../model/selectors";
import { useNow } from "./use-now";
import { useTranscriptScope } from "./use-transcript-scope";

/** The words of the row: "thinking · title · 12s" or "thought · title". */
export function thoughtLine(
  title: string | undefined,
  live: boolean,
  time: string | undefined,
): string {
  const parts = [live ? "thinking" : "thought"];
  if (title !== undefined && title !== "") {
    parts.push(title);
  }
  if (live && time !== undefined) {
    parts.push(time);
  }
  return parts.join(" · ");
}

export interface ThoughtRowViewProps {
  readonly title: string | undefined;
  /** The reasoning; "" when efrd did not keep it. */
  readonly text: string;
  /** True while the turn runs, so the reasoning can grow. */
  readonly live: boolean;
  readonly startedAt: number | undefined;
  readonly expanded: boolean;
  readonly onToggle: () => void;
}

/** The thought row. */
export function ThoughtRowView(props: ThoughtRowViewProps) {
  const now = useNow(props.live);
  const line = thoughtLine(props.title, props.live, elapsed(props.startedAt, now));
  const expanded = props.expanded;
  const parts = paragraphs(props.text);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={line}
      accessibilityState={{ expanded }}
      accessibilityHint={expanded ? "Hides the reasoning." : "Shows the reasoning."}
      onPress={props.onToggle}
    >
      <View className="min-h-24 flex-row items-center gap-6">
        <SymbolIcon
          name={expanded ? "chevron.down" : "chevron.right"}
          size={10}
          tone="text_subtle"
          weight="semibold"
        />
        {expanded && props.live ? <BrailleCell /> : null}
        <MonoText numberOfLines={1} className="flex-1">
          {line}
        </MonoText>
      </View>
      {expanded ? (
        parts.length > 0 ? (
          <View className="gap-8 pl-16">
            {parts.map((paragraph) => (
              <BodyText key={paragraph.key} size="thought" tone="text_subtle">
                {paragraph.text}
              </BodyText>
            ))}
          </View>
        ) : (
          <MonoText className="-mt-6 pl-16">the reasoning isn't kept after the turn ends</MonoText>
        )
      ) : null}
    </Pressable>
  );
}

export interface ThoughtRowProps {
  readonly rowKey: string;
  readonly turnId: string;
}

/** The thought row of turn `turnId`, from the stream store. */
export function ThoughtRow(props: ThoughtRowProps) {
  const scope = useTranscriptScope();
  const reasoning = useSelector(scope.store, selectReasoning(props.turnId), {
    compare: compareShallow,
  });
  const expanded = useSelector(scope.rowUi.store, rowFlag(props.rowKey, "expanded"));
  return (
    <ThoughtRowView
      title={reasoning.title}
      text={reasoning.buffer?.text ?? ""}
      live={reasoning.live}
      startedAt={reasoning.startedAt}
      expanded={expanded}
      onToggle={() => scope.rowUi.toggle(props.rowKey, "expanded")}
    />
  );
}
