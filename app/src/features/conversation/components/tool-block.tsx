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
// - While the call's approval waits (design R4 item 5, S-Tools round 8): the border_accent
//   outline, a still yolk dot, "waiting for approval" and a chevron where the loader and
//   the time go, and the seconds left under a minute. The whole card is a button that
//   opens the approval sheet; it never opens by itself. After the answer only the status
//   word changes: "allowed", then the usual loader, or "denied" or "expired" with the
//   command in text_muted. Who answered and when are for VoiceOver.
// - In auto, a quiet mono line under the header gives the verdict on the exit of the
//   call and what it got (AU-Auto), and one line per changed file that runs code says
//   whether it moved to quarantine or stayed in place (Q-Quarantine 7).
// - The writing block shows a tool input that the model still writes: "writing patch",
//   the braille cell and the size that grows.

import { Pressable, Text, View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { cn } from "@/components/ui/cn";
import { Dot } from "@/components/ui/dot";
import { Hairline } from "@/components/ui/hairline";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { PlainButton } from "@/components/ui/plain-button";
import { SymbolIcon } from "@/components/ui/symbol";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type { ToolCallView, ToolInputView } from "@/lib/stream/reducer";
import {
  moreLabel,
  type OutputLine,
  toolHeader,
  toolLabel,
  toolOutput,
  type ToolApproval,
  type ToolStatus,
  toolStatus,
  writingBlock,
} from "../model/tool-block";
import type { ExitNote } from "../model/exit-words";
import { FadeIn } from "./fade-in";
import { useNow } from "./use-now";

export interface ToolBlockViewProps {
  readonly call: ToolCallView;
  /** True when the turn of the call has ended: a call still open then failed. */
  readonly turnEnded: boolean;
  /** The approval question of the call, if efrd asked one. */
  readonly approval?: ToolApproval | undefined;
  /** The time left to answer under a minute, such as `42s`. */
  readonly left?: string | undefined;
  readonly expanded: boolean;
  readonly onToggle: () => void;
  /** Opens the approval sheet of the call. */
  readonly onOpenApproval?: (() => void) | undefined;
  /** The line about the exit of the call in auto: the verdict and what the call got. */
  readonly note?: ExitNote | undefined;
  /** True when the pause of the reviewer stopped the turn at this call. */
  readonly stopped?: boolean | undefined;
  /** The quiet lines of the files that run code which the call changed (quarantine). */
  readonly surface?: readonly string[] | undefined;
}

/** A tool block. */
export function ToolBlockView(props: ToolBlockViewProps) {
  const { call, expanded, approval, left, note, stopped } = props;
  const status0 = toolStatus(call, props.turnEnded, 0, approval, left, stopped);
  // NOTE: "allowed" turns into the loader 2 s after the answer, so the clock runs then.
  const running = status0.kind === "running" || status0.kind === "allowed";
  const waiting = status0.kind === "waiting";
  const closed = status0.kind === "closed";
  const now = useNow(running);
  const header = toolHeader(call);
  const status = toolStatus(call, props.turnEnded, now, approval, left, stopped);
  const output = closed ? NO_OUTPUT : toolOutput(call, expanded);
  const more = moreLabel(output, expanded);
  const open = props.onOpenApproval;
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
    <MenuView entries={entries} onAction={copy} openOnLongPress testID="tool-menu">
      <View>
        <View
          testID="tool-block"
          className={cn(
            "overflow-hidden rounded-box border",
            waiting ? "border-border-accent" : "border-border-subtle",
          )}
        >
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={toolLabel(header, status, approval)}
            accessibilityState={waiting ? {} : { expanded }}
            accessibilityHint={
              waiting
                ? "Opens the approval."
                : expanded
                  ? "Shows less output."
                  : "Shows all the output."
            }
            onPress={waiting ? open : props.onToggle}
            className={cn(
              "flex-row gap-8 px-12",
              header.wraps ? "items-start py-9" : "min-h-34 items-center",
              waiting && "active:bg-selection-bg",
            )}
          >
            <Text className="text-mono-xs text-text-subtle">{header.verb}</Text>
            {header.wraps ? (
              <Text className={cn("flex-1 text-mono-xs", subjectTone(closed))}>
                {header.subject}
              </Text>
            ) : (
              <Text
                numberOfLines={1}
                ellipsizeMode="middle"
                className={cn("flex-1 text-mono-xs", subjectTone(closed))}
              >
                {header.subject}
              </Text>
            )}
            <StatusMark status={status} />
          </Pressable>
          {note === undefined ? null : <ExitLine note={note} />}
          {props.surface === undefined || props.surface.length === 0 ? null : (
            <View testID="surface-lines" className="-mt-4 px-12 pb-9">
              {props.surface.map((line) => (
                <Text key={line} className="text-mono-xs text-text-subtle">
                  {line}
                </Text>
              ))}
            </View>
          )}
          {output.lines.length > 0 ? (
            <>
              <Hairline />
              {/* A tap on the output toggles too; VoiceOver reads its lines one by one. */}
              <Pressable accessible={false} onPress={props.onToggle} className="px-12 pt-7 pb-8">
                {/* Only new lines of a running call fade in: a block that the list
                  recycles or that the user opens shows its lines at once. */}
                {output.lines.map((line) =>
                  running ? (
                    <FadeIn key={line.key}>
                      <OutputText line={line} wraps={expanded} />
                    </FadeIn>
                  ) : (
                    <OutputText key={line.key} line={line} wraps={expanded} />
                  ),
                )}
                {output.capped ? (
                  <Text className="text-mono-xs text-text-subtle">
                    {`showing the last 200 of ${String(output.total)} lines`}
                  </Text>
                ) : null}
              </Pressable>
            </>
          ) : null}
          {more === undefined ? null : (
            <>
              <Hairline />
              <PlainButton label={more} height="row" wide onPress={props.onToggle} />
            </>
          )}
        </View>
      </View>
    </MenuView>
  );
}

