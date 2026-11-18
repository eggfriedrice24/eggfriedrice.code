// The context of one composer: `Composer.Root` gives it, and the slots read it with
// `use()`. Only the composer renders when its value changes: typing renders the
// composer, never the screen (t3code PR #21261).

import type { Readable } from "@tanstack/store";
import { createContext, type RefObject } from "react";
import type { TextInputInstance } from "react-native";

import type { AttachmentLimits } from "@/lib/protocol/generated";
import type { SendMode } from "@/features/settings/model/preferences";
import type { AttachSource } from "../../model/attachments";
import type { TileView } from "../../model/attachments-tiles";
import type { ComposerState, ComposerTrailing } from "../../model/composer-state";
import type { PhoneMode } from "../../model/model-choices";

/**
 * The question of the model that Send answers (S-Ask 3, "Type an answer"). The text of
 * the field is the answer. It is not the message draft: when the question ends while
 * the user types, the text goes with the answer state, so the next Send never sends it
 * as a message, which would count as the user's own words for efr's rules (ADR 0025).
 * Only the xmark keeps it as a message draft.
 */
export interface ComposerAnswer {
  /** The question on one line, for the answer row. */
  readonly question: string;
  /** The typed answer. */
  readonly draft: Readable<string>;
  /** Changes the typed answer. */
  readonly setText: (text: string) => void;
  /** True while efrd has not taken the answer yet. */
  readonly sending: boolean;
  /** True when the last answer did not arrive: the text stays, and the row says so. */
  readonly failed: boolean;
  /** Sends `text`, a valid answer (`answerText`). The field empties once efrd took it. */
  readonly send: (text: string) => void;
  /** Leaves the answer state: the strip and the status line come back. */
  readonly leave: () => void;
}

/** The files of the draft (board S-Attach): the plus, the tile row and its menus. */
export interface ComposerAttach {
  readonly tiles: readonly TileView[];
  /** The limits of efrd: its capability `attachments`. */
  readonly limits: AttachmentLimits;
  /** How many more files the draft takes; at 0 the plus is off. */
  readonly room: number;
  /** Opens the picker of `source`; its files join the draft. */
  readonly pick: (source: AttachSource) => void;
  /** The xmark or Remove of the tile `key`. */
  readonly remove: (key: string) => void;
  /** Quick Look of the photo tile `key`. */
  readonly quickLook: (key: string) => void;
}

/** What the slots of the composer read. */
export interface ComposerModel {
  /** The question that Send answers now; none for a message. */
  readonly answering: ComposerAnswer | undefined;
  /**
   * The files of the draft. Undefined when the plus does not show: a build without the
   * native parts, an efrd that takes no attachments, or an answer to a question.
   */
  readonly attach: ComposerAttach | undefined;
  /** True while the draft holds a file: a steer takes none, so the capsule is only Queue. */
  readonly queueOnly: boolean;
  readonly state: ComposerState;
  readonly trailing: ComposerTrailing;
  /** True on the blank root: a prompt there starts a new conversation. */
  readonly isBlankRoot: boolean;
  /** The text in the field: the draft, or the prompt that is sent now. */
  readonly value: string;
  readonly placeholder: string;
  /** The last choice of the Steer/Queue menu. */
  readonly sendMode: SendMode;
  /** True while a stop waits for efrd: Stop takes no second tap. */
  readonly stopping: boolean;
  /** Changes the draft. It drops the change while a send is in flight. */
  readonly setText: (text: string) => void;
  /** Sends the text as the trailing action says: send, steer or queue. */
  readonly submit: () => void;
  /** Picks a send mode in the menu: it sticks, and the text goes in that mode. */
  readonly pick: (mode: SendMode) => void;
  /** Stops the running turn. */
  readonly stop: () => void;
  /**
   * The mode of the next message from this phone (S-ComposerR7 8): auto only while the
   * Face ID unlock is on.
   */
  readonly mode: PhoneMode;
  /** Sets the mode of the next messages of this conversation on this phone. */
  readonly setMode: (mode: PhoneMode) => void;
}

/** The context of the composer. Null outside `Composer.Root`. */
export const ComposerContext = createContext<ComposerModel | null>(null);

/**
 * The ref of the composer's text input, apart from the model, so a tap anywhere on the
 * pill can focus it (design R4 item 2). `Composer.Root` gives it.
 */
export const ComposerInputRefContext = createContext<RefObject<TextInputInstance | null> | null>(
  null,
);

/**
 * The size of the composer (as t3code and Claude Code): a one-line pill at rest, the full
 * composer while the input has focus. `Composer.Root` gives it.
 */
export interface ComposerSize {
  /** True while the input has focus: the status line shows and the field grows. */
  readonly expanded: boolean;
  /** The input calls it on focus and on blur. */
  readonly setFocused: (focused: boolean) => void;
}

/** The size of the composer. Outside `Composer.Root` it is always expanded. */
export const ComposerSizeContext = createContext<ComposerSize>({
  expanded: true,
  setFocused: () => undefined,
});
