// Slot A of the composer, the status line (design 3.3, 3.19 and item 8.2): 30 pt high at
// the top of the pill, mono 11 in text_subtle, no wrap. Its 16 pt sides line up with the
// text of the field. The blank root has the line too (a choice of the user), with the
// cwd of a new conversation. It shows the cwd in text_muted and `ctx 42%`, from 90 %
// while a turn runs the warning glyph and the limit of its account (`5-hour 92%`,
// AC-Limits 3), then "waiting for you" in yolk while an approval waits here, and the
// grey braille cell with "reconnecting…" while the connection opens. The mode control
// and the model button stand at its end (composer-mode-button.tsx,
// composer-model-button.tsx).

import { useSelector } from "@tanstack/react-store";
import { createStore, shallow, type Readable } from "@tanstack/store";
import { Fragment, use, type ReactNode } from "react";
import { View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { MonoText } from "@/components/ui/mono-text";
import { SymbolIcon } from "@/components/ui/symbol";
import { useNow } from "@/features/navigation/use-now";
import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";

import { BLANK_STATUS, selectComposerStatus, statusParts } from "../../model/composer-state";
import { ComposerContext, ComposerSizeContext } from "./composer-context";

/** The store of a composer without a conversation: it never changes. */
const NO_STREAM: Readable<StreamState> = createStore(EMPTY_STREAM);

/**
 * The cwd gives way first on a narrow line (AC-Limits 3, AC-Out). Yoga takes the missing
 * room from each part in proportion to its shrink factor times its width, so the factor
 * of the path is so much larger than the 100 of ctx that ctx loses well under 1 pt
 * before the path is gone, and the 1 of the model button loses nothing.
 */
export const CWD_SHRINK = "shrink-[1000000]";

/** ctx gives way after the path and before the model button (S-ComposerR7 6). */
export const CONTEXT_SHRINK = "shrink-[100]";

/** The shrink class of each part that gives way. */
const SHRINKS: Partial<Record<string, string>> = { cwd: CWD_SHRINK, context: CONTEXT_SHRINK };

export interface ComposerStatusLineProps {
  /** The content of the line, such as `Composer.Status`. */
  readonly children?: ReactNode;
}

/** The status line row. It shows only while the composer is expanded (the input has focus). */
export function ComposerStatusLine(props: ComposerStatusLineProps) {
  const model = use(ComposerContext);
  const { expanded } = use(ComposerSizeContext);
  if (model === null || !expanded) {
    return null;
  }
  return (
    <View testID="composer-status-line" className="h-30 flex-row items-center gap-8 px-16">
      {props.children}
    </View>
  );
}

export interface ComposerStatusProps {
  /** The stream store of the conversation. Without it the line has only the connection. */
  readonly store?: Readable<StreamState>;
}

/** The words of the status line, from the stream store and the connection. */
export function ComposerStatus(props: ComposerStatusProps) {
  const model = use(ComposerContext);
  // NOTE: the minute clock drops a window of the limit once its reset passed.
  const now = useNow();
  const status = useSelector(
    props.store ?? NO_STREAM,
    (state) => selectComposerStatus(state, now),
    { compare: shallow },
  );
  const parts = statusParts(model?.isBlankRoot === true ? BLANK_STATUS : status);
  const reconnecting = model?.state.kind === "reconnecting";
  return (
    <>
      {parts.map((part, index) => {
        const shrink = SHRINKS[part.key];
        const text = (
          <MonoText
            tone={part.tone}
            numberOfLines={1}
            ellipsizeMode="middle"
            className={shrink === undefined ? "" : "shrink"}
          >
            {part.text}
          </MonoText>
        );
        // A part that gives way holds its own "·" after it, so the dot goes with the
        // part and no lone "·" stays (S-ComposerR7 6).
        const trailing = index < parts.length - 1 || reconnecting;
        return (
          <Fragment key={part.key}>
            {part.key === "limit" ? (
              <SymbolIcon name="exclamationmark.triangle" size={10} tone="state_warning" />
            ) : null}
            {shrink === undefined ? (
              <>
                {text}
                {trailing ? <Dot /> : null}
              </>
            ) : (
              <View
                testID={`composer-status-${part.key}`}
                className={`min-w-0 flex-row items-center gap-8 overflow-hidden ${shrink}`}
              >
                {text}
                {trailing ? <Dot /> : null}
              </View>
            )}
          </Fragment>
        );
      })}
      {reconnecting ? (
        <>
          <View
            accessible
            role="status"
            accessibilityLiveRegion="polite"
            className="flex-row items-center gap-8"
          >
            <BrailleCell tint="wait" />
            <MonoText tone="text_muted" numberOfLines={1}>
              reconnecting…
            </MonoText>
          </View>
        </>
      ) : null}
    </>
  );
}

function Dot() {
  return (
    <MonoText accessibilityElementsHidden importantForAccessibility="no">
      ·
    </MonoText>
  );
}
