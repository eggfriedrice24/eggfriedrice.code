// One assistant message of the transcript (design 3.5; research R4c.4 steps 4 to 6).
// Text on surface_1, no bubble and no fill.
//
// - The row follows its message on its own frames (use-paced-message.ts): the pacer
//   shows new text at most 120 ms late, and the row renders at most once per frame. A
//   message that does not change does not render; a settled one runs no frames.
// - The text shows as segments (model/message-segments.ts): Markdown, and each fence in
//   our code block frame. Each segment has a view of its own, keyed by where it begins,
//   so a final segment keeps its native view and never parses again (the BlockView idea
//   of the old UI, now with the React Compiler instead of a hand-written memo).
// - The list recycles row instances: one instance can show a row of another turn. So
//   the paced part is keyed by its turn and index, and it remounts with a new message.
//   No state of one message (the paced view, the fade, the cursor, "copied") can show
//   on another one.
// - A new generation of the message (the completed text replaced the text) remounts the
//   body, so the fade of the tail never misaligns.
// - While the text arrives, the yolk prose cursor sits under it: solid while tokens
//   come, blinking after 600 ms of silence. Only a message that streamed while the row
//   was on screen fades its tail in.

import { useState } from "react";
import { View } from "react-native";

import { BlockCursor } from "@/components/ui/block-cursor";
import type { MessageView } from "@/lib/stream/reducer";
import type { Segment } from "../model/message-segments";
import type { ShownMessage } from "../model/paced-message";
import { CodeBlock, DiffFence } from "./code-block";
import { MarkdownView } from "./markdown-view";
import { usePacedMessage } from "./use-paced-message";
import { useTranscriptScope } from "./use-transcript-scope";

export interface MessageRowProps {
  readonly turnId: string;
  readonly index: number;
}

/** The row of message `index` of turn `turnId`. */
export function MessageRow(props: MessageRowProps) {
  // A recycled instance with a new message remounts all of the paced part.
  return (
    <PacedMessageRow
      key={`${props.turnId}:${props.index}`}
      turnId={props.turnId}
      index={props.index}
    />
  );
}

function PacedMessageRow(props: MessageRowProps) {
  const scope = useTranscriptScope();
  const shown = usePacedMessage(scope.store, props.turnId, props.index);
  const message = shown.message;
  if (message === undefined || message.buffer.text.length === 0) {
    return null;
  }
  // A new generation remounts the body and its views.
  return <MessageBody key={message.generation} shown={shown} message={message} />;
}

export interface MessageBodyProps {
  readonly shown: ShownMessage;
  readonly message: MessageView;
}

/** The segments of one shown message. */
export function MessageBody(props: MessageBodyProps) {
  // Only a message that streamed while this view was on screen fades its tail in.
  const [streamed] = useState(() => props.shown.streaming);
  const shown = props.shown;
  return (
    <View testID="message-row">
      {shown.segments.map((segment) => (
        <SegmentView key={segment.key} segment={segment} fade={streamed && shown.streaming} />
      ))}
      {shown.streaming && !lastIsOpenCode(shown.segments) ? (
        <BlockCursor size="prose" mode="streaming" activity={shown.text.length} />
      ) : null}
    </View>
  );
}

function lastIsOpenCode(segments: readonly Segment[]): boolean {
  const last = segments[segments.length - 1];
  return last?.kind === "code" && !last.closed;
}

function SegmentView(props: { readonly segment: Segment; readonly fade: boolean }) {
  const segment = props.segment;
  switch (segment.kind) {
    case "markdown":
      return <MarkdownView markdown={segment.text} streaming={props.fade && segment.open} />;
    case "code":
      return (
        <CodeBlock
          language={segment.language}
          source={segment.source}
          body={segment.body}
          closed={segment.closed}
        />
      );
    case "diff":
      return <DiffFence body={segment.body} />;
    default:
      return null;
  }
}
