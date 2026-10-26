// A tool block of the transcript (design 3.7 and the S-Tools sheet): a box with a 1 pt
// border_subtle edge and no fill, one mono 11 header line (the verb in text_subtle, the
// subject in text_bright, then the status), and the last 3 lines of the output.
//
// - Status: the braille cell in yolk and the elapsed time while it runs; ✓ and the exit
//   code and time when done; ✗ and the exit code when it failed. Always a glyph and a
//   number or a word, never colour alone.
// - A tap expands the whole output in place (at most 200 lines); nothing is pushed.
//   The open state lives in the row state of the conversation, because the list
//   recycles row views.
// - New output lines fade in at the bottom; with Reduce Motion they appear at once.
// - A long press opens Copy command and Copy output.
// - The writing block shows a tool input that the model still writes: "writing patch",
//   the braille cell and the size that grows.

import { Pressable, Text, View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { cn } from "@/components/ui/cn";
import { Hairline } from "@/components/ui/hairline";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { PlainButton } from "@/components/ui/plain-button";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type { ToolCallView, ToolInputView } from "@/lib/stream/reducer";
import {
  moreLabel,
  type OutputLine,
  toolHeader,
  toolLabel,
  toolOutput,
  type ToolStatus,
  toolStatus,
  writingBlock,
} from "../model/tool-block";
import { FadeIn } from "./fade-in";
import { useNow } from "./use-now";

export interface ToolBlockViewProps {
  readonly call: ToolCallView;
  /** True when the turn of the call has ended: a call still open then failed. */
  readonly turnEnded: boolean;
  readonly expanded: boolean;
  readonly onToggle: () => void;
}

/** A tool block. */
export function ToolBlockView(props: ToolBlockViewProps) {
  const { call, expanded } = props;
  const running = !call.done && !props.turnEnded;
  const now = useNow(running);
  const header = toolHeader(call);
  const status = toolStatus(call, props.turnEnded, now);
  const output = toolOutput(call, expanded);
  const more = moreLabel(output, expanded);
  const outputText = call.output ?? call.tail;
  const entries: MenuEntry[] = [
    {
      id: "command",
      title: header.verb === "$" ? "Copy command" : "Copy input",
      image: "doc.on.doc",
    },
  ];
  if (outputText.length > 0) {
    entries.push({ id: "output", title: "Copy output", image: "doc.on.doc" });
  }
  const copy = (id: string): void => {
    void copyText(id === "output" ? outputText : call.summary);
    haptic("light");
  };
  return (
    <MenuView entries={entries} onAction={copy} trigger="long-press" testID="tool-menu">
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={toolLabel(header, status)}
        accessibilityState={{ expanded }}
        accessibilityHint={expanded ? "Shows less output." : "Shows all the output."}
        onPress={props.onToggle}
        className="overflow-hidden rounded-box border border-border-subtle"
      >
        <View
          className={cn(
            "flex-row gap-8 px-12",
            header.wraps ? "items-start py-9" : "min-h-34 items-center",
          )}
        >
          <Text className="text-mono-xs text-text-subtle">{header.verb}</Text>
          {header.wraps ? (
            <Text className="flex-1 text-mono-xs text-text-bright">{header.subject}</Text>
          ) : (
            <Text
              numberOfLines={1}
              ellipsizeMode="middle"
              className="flex-1 text-mono-xs text-text-bright"
            >
              {header.subject}
            </Text>
          )}
          <StatusMark status={status} />
        </View>
        {output.lines.length > 0 ? (
          <>
            <Hairline />
            <View className="px-12 pt-7 pb-8">
              {output.lines.map((line) => (
                <FadeIn key={line.key}>
                  <OutputText line={line} wraps={expanded} />
                </FadeIn>
              ))}
              {output.capped ? (
                <Text className="text-mono-xs text-text-subtle">
                  {`showing the last 200 of ${String(output.total)} lines`}
                </Text>
              ) : null}
            </View>
          </>
        ) : null}
        {more === undefined ? null : (
          <>
            <Hairline />
            <PlainButton label={more} height="row" wide onPress={props.onToggle} />
          </>
        )}
      </Pressable>
    </MenuView>
  );
}

function OutputText(props: { readonly line: OutputLine; readonly wraps: boolean }) {
  const spans = props.line.spans.map((span) =>
    span.error ? (
      <Text key={span.start} className="text-state-error">
        {span.text}
      </Text>
    ) : (
      span.text
    ),
  );
  const empty = props.line.text === "" ? " " : null;
  if (props.wraps) {
    return (
      <Text className="text-mono-xs text-text-subtle">
        {spans}
        {empty}
      </Text>
    );
  }
  return (
    <Text numberOfLines={1} ellipsizeMode="clip" className="text-mono-xs text-text-subtle">
      {spans}
      {empty}
    </Text>
  );
}

/** The glyph and the number of a status. */
export function StatusMark(props: { readonly status: ToolStatus }) {
  const status = props.status;
  if (status.kind === "running") {
    return (
      <View className="flex-row items-center gap-6">
        <BrailleCell label="Running" />
        {status.elapsed === undefined ? null : (
          <Text className="text-mono-xs text-text-subtle">{status.elapsed}</Text>
        )}
      </View>
    );
  }
  const done = status.kind === "done";
  return (
    <Text className="text-mono-xs text-text-subtle">
      <Text className={done ? "text-state-success" : "text-state-error"}>{done ? "✓" : "✗"}</Text>
      {` ${status.detail}`}
    </Text>
  );
}

export interface WritingBlockViewProps {
  readonly input: ToolInputView;
}

/** A tool input that the model still writes. Only its size grows. */
export function WritingBlockView(props: WritingBlockViewProps) {
  const block = writingBlock(props.input);
  return (
    <View
      accessible
      accessibilityLabel={`${block.verb}, ${block.size}`}
      className="min-h-34 flex-row items-center gap-8 rounded-box border border-border-subtle px-12"
    >
      <Text className="flex-1 text-mono-xs text-text-subtle">{block.verb}</Text>
      <BrailleCell />
      <Text className="text-mono-xs text-text-subtle">{block.size}</Text>
    </View>
  );
}
