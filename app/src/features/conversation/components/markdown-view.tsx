// One Markdown text of the transcript (design 3.5): react-native-enriched-markdown in its
// GitHub flavour, with the style of theme/markdown.ts. Native Text runs, no web view.
//
// - Enriched loads images and makes a `<video src>` HTML block into a video, with no tap.
//   So the message row gives only inert Markdown (inertMarkdown of
//   model/message-segments.ts): an image is a link, and no line begins an HTML block.
//   Inline HTML is text (enriched sets MD_FLAG_NOHTMLSPANS).
// - Math is off, and the system's link preview is off, because it would load the page.
//   A tap on a link asks first, then opens it in SFSafariViewController (model/links.ts
//   and lib/native/links.ts).
// - While the text streams, `streaming` turns on enriched's fade of the appended tail
//   (design 4: the streaming tail), unless Reduce Motion is on. The text must then only
//   grow: the message row gives each segment a view of its own, and remounts the
//   message when its text is replaced.
// - A message uses enriched's GitHub flavour (tables, task lists). The body of a code
//   block frame uses the CommonMark flavour, which draws no code block header of its own.
// - The React Compiler keeps the native element while the text and the flags stay the
//   same, so a final segment never parses its text again.

import {
  EnrichedMarkdownText,
  type MarkdownStyle,
  type Md4cFlags,
  type StreamingConfig,
} from "react-native-enriched-markdown";

import { openLink } from "@/lib/native/links";
import { useReduceMotion } from "@/lib/native/reduce-motion";
import { MARKDOWN_STYLE } from "@/theme/markdown";
import { roles } from "@/theme/roles";
import { linkAction } from "../model/links";

const FLAGS: Md4cFlags = {
  latexMath: false,
  underline: false,
  superscript: false,
  subscript: false,
  highlight: false,
  hardSoftBreaks: false,
  preserveBlankLines: false,
  admonitions: true,
};

const STREAMING: StreamingConfig = { tableMode: "progressive", codeBlockMode: "progressive" };

const CONTAINER = { flexShrink: 1 } as const;

export interface MarkdownViewProps {
  readonly markdown: string;
  /** True while the text grows: the appended tail fades in. */
  readonly streaming: boolean;
  /** The style; the default is the style of every assistant message. */
  readonly markdownStyle?: MarkdownStyle;
  /**
   * `fence` for the body of our code block frame: enriched's CommonMark flavour draws a
   * fence as styled text, without the header bar that its GitHub flavour adds, so the
   * frame's header is the only one.
   */
  readonly variant?: "message" | "fence";
}

/** A Markdown text of the transcript. */
export function MarkdownView(props: MarkdownViewProps) {
  const reduceMotion = useReduceMotion();
  return (
    <EnrichedMarkdownText
      markdown={props.markdown}
      flavor={props.variant === "fence" ? "commonmark" : "github"}
      markdownStyle={props.markdownStyle ?? MARKDOWN_STYLE}
      md4cFlags={FLAGS}
      streamingAnimation={props.streaming && !reduceMotion}
      streamingConfig={STREAMING}
      onLinkPress={(event) => openLink(linkAction(event.url))}
      enableLinkPreview={false}
      enableTaskListItemToggle={false}
      selectable
      selectionColor={roles.selection_bg}
      containerStyle={CONTAINER}
    />
  );
}
