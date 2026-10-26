// One Markdown text of the transcript: react-native-enriched-markdown in its GitHub
// flavor (tables, task lists, code blocks with a copy button), with the style of the
// design system. Math is off, raw HTML is never rendered, and the system's link preview
// is off, because it would load the page. A tap on a link asks first (links.ts).
//
// While a message streams, `streaming` turns on the fade of the appended tail and the
// progressive tables and code blocks. The text must then only grow; the row remounts
// the view when the text is replaced.

import { memo, useCallback } from "react";
import { Alert, Linking } from "react-native";
import {
  EnrichedMarkdownText,
  type LinkPressEvent,
  type Md4cFlags,
  type StreamingConfig,
} from "react-native-enriched-markdown";

import { MARKDOWN_STYLE } from "@/theme/markdown";
import { semantic } from "@/theme/palette";

import { linkAction } from "./links";

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

/** Asks before it opens `url`, and opens only web and mail addresses. */
export function openLink(url: string): void {
  const action = linkAction(url);
  if (action.kind === "refuse") {
    Alert.alert("efr did not open the link", `${action.reason}\n\n${action.url}`);
    return;
  }
  Alert.alert(
    action.host === "" ? "Write an email?" : `Open ${action.host}?`,
    action.url,
    [
      { text: "Cancel", style: "cancel" },
      {
        text: "Open",
        onPress: () => {
          Linking.openURL(action.url).catch(() => {
            Alert.alert("The link did not open", action.url);
          });
        },
      },
    ],
    { cancelable: true },
  );
}

/** A Markdown text of the transcript. */
export const MarkdownView = memo(function MarkdownView(props: {
  readonly markdown: string;
  readonly streaming: boolean;
}) {
  const onLinkPress = useCallback((event: LinkPressEvent) => openLink(event.url), []);
  return (
    <EnrichedMarkdownText
      markdown={props.markdown}
      flavor="github"
      markdownStyle={MARKDOWN_STYLE}
      md4cFlags={FLAGS}
      streamingAnimation={props.streaming}
      streamingConfig={STREAMING}
      onLinkPress={onLinkPress}
      enableLinkPreview={false}
      enableTaskListItemToggle={false}
      selectable
      selectionColor={semantic.selection_bg}
      containerStyle={CONTAINER}
    />
  );
});
