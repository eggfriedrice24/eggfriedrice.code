// The row of an `ask_user` call (S-Ask 4): the question of the model and how it ended,
// in the frame of a tool card. model/question.ts gives the words.
//
// - Waiting: the yolk dot, a border_accent edge, the question in text_primary, and
//   "waiting for you" (or "sending…") in accent_primary. A tap opens the action sheet,
//   as a tap on the strip does.
// - After its end: questionmark.circle in text_subtle, the question and the end, such as
//   "answered · Postgres 16" or "nobody answered", in text_muted.

import { useRef } from "react";
import { Pressable, View, type HostInstance } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { cn } from "@/components/ui/cn";
import { Dot } from "@/components/ui/dot";
import { MonoText } from "@/components/ui/mono-text";
import { SymbolIcon } from "@/components/ui/symbol";

import type { QuestionRow } from "../model/question";

export interface QuestionRowViewProps {
  readonly row: QuestionRow;
  /** Opens the action sheet of a waiting question, out of `anchor`; none: no tap. */
  readonly onOpen: ((anchor: HostInstance | null) => void) | undefined;
}

/** The card of a question of the model. */
export function QuestionRowView(props: QuestionRowViewProps) {
  const { row, onOpen } = props;
  const anchor = useRef<HostInstance>(null);
  const tappable = row.waiting && onOpen !== undefined;
  return (
    <View
      ref={anchor}
      testID="question-row"
      className={cn(
        "overflow-hidden rounded-box border",
        row.waiting ? "border-border-accent" : "border-border-subtle",
      )}
    >
      <Pressable
        accessibilityRole={tappable ? "button" : "text"}
        accessibilityLabel={`${row.question} ${row.state}.`}
        accessibilityHint={tappable ? "Opens the choices." : undefined}
        // NOTE: no `disabled`: VoiceOver reads a disabled view as "dimmed", and a row
        // that tells a past answer is no control. With no `onPress` it takes no tap.
        onPress={tappable ? () => onOpen(anchor.current) : undefined}
        className={cn(
          "flex-row items-start gap-8 px-12 py-9",
          tappable && "active:bg-selection-bg",
        )}
      >
        <View className="h-18 w-14 items-center justify-center">
          {row.waiting ? (
            <Dot size="large" />
          ) : (
            <SymbolIcon name="questionmark.circle" size={13} tone="text_subtle" />
          )}
        </View>
        <View className="min-w-0 flex-1 gap-2">
          <BodyText size="rowDetail" tone={row.waiting ? "text_primary" : "text_muted"}>
            {row.question}
          </BodyText>
          <MonoText
            tone={row.waiting ? "accent_primary" : "text_muted"}
            numberOfLines={1}
            ellipsizeMode="tail"
          >
            {row.state}
          </MonoText>
        </View>
      </Pressable>
    </View>
  );
}
