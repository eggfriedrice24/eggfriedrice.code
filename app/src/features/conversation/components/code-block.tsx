// A code block of an assistant message (design 3.5, decision D12): our own frame around
// the fence, because enriched 1.1.1 draws no code block header. A box with radius 10
// and a 1 pt border_subtle edge, no fill; a 32 pt header with the language label in
// mono 11 text_subtle and the copy button; a hairline; then the body.
//
// - The body is an enriched text that holds only this fence, for the highlighter, with
//   the frame's style (FENCE_MARKDOWN_STYLE). It uses enriched's CommonMark flavour, so
//   enriched adds no header of its own. Long lines scroll sideways.
// - While the fence is open, the label says "rust · streaming", copy is off, and the
//   yolk code cursor sits under the code.
// - A diff fence shows its lines in the diff view instead of the highlighter.

import type { ReactNode } from "react";
import { ScrollView, View } from "react-native";

import { BlockCursor } from "@/components/ui/block-cursor";
import { CopyButton } from "@/components/ui/copy-button";
import { Hairline } from "@/components/ui/hairline";
import { MonoText } from "@/components/ui/mono-text";
import { FENCE_MARKDOWN_STYLE } from "@/theme/markdown";
import { diffModel } from "../model/diff-view";
import { codeLabel } from "../model/message-segments";
import { DiffHunks } from "./diff-view";
import { MarkdownView } from "./markdown-view";

const GROW = { flexGrow: 1 } as const;

export interface CodeBlockProps {
  /** The language of the fence; "" when it has none. */
  readonly language: string;
  /** The whole fence as Markdown. */
  readonly source: string;
  /** What the copy button copies: the lines inside the fence. */
  readonly body: string;
  readonly closed: boolean;
}

/** A fenced code block in its frame. */
export function CodeBlock(props: CodeBlockProps) {
  return (
    <Frame label={codeLabel(props.language, props.closed)} copy={props.body} closed={props.closed}>
      <ScrollView horizontal contentContainerStyle={GROW} className="px-12 pt-8 pb-10">
        <MarkdownView
          markdown={props.source}
          streaming={!props.closed}
          markdownStyle={FENCE_MARKDOWN_STYLE}
          variant="fence"
        />
      </ScrollView>
      {props.closed ? null : (
        <View className="px-12 pb-10">
          <BlockCursor size="code" mode="streaming" activity={props.body.length} />
        </View>
      )}
    </Frame>
  );
}

export interface DiffFenceProps {
  /** The lines inside the fence. */
  readonly body: string;
}

/** A closed diff fence of a message, in the code block frame. */
export function DiffFence(props: DiffFenceProps) {
  const model = diffModel(props.body, "");
  return (
    <Frame label="diff" copy={props.body} closed>
      <DiffHunks hunks={model.hunks} />
    </Frame>
  );
}

function Frame(props: {
  readonly label: string;
  readonly copy: string;
  readonly closed: boolean;
  readonly children: ReactNode;
}) {
  return (
    <View className="mb-8 overflow-hidden rounded-box border border-border-subtle">
      <View className="h-32 flex-row items-center pr-4 pl-12">
        <MonoText className="flex-1" numberOfLines={1}>
          {props.label}
        </MonoText>
        <CopyButton variant="code" text={props.copy} label="Copy code" disabled={!props.closed} />
      </View>
      <Hairline />
      {props.children}
    </View>
  );
}