/**
 * The line about the exit under the header (AU-Auto): mono 11, wrapped, never cut. The
 * whole network, the loudest grant, leads with the warning glyph.
 */
function ExitLine(props: { readonly note: ExitNote }) {
  const { note } = props;
  return (
    <View testID="exit-note" className="-mt-4 flex-row gap-4 px-12 pb-9">
      {note.loud ? (
        <View className="pt-2">
          <SymbolIcon name="exclamationmark.triangle" size={11} tone="accent_primary" />
        </View>
      ) : null}
      <Text
        className={cn(
          "flex-1 text-mono-xs",
          note.tone === "text_muted" ? "text-text-muted" : "text-text-subtle",
        )}
      >
        {note.text}
      </Text>
    </View>
  );
}

/** A call that did not run shows no output. */
const NO_OUTPUT = { lines: [], total: 0, capped: false } as const;

/** The command of a call that did not run is text_muted. */
function subjectTone(closed: boolean): string {
  return closed ? "text-text-muted" : "text-text-bright";
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
  if (status.kind === "waiting") {
    return (
      <View className="flex-row items-center gap-6">
        <Dot size="large" />
        <Text className="text-mono-xs text-accent-primary">waiting for approval</Text>
        {status.left === undefined ? null : (
          <Text className="text-mono-xs text-accent-primary">{status.left}</Text>
        )}
        <SymbolIcon name="chevron.right" size={10} tone="text_muted" weight="heavy" />
      </View>
    );
  }
  if (status.kind === "reviewing") {
    return (
      <View className="flex-row items-center gap-6">
        <BrailleCell label="Reviewing" />
        <Text className="text-mono-xs text-text-subtle">reviewing</Text>
      </View>
    );
  }
  if (status.kind === "allowed" || status.kind === "closed") {
    return (
      <Text className="text-mono-xs text-text-muted">
        {status.kind === "allowed" ? "allowed" : status.word}
      </Text>
    );
  }
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
