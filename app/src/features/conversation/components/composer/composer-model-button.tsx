// The model button at the right end of the status line of the composer (S-ModelSheet,
// MS-Switched, AC-Out): what the next message uses, such as "GPT-6.1 Sol · b · medium".
// It names the account when the model runs on two. A 9 pt chevron follows the text. The
// button is text_muted, or text_primary when the user chose the model for this
// conversation. A tap opens the model sheet. To make room on a narrow line, the path
// goes first (its shrink factor is much larger), then the button is cut from its end.
//
// Round 7 (S-ComposerR7 1, 2): the mark of the account stands before the name at 11 pt,
// 5 pt from it: the Claude spark in its own orange, the OpenAI mark in the colour of the
// button; another provider has no mark and no gap. A signed-out account makes the words
// text_subtle and the mark 40 %. A new thread shows the favourite of the last plan.
//
// It shows nothing until the app knows a model: no list and no turn with a model.

import { useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import { createStore, shallow, type Readable } from "@tanstack/store";
import { Image, Pressable, Text } from "react-native";

import { cn } from "@/components/ui/cn";
import { SymbolIcon } from "@/components/ui/symbol";

import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";
import { roles } from "@/theme/roles";
import { modelsQueryOptions } from "../../conversation-queries";
import type { ProviderLogoKind } from "../../model/accounts";
import { composerDraftKey, type ComposerTarget } from "../../model/composer-state";
import {
  favouriteOf,
  selectFavourites,
  selectModelChoice,
  selectPlan,
} from "../../model/model-choices";
import {
  currentSettings,
  freshChoice,
  modelButton,
  type ModelButtonView,
} from "../../model/model-sheet";
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
  const favourites = useSelector(deps.modelChoices.store, selectFavourites);
  const plan = useSelector(deps.modelChoices.store, selectPlan(target.machine));
  const last = useSelector(props.store ?? NO_STREAM, selectLastSettings, { compare: shallow });
  const fresh = target.isBlankRoot
    ? freshChoice(list, plan, (provider) => favouriteOf(favourites, target.machine, provider))
    : undefined;
  const current = currentSettings(list, choice, last.model === undefined ? undefined : last, fresh);
  const button = modelButton(list, current);
  if (button === undefined) {
    return null;
  }
  const tone = toneOf(button);
  return (
    <Pressable
      testID="composer-model-button"
      accessibilityRole="button"
      accessibilityLabel={button.label}
      accessibilityHint="Opens the models of this machine"
      hitSlop={{ top: 7, bottom: 7 }}
      onPress={props.onPress}
      className="shrink flex-row items-center gap-4 active:opacity-60"
    >
      <ButtonMark logo={button.logo} dim={button.dim} tone={tone} />
      <Text
        numberOfLines={1}
        ellipsizeMode="tail"
        className={cn("shrink text-mono-xs", TEXT[tone])}
      >
        {button.text}
      </Text>
      <SymbolIcon name="chevron.down" size={9} weight="semibold" tone={tone} />
    </Pressable>
  );
}

type ButtonTone = "text_primary" | "text_muted" | "text_subtle";

/** The text class of each tone of the button. */
const TEXT: Readonly<Record<ButtonTone, string>> = {
  text_primary: "text-text-primary",
  text_muted: "text-text-muted",
  text_subtle: "text-text-subtle",
};

/** The colour of the button: subtle when signed out, primary when chosen here. */
function toneOf(button: ModelButtonView): ButtonTone {
  if (button.dim) {
    return "text_subtle";
  }
  return button.chosen ? "text_primary" : "text_muted";
}

// Metro resolves the images by these paths, as provider-logo.tsx does.
const CLAUDE: number = require("../../../../../assets/providers/claude.png");
const OPENAI: number = require("../../../../../assets/providers/openai.png");

interface ButtonMarkProps {
  readonly logo: ProviderLogoKind;
  readonly dim: boolean;
  readonly tone: ButtonTone;
}

/** The 11 pt mark before the name; 5 pt from it (the 4 pt gap and 1 pt of margin). */
function ButtonMark(props: ButtonMarkProps) {
  if (props.logo === "none") {
    return null;
  }
  return (
    <Image
      testID="composer-model-mark"
      accessibilityElementsHidden
      importantForAccessibility="no"
      source={props.logo === "claude" ? CLAUDE : OPENAI}
      // The Claude spark keeps its own orange; the OpenAI mark takes the colour of the text.
      tintColor={props.logo === "openai" ? roles[props.tone] : undefined}
      className={cn("mr-1 h-11 w-11", props.dim && "opacity-40")}
    />
  );
}
