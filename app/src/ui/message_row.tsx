// One assistant message of the transcript (R4c.4 steps 4 to 6 of the research).
//
// The row follows the stream store on its own frame loop: each change of its message
// goes to the pacer as an arrival at the time it came, and on each display frame the row
// takes the pacer's length and commits at most once. A row whose message does not change
// does not render; a settled row runs no frames.
//
// The text goes to one Markdown view, append-only while it streams. A replace (the
// completed text is not an extension of the text so far) gives the message a new
// generation, and the row remounts the view for it, so the fade of the tail never
// misaligns. A long message, and a final one with a diff, is shown as its final blocks
// (blocks.ts), each in a view of its own that never renders again, plus the tail.

import { memo, useCallback, useEffect, useRef, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { MessageView, StreamState } from "@/lib/stream/reducer";
import { semantic } from "@/theme/palette";

import {
  type Block,
  type BlockSplit,
  completeBlocks,
  EMPTY_SPLIT,
  fenceBody,
  splitBlocks,
} from "./blocks";
import { copyText } from "@/lib/native/clipboard";
import { DiffView } from "./diff_view";
import type { ExternalStore } from "@/lib/store/frame-store";
import { frameNow } from "./hooks";
import { MarkdownView } from "./markdown_view";
import { pacedArrive, pacedFrame, type PacedText, startPaced } from "./paced_text";
import { PLAIN } from "./styles";

/**
 * From this length (UTF-16 units) a message shows as blocks. R4c.4 step 6: a reply of 20
 * to 100 KB can lag on a phone when one view parses all of it on each change. The device
 * spike decides the final number.
 */
export const BLOCKS_FROM = 20_000;

const DIFF_FENCE = /^(`{3,}|~{3,}) *(diff|patch|udiff)\b/m;

/** The text to show as final blocks and a tail. */
interface Blocks {
  readonly final: readonly Block[];
  /** The text that the final blocks index into. */
  readonly text: string;
  readonly tail: string;
}

/** What the row shows on a frame. */
interface Shown {
  readonly message: MessageView | undefined;
  readonly text: string;
  /** `undefined` when the message shows as one Markdown view. */
  readonly blocks: Blocks | undefined;
}

function select(stream: StreamState, turnId: string, index: number): MessageView | undefined {
  return stream.turns.get(turnId)?.messages.get(index);
}

function input(message: MessageView) {
  return { text: message.buffer.text, final: message.final, generation: message.generation };
}

/**
 * The blocks of `text`, or `undefined` when the message shows as one view. The split
 * continues `previous`, so a frame scans only the tail.
 */
function blocksOf(
  text: string,
  final: boolean,
  previous: BlockSplit,
): { blocks: Blocks | undefined; split: BlockSplit } {
  if (text.length < BLOCKS_FROM && !(final && DIFF_FENCE.test(text))) {
    return { blocks: undefined, split: previous };
  }
  if (final) {
    return { blocks: { final: completeBlocks(text, previous), text, tail: "" }, split: previous };
  }
  const split = splitBlocks(text, previous);
  return { blocks: { final: split.blocks, text, tail: text.slice(split.tailStart) }, split };
}

/** What the row shows of `message` when it shows `length` units. */
function shownOf(
  message: MessageView | undefined,
  length: number,
  previous: BlockSplit,
): { shown: Shown; split: BlockSplit } {
  if (message === undefined) {
    return { shown: { message, text: "", blocks: undefined }, split: previous };
  }
  const full = message.buffer.text;
  const text = length >= full.length ? full : full.slice(0, length);
  const { blocks, split } = blocksOf(text, message.final && text === full, previous);
  return { shown: { message, text, blocks }, split };
}

/**
 * The message and the part of it to show, on the row's own frame loop. `stream` is the
 * stream store itself, not a frame-gated view, so the pacer sees each arrival at its
 * time.
 */
function usePacedMessage(
  stream: ExternalStore<StreamState>,
  turnId: string,
  index: number,
  reduceMotion: boolean,
): Shown {
  const [view, setView] = useState<Shown>(() => {
    const message = select(stream.getSnapshot(), turnId, index);
    if (message === undefined) {
      return shownOf(message, 0, EMPTY_SPLIT).shown;
    }
    const now = frameNow();
    const paced = startPaced(input(message), now);
    const length = pacedFrame(paced, message.buffer.text, now, reduceMotion).shown;
    return shownOf(message, length, EMPTY_SPLIT).shown;
  });
  // The loop reads Reduce Motion when it runs, so a change of it does not start the loop
  // again (a new loop would start its pacer from nothing).
  const reduceRef = useRef(reduceMotion);
  useEffect(() => {
    reduceRef.current = reduceMotion;
  }, [reduceMotion]);

  useEffect(() => {
    let message: MessageView | undefined;
    let paced: PacedText | undefined;
    let split: BlockSplit = EMPTY_SPLIT;
    let frame: number | undefined;
    const tick = () => {
      frame = undefined;
      if (message === undefined || paced === undefined) {
        return;
      }
      const current = message;
      const result = pacedFrame(paced, current.buffer.text, frameNow(), reduceRef.current);
      paced = result.state;
      const next = shownOf(current, result.shown, split);
      split = next.split;
      setView((previous) =>
        previous.message === current && previous.text === next.shown.text ? previous : next.shown,
      );
      if (!result.settled) {
        frame = requestAnimationFrame(tick);
      }
    };
    const onChange = () => {
      const next = select(stream.getSnapshot(), turnId, index);
      if (next === message) {
        return;
      }
      const now = frameNow();
      if (next !== undefined) {
        if (message !== undefined && next.generation !== message.generation) {
          split = EMPTY_SPLIT;
        }
        paced =
          paced === undefined ? startPaced(input(next), now) : pacedArrive(paced, input(next), now);
      }
      message = next;
      frame ??= requestAnimationFrame(tick);
    };
    const unsubscribe = stream.subscribe(onChange);
    onChange();
    return () => {
      unsubscribe();
      if (frame !== undefined) {
        cancelAnimationFrame(frame);
      }
    };
  }, [stream, turnId, index]);

  return view;
}

/** The row of message `index` of turn `turnId`. */
export const MessageRow = memo(function MessageRow(props: {
  readonly stream: ExternalStore<StreamState>;
  readonly turnId: string;
  readonly index: number;
  readonly reduceMotion: boolean;
}) {
  const view = usePacedMessage(props.stream, props.turnId, props.index, props.reduceMotion);
  const { message } = view;
  if (message === undefined || message.buffer.text.length === 0) {
    return null;
  }
  // A new generation remounts the body and its views.
  return <MessageBody key={message.generation} view={view} message={message} />;
});

function MessageBody(props: { readonly view: Shown; readonly message: MessageView }) {
  const { message, view } = props;
  // Only a message that streamed while the row was on screen fades its tail in.
  const [streamed] = useState(() => !message.final);
  const live = streamed && !message.final;
  const blocks = view.blocks;
  return (
    <View style={styles.row}>
      {blocks === undefined ? (
        <MarkdownView markdown={view.text} streaming={streamed} />
      ) : (
        <>
          {blocks.final.map((block) => (
            <BlockView key={block.start} text={blocks.text} block={block} />
          ))}
          {blocks.tail.length > 0 ? <MarkdownView markdown={blocks.tail} streaming={live} /> : null}
        </>
      )}
      {message.final ? <CopyButton text={message.buffer.text} /> : null}
    </View>
  );
}

/** A final block: it never changes, so its view never renders again. */
const BlockView = memo(
  function BlockView(props: { readonly text: string; readonly block: Block }) {
    const source = props.text.slice(props.block.start, props.block.end).replace(/\n+$/, "");
    if (props.block.kind === "diff") {
      return (
        <View style={styles.diff}>
          <DiffView diff={fenceBody(source)} />
        </View>
      );
    }
    return <MarkdownView markdown={source} streaming={false} />;
  },
  (before, after) =>
    before.block.start === after.block.start &&
    before.block.end === after.block.end &&
    before.block.kind === after.block.kind,
);

function CopyButton(props: { readonly text: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) {
      return undefined;
    }
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);
  const { text } = props;
  const copy = useCallback(() => {
    copyText(text);
    setCopied(true);
  }, [text]);
  return (
    <Pressable
      onPress={copy}
      accessibilityRole="button"
      accessibilityLabel="Copy the message"
      hitSlop={8}
      style={PLAIN}
    >
      <Text style={styles.copyText}>{copied ? "Copied" : "Copy"}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  row: { paddingVertical: 4, gap: 4 },
  diff: { marginBottom: 10 },
  copyText: { color: semantic.text_subtle, fontSize: 13, paddingVertical: 2 },
});
