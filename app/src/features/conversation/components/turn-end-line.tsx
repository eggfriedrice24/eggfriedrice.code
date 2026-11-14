// The line at the end of a turn (design 3.9 and the C-Failed board): one small mono 11/15
// line in text_subtle right under the answer, left aligned as the terminal prints it, such
// as "14:32 · 3m 08s · 18k tokens". The user asked for it in place of the centred line
// between two hairlines.
//
// - Interrupted: "■" in text_muted, then "interrupted by you · 14:31 · 1m 02s".
// - Failed: "✗" in state_error and "turn failed" in text_primary, then the time; under
//   it the reason in mono 11/16 text_muted; under that the plain buttons
//   "retry" and "copy error", 18 pt apart. Retry sends the turn's prompt again as a new
//   prompt (decision D8). The composer goes back to idle by itself.
//
// - A turn whose account is out (AC-Out): the reason with its reset, then under the newest
//   turn the offer of the same model on the other accounts ("switch to ChatGPT plan · b",
//   which opens the model sheet with that row selected), a quiet line for a signed-out
//   account or for none, and "retry" when no switch stands. model/account-out.ts.
// - A turn that searched the web (TM-Web 10 to 13, kept minimal on the phone): the
//   sources that the answer cites, one quiet line each above the end line, the count of
//   the searches last on the end line, and under the first turn of a conversation that
//   read the provider's stored copy of the web, one quiet line that says what it means.
//
// The error haptic of a failed turn belongs to the screen: it plays only when the
// conversation is on screen.

import { useSelector } from "@tanstack/react-store";
import { createStore } from "@tanstack/store";
import { Pressable, Text, View } from "react-native";

import { MonoText } from "@/components/ui/mono-text";
import { PlainButton } from "@/components/ui/plain-button";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import { openLink } from "@/lib/native/links";
import type { AccountOut, Origin } from "@/lib/protocol/generated";
import type { StreamState, TurnEnd, TurnWebView } from "@/lib/stream/reducer";
import { useNow } from "@/features/navigation/use-now";
import { accountOutView } from "../model/account-out";
import { linkAction } from "../model/links";
import type { ModelChoicesState } from "../model/model-choices";
import { compareShallow } from "../model/selectors";
import {
  errorText,
  type SourceRow,
  sourceRows,
  STORED_COPY_NOTE,
  type TurnEndView,
  turnEndView,
} from "../model/turn-end";
import { useTranscriptScope } from "./use-transcript-scope";

/** The choices of a transcript without a composer: none. */
const NO_CHOICES = createStore<ModelChoicesState>({
  choices: new Map(),
  quiet: new Set<string>(),
});

export interface TurnEndLineViewProps {
  readonly view: TurnEndView;
  /** Sends the prompt again; undefined when there is no prompt to send. */
  readonly onRetry: (() => void) | undefined;
}

/** The turn end line. */
export function TurnEndLineView(props: TurnEndLineViewProps) {
  const view = props.view;
  const onRetry = props.onRetry;
  return (
    <View>
      <View accessible accessibilityLabel={view.label}>
        <Text className="text-mono-xs-tight text-text-subtle">
          {view.glyph === undefined ? null : (
            <Text className={view.glyph === "✗" ? "text-state-error" : "text-text-muted"}>
              {`${view.glyph} `}
            </Text>
          )}
          {view.lead === undefined ? null : (
            <Text className="text-text-primary">{`${view.lead} `}</Text>
          )}
          {view.detail}
        </Text>
      </View>
      {view.reason === undefined ? null : (
        <MonoText tone="text_muted" className="mt-4">
          {view.reason}
        </MonoText>
      )}
      {view.canRetry ? (
        <View className="mt-2 flex-row gap-18">
          {onRetry === undefined ? null : (
            <PlainButton label="retry" accessibilityLabel="Retry the turn" onPress={onRetry} />
          )}
          <PlainButton
            label="copy error"
            onPress={() => {
              void copyText(errorText(view));
              haptic("light");
            }}
          />
        </View>
      ) : null}
    </View>
  );
}

export interface AccountOutEndProps {
  readonly view: TurnEndView;
  readonly out: AccountOut;
  /** The turn is the newest of the conversation: only it offers a switch. */
  readonly newest: boolean;
  readonly onRetry: (() => void) | undefined;
  readonly onSwitch: (model: string, provider: string) => void;
}

/**
 * The end of a turn whose account is out (AC-Out): "✗ turn failed", the reason with its
 * reset, then the offer: "switch to ChatGPT plan · b" per account that serves the model,
 * a quiet line for a signed-out account or for none, and "retry" when no switch stands.
 */
