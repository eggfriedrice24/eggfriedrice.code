// The thought row (design 3.6, round 4 item 4 and the S-Messages sheet): one mono 11/16
// line in text_subtle, collapsed by default. A tap toggles it.
//
// - "thought · <title> · 9s", or "thinking · <title> · 12s" with the yolk braille cell
//   while the turn runs. model/thought-line.ts says which title and which time.
// - A bold title is text_muted; the first words of a reasoning without one are italic
//   text_subtle. The title truncates at the end of the line; the time never does.
// - Expanded: the reasoning in SF Pro 13/19, 16 pt in. With no text, efrd kept only the
//   title after the turn, so the row says "the reasoning isn't kept after the turn ends".
//
// The open state lives in the row state of the conversation (the list recycles rows).
// The row does not animate, so Reduce Motion changes nothing here.

import { useSelector } from "@tanstack/react-store";
import { Pressable, Text, View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { BrailleCell } from "@/components/ui/braille-cell";
import { MonoText } from "@/components/ui/mono-text";
import { SymbolIcon } from "@/components/ui/symbol";
import { paragraphs } from "../model/paragraphs";
import { rowFlag } from "../model/row-ui-state";
import { compareShallow, selectReasoning } from "../model/selectors";
import { thoughtLine } from "../model/thought-line";
import { useNow } from "./use-now";
import { useTranscriptScope } from "./use-transcript-scope";

export interface ThoughtRowViewProps {
  readonly title: string | undefined;
  /** The reasoning; "" when efrd did not keep it. */
  readonly text: string;
  /** True while the turn runs, so the reasoning can grow. */
  readonly live: boolean;
  readonly startedAt: number | undefined;
  /** When the first thought ended. */
  readonly endedAt?: number | undefined;
  readonly expanded: boolean;
  readonly onToggle: () => void;
}

/** The thought row. */
export function ThoughtRowView(props: ThoughtRowViewProps) {
  // NOTE: the turn can run on after its first thought, while the answer comes.
  const thinking = props.live && props.endedAt === undefined;
  const now = useNow(thinking);
  const expanded = props.expanded;
  const line = thoughtLine({
    title: props.title,
    text: props.text,
    live: thinking,
    expanded,
    startedAt: props.startedAt,
    endedAt: props.endedAt,
    now,
  });
  const parts = paragraphs(props.text);
  // Only the line is the button, so VoiceOver can read the reasoning under it.
  return (
    <View>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={line.label}
        accessibilityState={{ expanded }}
        accessibilityHint={expanded ? "Hides the reasoning." : "Shows the reasoning."}
        onPress={props.onToggle}
        className="min-h-24 flex-row items-center gap-6"
      >
        <SymbolIcon
          name={expanded ? "chevron.down" : "chevron.right"}
          size={10}
          tone="text_subtle"
          weight="semibold"
        />
        {thinking ? <BrailleCell /> : null}
        <MonoText numberOfLines={1} className="shrink">
          {line.word}
          {line.title === undefined ? null : (
            <>
              {" · "}
              <Text className="text-text-muted">{line.title}</Text>
            </>
          )}
          {line.excerpt === undefined ? null : (
            <>
              {" · "}
              <Text className="italic">{line.excerpt}</Text>
            </>
          )}
        </MonoText>
        {line.time === undefined ? null : (
          <MonoText className="shrink-0">{`· ${line.time}`}</MonoText>
        )}
      </Pressable>
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
    </View>
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
      endedAt={reasoning.endedAt}
      expanded={expanded}
      onToggle={() => scope.rowUi.toggle(props.rowKey, "expanded")}
    />
  );
}
