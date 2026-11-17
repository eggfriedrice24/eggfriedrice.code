// The composer (plan section 2.6, design 3.3): one compound component whose state comes
// from the pure functions of model/composer-state.ts.
//
//   <Composer.Root deps={deps} target={target} connection={connection} machineName="laptop">
//     {strip}                                  the approval strip while a question waits
//     <Composer.StatusLine>                    slot A: cwd · ctx · waiting / reconnecting
//       <Composer.Status store={stream.store} />
//       <ComposerModeButton ... />             the mode control, then
//       <ComposerModelButton ... />            the model button at its end
//     </Composer.StatusLine>
//     <Composer.Body>                          at rest one row, focused a column:
//       attach (slot B, off for now)
//       <Composer.Input />                     slot C: reads its own draft
//       voice (off for now)
//       <Composer.Trailing />                  slot D: Send | Stop | Stop + Steer/Queue | sending
//     </Composer.Body>
//   </Composer.Root>                           slot E: the bottom edge, or 6 pt over the keyboard
//
// `DockedComposer` is this tree with the slots of v1. While the user types an answer to a
// question of the model (S-Ask 3), the answer row takes the place of the strip and the
// status line, and the fixed hint stands under the body (composer-answer-row.tsx).

import { use, type ReactNode } from "react";
import { View } from "react-native";

import type { Readable } from "@tanstack/store";

import type { StreamState } from "@/lib/stream/reducer";

import { ComposerAnswerHint, ComposerAnswerRow } from "./composer-answer-row";
import { ComposerSizeContext } from "./composer-context";
import { ComposerInput } from "./composer-input";
import { ComposerAttachButton, ComposerVoiceButton } from "./composer-media-buttons";
import { ComposerModeButton } from "./composer-mode-button";
import { ComposerModelButton } from "./composer-model-button";
import { ComposerRoot, type ComposerRootProps } from "./composer-root";
import { ComposerStatus, ComposerStatusLine } from "./composer-status-line";
import { ComposerTrailing } from "./composer-trailing";

/**
 * The field and the actions, in the two sizes of the composer (as t3code):
 *
 * - At rest, one row: attach, the field, voice, then the trailing action, gap 8, aligned
 *   at the bottom. With 4 pt above and under the 40 pt field, the pill is 48 pt high.
 *   The 8 pt on each side put the 32 pt buttons in the middle of the round ends.
 * - Focused, a column: the field over the full width under the status line, so its text
 *   starts at the top left with the 16 pt sides of the line, then a row of the actions:
 *   attach at the left, voice and the trailing action at the right.
 *
 * The field keeps its place among the children in both sizes, so React keeps the same
 * native input and the focus stays when the size changes.
 */
export function ComposerBody() {
  const { expanded } = use(ComposerSizeContext);
  return (
    <>
      <View className={expanded ? "pb-4" : "flex-row items-end gap-8 px-8 py-4"}>
        {expanded ? null : <ComposerAttachButton />}
        <ComposerInput />
        {expanded ? null : <ComposerVoiceButton />}
        {expanded ? null : <ComposerTrailing />}
        {expanded ? (
          <View testID="composer-actions" className="flex-row items-center gap-8 px-8">
            <ComposerAttachButton />
            <View className="flex-1" />
            <ComposerVoiceButton />
            <ComposerTrailing />
          </View>
        ) : null}
      </View>
      <ComposerAnswerHint />
    </>
  );
}

/** The slots of the composer. */
export const Composer = {
  Root: ComposerRoot,
  StatusLine: ComposerStatusLine,
  Status: ComposerStatus,
  Body: ComposerBody,
  Input: ComposerInput,
  Trailing: ComposerTrailing,
} as const;

export interface DockedComposerProps extends Omit<ComposerRootProps, "children"> {
  /**
   * The stream store of the conversation, for the status line. None on the blank root,
   * where the line shows the cwd of a new conversation.
   */
  readonly stream?: Readable<StreamState>;
  /** The row above the status line: the approval strip while a question waits. */
  readonly strip?: ReactNode;
  /** Opens the model sheet. Without it the status line has no model button. */
  readonly onOpenModels?: () => void;
}

/**
 * The composer: the strip, the status line with the model button at its end, the field
 * and the trailing action.
 */
export function DockedComposer(props: DockedComposerProps) {
  const { stream, strip, onOpenModels, ...root } = props;
  // S-Ask 3: while the user types an answer, the answer row takes the place of the strip
  // and the status line. The body keeps its place, so the field keeps its focus.
  const answering = root.answering !== undefined;
  return (
    <ComposerRoot {...root}>
      {answering ? <ComposerAnswerRow /> : strip}
      {answering ? null : (
        <ComposerStatusLine>
          {stream === undefined ? <ComposerStatus /> : <ComposerStatus store={stream} />}
          {/* The two settings of the next message stand together at the right end. */}
          <View className="ml-auto shrink flex-row items-center gap-10">
            <ComposerModeButton deps={root.deps} />
            {onOpenModels === undefined ? null : stream === undefined ? (
              <ComposerModelButton
                deps={root.deps}
                target={root.target}
                machineName={root.machineName}
                onPress={onOpenModels}
              />
            ) : (
              <ComposerModelButton
                deps={root.deps}
                target={root.target}
                machineName={root.machineName}
                store={stream}
                onPress={onOpenModels}
              />
            )}
          </View>
        </ComposerStatusLine>
      )}
      <ComposerBody />
    </ComposerRoot>
  );
}