export function AccountOutEnd(props: AccountOutEndProps) {
  const scope = useTranscriptScope();
  const now = useNow();
  const choice = useSelector(scope.choice?.store ?? NO_CHOICES, (state) =>
    scope.choice === undefined ? undefined : state.choices.get(scope.choice.key),
  );
  const offer = accountOutView(props.out, { newest: props.newest, now, choice });
  const onRetry = props.onRetry;
  const actions = offer.switches.length > 0 || (offer.retry && onRetry !== undefined);
  return (
    <View testID="account-out">
      <TurnEndLineView
        view={{ ...props.view, reason: offer.reason, canRetry: false }}
        onRetry={undefined}
      />
      {offer.notes.map((note) => (
        <MonoText key={note} tone="text_subtle" className="mt-2">
          {note}
        </MonoText>
      ))}
      {actions ? (
        <View className="mt-2 flex-row flex-wrap gap-x-18">
          {offer.switches.map((action) => (
            <PlainButton
              key={action.provider}
              label={action.label}
              onPress={() => props.onSwitch(action.model, action.provider)}
            />
          ))}
          {offer.retry && onRetry !== undefined ? (
            <PlainButton label="retry" accessibilityLabel="Retry the turn" onPress={onRetry} />
          ) : null}
        </View>
      ) : null}
    </View>
  );
}

export interface SourcesViewProps {
  readonly rows: readonly SourceRow[];
}

/**
 * The sources that the answer cites, above the end line: the mark in text_subtle, the
 * page in accent_ui, then the title in text_subtle, one mono 11 line each. A tap asks,
 * then opens the page (the link rule of the app).
 */
export function SourcesView(props: SourcesViewProps) {
  return (
    <View testID="web-sources" className="mb-4">
      {props.rows.map((row) => (
        <Pressable
          key={row.mark}
          accessibilityRole="link"
          accessibilityLabel={`Source ${row.mark.slice(1, -1)}: ${row.title ?? row.page}, ${row.page}`}
          onPress={() => openLink(linkAction(row.url))}
          className="min-h-22 justify-center active:opacity-60"
        >
          <Text numberOfLines={1} className="text-mono-xs text-text-subtle">
            {`${row.mark} `}
            <Text className="text-accent-ui">{row.page}</Text>
            {row.title === undefined ? null : `  ${row.title}`}
          </Text>
        </Pressable>
      ))}
    </View>
  );
}

/** What the turn end row reads of its turn. */
interface EndRowView {
  readonly end: TurnEnd | undefined;
  readonly startedAt: number | undefined;
  readonly interruptedBy: Origin | undefined;
  readonly prompt: string | undefined;
  readonly web: TurnWebView | undefined;
  /** No later turn: a switch offer stands only under the newest turn. */
  readonly newest: boolean;
}

function selectEndRow(turnId: string): (state: StreamState) => EndRowView {
  return (state) => {
    const turn = state.turns.get(turnId);
    return {
      end: turn?.end,
      startedAt: turn?.startedAt,
      interruptedBy: turn?.interruptedBy,
      prompt: turn?.prompt?.text,
      web: turn?.web,
      newest: state.order[state.order.length - 1] === turnId,
    };
  };
}

export interface TurnEndRowProps {
  readonly turnId: string;
}

/** The end of turn `turnId`, from the stream store. */
export function TurnEndRow(props: TurnEndRowProps) {
  const scope = useTranscriptScope();
  const row = useSelector(scope.store, selectEndRow(props.turnId), { compare: compareShallow });
  if (row.end === undefined) {
    return null;
  }
  const view = turnEndView(row.end, row.startedAt, row.interruptedBy, scope.machineName, row.web);
  const turnId = props.turnId;
  const prompt = row.prompt;
  const sources = sourceRows(row.web);
  const onRetry = prompt === undefined ? undefined : () => scope.actions.retry(turnId, prompt);
  const out = row.end.outcome === "failed" ? row.end.accountOut : undefined;
  return (
    <View>
      {sources.length === 0 ? null : <SourcesView rows={sources} />}
      {out === undefined ? (
        <TurnEndLineView view={view} onRetry={onRetry} />
      ) : (
        <AccountOutEnd
          view={view}
          out={out}
          newest={row.newest}
          onRetry={onRetry}
          onSwitch={scope.actions.openModels}
        />
      )}
      {row.web?.storedCopyNote === true ? (
        <MonoText tone="text_muted" className="mt-2">
          {STORED_COPY_NOTE}
        </MonoText>
      ) : null}
    </View>
  );
}
