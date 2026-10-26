// Slot A of the composer, the status line (design 3.3, 3.19 and item 8.2): 30 pt high at
// the top of the pill, mono 11 in text_subtle, no wrap. Its 16 pt sides line up with the
// text of the field. The blank root has the line too (a choice of the user), with the
// cwd and the mode of a new conversation. It shows the cwd in text_muted, the mode and
// `ctx 42%`, then "waiting for you" in yolk while an approval waits here, and the grey
// braille cell with "reconnecting…" while the connection opens. Later the line holds
// chips and the mode and model pickers.

import { useSelector } from "@tanstack/react-store";
import { createStore, shallow, type Readable } from "@tanstack/store";
import { Fragment, use, type ReactNode } from "react";
import { View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { MonoText } from "@/components/ui/mono-text";
import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";

import { BLANK_STATUS, selectComposerStatus, statusParts } from "../../model/composer-state";
import { ComposerContext } from "./composer-context";

/** The store of a composer without a conversation: it never changes. */
const NO_STREAM: Readable<StreamState> = createStore(EMPTY_STREAM);

export interface ComposerStatusLineProps {
  /** The content of the line, such as `Composer.Status`. */
  readonly children?: ReactNode;
}

/** The status line row. */
export function ComposerStatusLine(props: ComposerStatusLineProps) {
  const model = use(ComposerContext);
  if (model === null) {
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
  const status = useSelector(props.store ?? NO_STREAM, selectComposerStatus, { compare: shallow });
  const parts = statusParts(model?.isBlankRoot === true ? BLANK_STATUS : status);
  const reconnecting = model?.state.kind === "reconnecting";
  return (
    <>
      {parts.map((part, index) => (
        <Fragment key={part.key}>
          {index > 0 ? <Dot /> : null}
          <MonoText
            tone={part.tone}
            numberOfLines={1}
            ellipsizeMode="middle"
            className={part.key === "cwd" ? "shrink" : ""}
          >
            {part.text}
          </MonoText>
        </Fragment>
      ))}
      {reconnecting ? (
        <>
          {parts.length > 0 ? <Dot /> : null}
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
