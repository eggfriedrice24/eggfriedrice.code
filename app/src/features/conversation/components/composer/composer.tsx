// The composer (plan section 2.6, design 3.3): one compound component whose state comes
// from the pure functions of model/composer-state.ts.
//
//   <Composer.Root deps={deps} target={target} connection={connection} machineName="laptop">
//     <Composer.StatusLine>                    slot A: cwd · mode · ctx · waiting / reconnecting
//       <Composer.Status store={stream.store} />
//     </Composer.StatusLine>
//     <Composer.Row>
//       <Composer.Leading />                   slot B: 0 pt in v1
//       <Composer.Input />                     slot C: reads its own draft
//       <Composer.Trailing />                  slot D: Send | Stop | Stop + Steer/Queue | sending
//     </Composer.Row>
//   </Composer.Root>                           slot E: the bottom edge, or 6 pt over the keyboard
//
// `DockedComposer` is this tree with the slots of v1.

import type { ReactNode } from "react";
import { View } from "react-native";

import type { Readable } from "@tanstack/store";

import type { StreamState } from "@/lib/stream/reducer";

import { ComposerInput } from "./composer-input";
import { ComposerRoot, type ComposerRootProps } from "./composer-root";
import { ComposerStatus, ComposerStatusLine } from "./composer-status-line";
import { ComposerTrailing } from "./composer-trailing";

export interface ComposerRowProps {
  readonly children: ReactNode;
}

/**
 * The row of the field and the actions: gap 8, aligned at the bottom as the field grows.
 * With 4 pt above and under the 40 pt field, a one-line pill is 48 pt high. The 8 pt on
 * the right put the 32 pt action in the middle of the round end of the pill.
 */
export function ComposerRow(props: ComposerRowProps) {
  return <View className="flex-row items-end gap-8 py-4 pr-8">{props.children}</View>;
}

export interface ComposerLeadingProps {
  /** Later an attach button (a 32 pt circle, as the actions). */
  readonly children?: ReactNode;
}

/** Slot B. It is empty in v1 and takes no width, so the row adds no gap for it. */
export function ComposerLeading(props: ComposerLeadingProps) {
  return props.children ?? null;
}

/** The slots of the composer. */
export const Composer = {
  Root: ComposerRoot,
  StatusLine: ComposerStatusLine,
  Status: ComposerStatus,
  Row: ComposerRow,
  Leading: ComposerLeading,
  Input: ComposerInput,
  Trailing: ComposerTrailing,
} as const;

export interface DockedComposerProps extends Omit<ComposerRootProps, "children"> {
  /** The stream store of the conversation, for the status line. None on the blank root. */
  readonly stream?: Readable<StreamState>;
}

/** The composer of v1: the status line, the field and the trailing action. */
export function DockedComposer(props: DockedComposerProps) {
  const { stream, ...root } = props;
  return (
    <ComposerRoot {...root}>
      <ComposerStatusLine>
        {stream === undefined ? <ComposerStatus /> : <ComposerStatus store={stream} />}
      </ComposerStatusLine>
      <ComposerRow>
        <ComposerLeading />
        <ComposerInput />
        <ComposerTrailing />
      </ComposerRow>
    </ComposerRoot>
  );
}
