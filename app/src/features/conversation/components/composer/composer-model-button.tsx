// The model button at the right end of the status line of the composer (S-ModelSheet,
// MS-Switched, AC-Out): what the next message uses, such as "GPT-6.1 Sol · b · medium".
// It names the account when the model runs on two. A tap opens the model sheet. To make
// room on a narrow line, the path goes first, then the button is cut from its end.
//
// It shows nothing until the app knows a model: no list and no turn with a model.

import { useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import { createStore, shallow, type Readable } from "@tanstack/store";
import { Pressable, Text } from "react-native";

import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";
import { modelsQueryOptions } from "../../conversation-queries";
import { composerDraftKey, type ComposerTarget } from "../../model/composer-state";
import { selectModelChoice } from "../../model/model-choices";
import { currentSettings, modelButton } from "../../model/model-sheet";
import { selectLastSettings } from "../../model/selectors";
import type { ComposerDeps } from "../../use-composer-actions";

/** The store of a composer without a conversation: it never changes. */
const NO_STREAM: Readable<StreamState> = createStore(EMPTY_STREAM);

export interface ComposerModelButtonProps {
  readonly deps: ComposerDeps;
  readonly target: ComposerTarget;
  /** The stream store of the conversation; none on the blank root. */
  readonly store?: Readable<StreamState>;
  /** Opens the model sheet of this composer. */
  readonly onPress: () => void;
}

/** The model button. */
export function ComposerModelButton(props: ComposerModelButtonProps) {
  const { deps, target } = props;
  const list = useQuery(modelsQueryOptions(deps.caller, target.machine)).data;
  const choice = useSelector(deps.modelChoices.store, selectModelChoice(composerDraftKey(target)));
  const last = useSelector(props.store ?? NO_STREAM, selectLastSettings, { compare: shallow });
  const current = currentSettings(list, choice, last.model === undefined ? undefined : last);
  const button = modelButton(list, current);
  if (button === undefined) {
    return null;
  }
  return (
    <Pressable
      testID="composer-model-button"
      accessibilityRole="button"
      accessibilityLabel={button.label}
      accessibilityHint="Opens the models of this machine"
      hitSlop={{ top: 7, bottom: 7 }}
      onPress={props.onPress}
      className="ml-auto shrink active:opacity-60"
    >
      <Text numberOfLines={1} ellipsizeMode="tail" className="text-mono-xs text-text-muted">
        {button.text}
      </Text>
    </Pressable>
  );
}
