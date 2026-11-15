// The model of one composer: it reads the connection, the draft, the outbox and the send
// mode with narrow selectors, gives them to the pure functions of
// model/composer-state.ts, and joins the actions. `Composer.Root` calls it once and
// gives the result to the slots through the context.
//
// A message in auto (S-ComposerR7 5, AU-Auto 4-6): the tap on Send asks iOS to check the
// owner first. That is a send or a queue while the mode of this conversation is auto, and
// a steer into a turn that runs in auto. While iOS checks, the message stays in the field
// and Send shows the braille cell. A check that does not pass sends nothing and shows one
// line in the strip slot; the line goes at the next Send, at a change of mode, or when
// the field is empty.

import { useSelector } from "@tanstack/react-store";
import type { Readable } from "@tanstack/store";
import { useState } from "react";

import { devWarn } from "@/lib/dev/dev-warn";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { SendMode } from "@/features/settings/model/preferences";
import {
  composerDraftKey,
  composerEditable,
  composerPlaceholder,
  composerState,
  composerTrailing,
  createLinkSelector,
  inFlightText,
  ignoreReason,
  type ComposerTarget,
} from "../../model/composer-state";
import { selectDraft } from "../../model/drafts";
import { selectMode, type PhoneMode } from "../../model/model-choices";
import type { OutboxEntry } from "../../model/outbox";
import { CHECK_REASON, notSentLine, type NotSentLine } from "@/lib/lock/owner-check";
import { useComposerActions, type ComposerDeps } from "../../use-composer-actions";
import type { ComposerModel } from "./composer-context";

/** What a composer reads from its screen. */
export interface ComposerOptions {
  readonly deps: ComposerDeps;
  readonly target: ComposerTarget;
  /** The connection atom of the machine's supervisor. */
  readonly connection: Readable<MachineConnection>;
  /** The name of the machine, for "laptop is offline". */
  readonly machineName: string;
  /** True when no machine of the phone answers: "Tailscale is not connected". */
  readonly tailnetDown?: boolean;
  /** The running turn of the conversation, or undefined when none runs. */
  readonly runningTurnId?: string;
  /** True when the running turn runs in auto: a steer into it needs the check too. */
  readonly runningAuto?: boolean;
  /**
   * Called with the outbox entry of each prompt that the composer sends. On the blank
   * root, the screen pushes the new conversation here.
   */
  readonly onSend?: (entry: OutboxEntry) => void;
}

/** The model of the composer of `options.target`. */
export function useComposer(options: ComposerOptions): ComposerModel {
  const { deps, target, runningTurnId, onSend } = options;
  const [selectLink] = useState(createLinkSelector);
  const link = useSelector(options.connection, selectLink);
  const key = composerDraftKey(target);
  const text = useSelector(deps.drafts.store, selectDraft(key));
  const inFlight = useSelector(deps.outbox.store, (snapshot) => inFlightText(snapshot, target));
  const sendMode = useSelector(deps.preferences.store, (preferences) => preferences.sendMode);
  const mode = useSelector(deps.modelChoices.store, selectMode(key));
  const actions = useComposerActions(deps, target, runningTurnId);
  // Short local UI state of this composer: iOS checks now, and the last check failed.
  const [checking, setChecking] = useState(false);
  const [failed, setFailed] = useState<NotSentLine | undefined>(undefined);
  const state = composerState({
    isBlankRoot: target.isBlankRoot,
    link,
    runningTurnId,
    text,
    sending: inFlight !== undefined || actions.steering || checking,
    sendMode,
  });

  /** True when `kind` needs the check of the owner first. */
  const needsCheck = (kind: SendKind): boolean =>
    kind === "steer" ? options.runningAuto === true : mode === "auto";

  // NOTE: Each send runs before the call of `onSend`. `onSend?.(actions.send(text))` does
  // not send at all when the screen gives no `onSend`, as a conversation screen does.
  const perform = (kind: SendKind, words: string, auto: boolean): void => {
    if (kind === "steer") {
      actions.steer(words);
      return;
    }
    const entry = kind === "send" ? actions.send(words, auto) : actions.queue(words, auto);
    onSend?.(entry);
  };

  const submit = (pickedMode: SendMode): void => {
    const kind = sendKindOf(state, pickedMode);
    if (kind === undefined) {
      devWarn(`the composer of ${target.id} ignores a submit: ${ignoreReason(state)}`);
      return;
    }
    setFailed(undefined);
    if (!needsCheck(kind)) {
      perform(kind, text, false);
      return;
    }
    const words = text;
    const auto = kind !== "steer";
    setChecking(true);
    void deps
      .checkOwner(CHECK_REASON)
      .then((check) => {
        const line = notSentLine(check);
        if (line === undefined) {
          perform(kind, words, auto);
        }
        setFailed(line);
        return line;
      })
      .catch(() => {
        setFailed({ head: "Not sent.", rest: "The check did not run. Tap Send to try again." });
      })
      .finally(() => setChecking(false));
  };
  const checks =
    state.kind === "typing" || state.kind === "running"
      ? needsCheck(sendKindOf(state, sendMode) ?? "send")
      : false;

  return {
    state,
    trailing: composerTrailing(state),
    isBlankRoot: target.isBlankRoot,
    value: state.kind === "sending" ? (inFlight ?? text) : text,
    placeholder: composerPlaceholder(state, {
      machineName: options.machineName,
      tailnetDown: options.tailnetDown === true,
    }),
    sendMode,
    stopping: actions.stopping,
    setText: (next: string): void => {
      if (composerEditable(state)) {
        deps.drafts.set(key, next);
      }
    },
    submit: (): void => {
      submit(sendMode);
    },
    pick: (picked: SendMode): void => {
      deps.preferences.setSendMode(picked);
      submit(picked);
    },
    stop: (): void => {
      if (!actions.stopping) {
        actions.stop();
      }
    },
    mode,
    setMode: (next: PhoneMode): void => {
      setFailed(undefined);
      deps.modelChoices.setMode(key, next);
    },
    checks,
    checking,
    notSent: text === "" ? undefined : failed,
  };
}

/** What a tap on Send does. */
type SendKind = "send" | "steer" | "queue";

/** What a tap on Send does in `state` with the send mode `mode`, or none. */
function sendKindOf(state: ReturnType<typeof composerState>, mode: SendMode): SendKind | undefined {
  if (state.kind === "typing") {
    return "send";
  }
  if (state.kind === "running" && state.hasText) {
    return mode === "steer" ? "steer" : "queue";
  }
  return undefined;
}
