// The context of one composer: `Composer.Root` gives it, and the slots read it with
// `use()`. Only the composer renders when its value changes: typing renders the
// composer, never the screen (t3code PR #21261).

import { createContext, type RefObject } from "react";
import type { TextInputInstance } from "react-native";

import type { SendMode } from "@/features/settings/model/preferences";
import type { ComposerState, ComposerTrailing } from "../../model/composer-state";

/** What the slots of the composer read. */
export interface ComposerModel {
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
