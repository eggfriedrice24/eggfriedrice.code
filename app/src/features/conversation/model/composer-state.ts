// The state of the composer (plan section 2.6, design 3.3), as pure functions of what it
// shows: the connection to the machine, the running turn, the text, a send in flight and
// the send mode. The components draw only what these functions give, so the composer has
// no flags of its own, and each state of the design canvas has a test.
//
// - The link: the connection, as the composer sees it. A connection that closed, or one
//   that opens, is "reconnecting": the input stays editable and Send waits. When the
//   machine does not answer ("after a few failed tries" in the design), the composer is
//   "disconnected" until the connection is back, also while the client tries again, so
//   the input does not turn on and off at each try.
// - The state: what the composer is now (blank, idle, typing, running, sending,
//   reconnecting, disconnected). The read-only "held in a terminal" state of the canvas
//   is gone (user decision): efrd lets the phone send to a held conversation.
// - The trailing action, the placeholder, the status line and the target of a send.
//
// Pure TypeScript.

import type { ContextUse, Mode } from "@/lib/protocol/generated";
import type { StreamState } from "@/lib/stream/reducer";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { ConnectionView } from "@/features/machines/model/connection-view";
import type { SendMode } from "@/features/settings/model/preferences";
import { shortPath } from "@/features/conversations/model/list-format";
import { BLANK_ROOT, draftKey, type DraftKey } from "./drafts";
import { contextGauge } from "./format";
import { entriesFor, resolveAlias, type OutboxSnapshot, type OutboxTarget } from "./outbox";
import { selectStatus, type Waiting } from "./selectors";

/** Why the composer cannot send, in the words of its placeholder. */
export type DisconnectReason =
  /** The machine does not answer. With `tailnetDown`, Tailscale is most likely off. */
  | "unreachable"
  /** efrd answered that it is starting. */
  | "starting"
  /** efrd does not accept this phone any more: it must pair again. */
  | "repair"
  /** The app and efrd speak different versions of the protocol. */
  | "update";

/** The connection to the machine, as the composer sees it. */
export type ComposerLink =
  | { readonly kind: "online" }
  /** The connection opens or closed: the input stays editable, and Send waits. */
  | { readonly kind: "reconnecting" }
  | { readonly kind: "disconnected"; readonly reason: DisconnectReason };

const ONLINE: ComposerLink = { kind: "online" };
const RECONNECTING: ComposerLink = { kind: "reconnecting" };
const DISCONNECTED: Readonly<Record<DisconnectReason, ComposerLink>> = {
  unreachable: { kind: "disconnected", reason: "unreachable" },
  starting: { kind: "disconnected", reason: "starting" },
  repair: { kind: "disconnected", reason: "repair" },
  update: { kind: "disconnected", reason: "update" },
};

/**
 * The link after the connection went to `view`. `previous` is the link before it, or
 * undefined for the first one. A link is one of a few constant objects, so a compare
 * with `===` finds a change.
 */
export function composerLink(
  previous: ComposerLink | undefined,
  view: ConnectionView,
): ComposerLink {
  switch (view.kind) {
    case "online":
      return ONLINE;
    case "connecting":
    case "retrying":
      // A disconnected composer stays so while the client tries again: else the input
      // turns on for each try of a machine that does not answer.
      return previous?.kind === "disconnected" ? previous : RECONNECTING;
    case "unreachable":
      return DISCONNECTED.unreachable;
    case "starting":
      return DISCONNECTED.starting;
    case "update_needed":
      return DISCONNECTED.update;
    default:
      // `repair_needed` and `unpaired`.
      return DISCONNECTED.repair;
  }
}

/**
 * A selector of the link from the connection atom of a supervisor, for `useSelector`.
 * It keeps the last link, so each composer makes its own with `useState`.
 */
export function createLinkSelector(): (connection: MachineConnection) => ComposerLink {
  let last: ComposerLink | undefined;
  return (connection) => {
    last = composerLink(last, connection.view);
    return last;
  };
}

/** What the composer is now. */
export type ComposerState =
  /** The blank root with an empty field: Send is disabled. */
  | { readonly kind: "blank" }
  /** A conversation with no running turn and an empty field: Send is disabled. */
  | { readonly kind: "idle" }
  /** No running turn and some text: Send is enabled. */
  | { readonly kind: "typing" }
  /** A turn runs. Empty: Stop only. With text: Stop and the Steer or Queue capsule. */
  | { readonly kind: "running"; readonly hasText: boolean; readonly sendMode: SendMode }
  /**
   * A prompt or a steer waits for the answer of efrd: the text shows dimmed, and the
   * Send circle shows the braille cell. `running` keeps Stop next to it.
   */
  | { readonly kind: "sending"; readonly running: boolean }
  /** The connection opens: the input stays editable, and Send is disabled. */
  | { readonly kind: "reconnecting"; readonly running: boolean }
  /** No connection: the input cannot take text, at 60%, and its placeholder says why. */
  | { readonly kind: "disconnected"; readonly reason: DisconnectReason };

/** What `composerState` reads. */
export interface ComposerInput {
  /** True on the blank root, where the first send makes a new conversation. */
  readonly isBlankRoot: boolean;
  readonly link: ComposerLink;
  /** The id of the running turn of the conversation, or undefined when none runs. */
  readonly runningTurnId: string | undefined;
  readonly text: string;
  /** True while a prompt or a steer of this composer waits for efrd. */
  readonly sending: boolean;
  readonly sendMode: SendMode;
}

/** True when `text` has something to send: whitespace only is nothing. */
export function hasText(text: string): boolean {
  return text.trim().length > 0;
}

/** The state of the composer. The connection comes first, then a send in flight. */
export function composerState(input: ComposerInput): ComposerState {
  const running = !input.isBlankRoot && input.runningTurnId !== undefined;
  if (input.link.kind === "disconnected") {
    return { kind: "disconnected", reason: input.link.reason };
  }
  if (input.link.kind === "reconnecting") {
    return { kind: "reconnecting", running };
  }
  if (input.sending) {
    return { kind: "sending", running };
  }
  const typed = hasText(input.text);
  if (running) {
    return { kind: "running", hasText: typed, sendMode: input.sendMode };
  }
  if (typed) {
    return { kind: "typing" };
  }
  return input.isBlankRoot ? { kind: "blank" } : { kind: "idle" };
}

/** What the trailing slot (slot D) shows. */
export type ComposerTrailing =
  /** The Send circle: filled yolk when enabled, a hairline circle when disabled. */
  | { readonly kind: "send"; readonly enabled: boolean }
  /** Stop, and the Steer or Queue capsule when there is text (`capsule` is null without). */
  | { readonly kind: "stop"; readonly capsule: SendMode | null }
  /** The Send circle with the braille cell; Stop before it while a turn runs. */
  | { readonly kind: "sending"; readonly stop: boolean };

/** The trailing action of `state`. */
export function composerTrailing(state: ComposerState): ComposerTrailing {
  switch (state.kind) {
    case "typing":
      return { kind: "send", enabled: true };
    case "running":
      return { kind: "stop", capsule: state.hasText ? state.sendMode : null };
    case "sending":
      return { kind: "sending", stop: state.running };
    default:
      return { kind: "send", enabled: false };
  }
}

/**
 * True when the input takes text. A send in flight and no connection stop it. While a
 * send is in flight, the native input stays editable, so the keyboard does not go down
 * for each send; the composer only drops the changes.
 */
export function composerEditable(state: ComposerState): boolean {
  return state.kind !== "disconnected" && state.kind !== "sending";
}

/** The key of the draft of `target`: the blank root has one draft per machine. */
export function composerDraftKey(target: ComposerTarget): DraftKey {
  return draftKey(target.machine, target.isBlankRoot ? BLANK_ROOT : target.id);
}

/** The words of a placeholder that need the app. */
export interface PlaceholderWords {
  /** The name of the machine, such as "laptop". */
  readonly machineName: string;
  /** True when no machine of the phone answers: Tailscale is then most likely off. */
  readonly tailnetDown: boolean;
}

/** The placeholder of the input in `state`. */
export function composerPlaceholder(state: ComposerState, words: PlaceholderWords): string {
  switch (state.kind) {
    case "running":
      return "Steer or queue";
    case "reconnecting":
    case "sending":
      return state.running ? "Steer or queue" : "Message efr";
    case "disconnected":
      return disconnectText(state.reason, words);
    default:
      return "Message efr";
  }
}

/** The short reason of a disconnected composer (design 3.3, 3.19 and item 8.19). */
export function disconnectText(reason: DisconnectReason, words: PlaceholderWords): string {
  switch (reason) {
    case "unreachable":
      return words.tailnetDown ? "Tailscale is not connected" : `${words.machineName} is offline`;
    case "starting":
      return "efrd is starting";
    case "repair":
      return "re-pair needed";
    default:
      return "update needed";
  }
}

/** What a composer writes to: a conversation, or the blank root of a machine. */
export interface ComposerTarget {
  /** The daemon id of the machine. */
  readonly machine: string;
  /**
   * The conversation id. On the blank root, and on a conversation screen before efrd
   * made the conversation, it is the local id of the new target of the outbox.
   */
  readonly id: string;
  /** True on the blank root: the first send makes a new conversation. */
  readonly isBlankRoot: boolean;
}

/**
 * Where a prompt of `target` goes in the outbox. A local id that efrd made into a
 * conversation goes to that conversation; one that efrd did not make yet stays a new
 * target, so the prompt waits for the first one.
 */
export function outboxTargetOf(snapshot: OutboxSnapshot, target: ComposerTarget): OutboxTarget {
  if (target.isBlankRoot) {
    return { kind: "new", localId: target.id };
  }
  const alias = resolveAlias(snapshot, target.machine, target.id);
  if (alias !== undefined) {
    return { kind: "conversation", conversationId: alias };
  }
  const local = snapshot.entries.some(
    (entry) =>
      entry.machine === target.machine &&
      entry.target.kind === "new" &&
      entry.target.localId === target.id,
  );
  return local
    ? { kind: "new", localId: target.id }
    : { kind: "conversation", conversationId: target.id };
}

/** The conversation id of `target` for a steer or a stop: its alias, else its id. */
export function conversationIdOf(snapshot: OutboxSnapshot, target: ComposerTarget): string {
  return resolveAlias(snapshot, target.machine, target.id) ?? target.id;
}

/**
 * The text of the newest prompt of `target` that waits for the answer of efrd, or
 * undefined when none waits. The composer shows it dimmed while it is sent.
 */
export function inFlightText(snapshot: OutboxSnapshot, target: ComposerTarget): string | undefined {
  const entries = entriesFor(snapshot, target.machine, target.id);
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i];
    if (entry !== undefined && (entry.state === "pending" || entry.state === "sending")) {
      return entry.text;
    }
  }
  return undefined;
}

/** What the status line (slot A) shows of the conversation. */
export interface ComposerStatus {
  readonly cwd: string | undefined;
  readonly mode: Mode | undefined;
  readonly context: ContextUse | undefined;
  readonly waiting: Waiting;
}

/** The status line selector of the stream store. Compare with `shallow`. */
export function selectComposerStatus(state: StreamState): ComposerStatus {
  const status = selectStatus(state);
  return {
    cwd: status.cwd,
    mode: status.mode,
    context: status.context,
    waiting: status.waiting,
  };
}

/** The colour of a part of the status line, by its role. */
export type StatusTone = "text_muted" | "text_subtle" | "accent_primary";

/** One part of the status line. The line puts a "·" between two parts. */
export interface StatusPart {
  readonly key: "cwd" | "mode" | "context" | "waiting";
  readonly text: string;
  readonly tone: StatusTone;
}

/**
 * The parts of the status line: the cwd, the mode, `ctx 42%`, and "waiting for you"
 * while an approval waits on this phone. A part without a value is not there.
 */
export function statusParts(status: ComposerStatus): readonly StatusPart[] {
  const parts: StatusPart[] = [];
  if (status.cwd !== undefined && status.cwd !== "") {
    parts.push({ key: "cwd", text: shortPath(status.cwd), tone: "text_muted" });
  }
  if (status.mode !== undefined) {
    parts.push({ key: "mode", text: status.mode, tone: "text_subtle" });
  }
  const gauge = status.context === undefined ? undefined : contextGauge(status.context);
  if (gauge !== undefined) {
    parts.push({ key: "context", text: gauge.text, tone: "text_subtle" });
  }
  if (status.waiting === "phone") {
    parts.push({ key: "waiting", text: "waiting for you", tone: "accent_primary" });
  } else if (status.waiting === "computer") {
    parts.push({ key: "waiting", text: "waiting in the terminal", tone: "text_muted" });
  }
  return parts;
}

/** The items of the Steer/Queue menu (design 3.3 and item x4). */
export interface SendModeChoice {
  readonly mode: SendMode;
  readonly title: string;
  readonly subtitle: string;
}

/** The two choices of the Steer/Queue menu, in order. */
export const SEND_MODE_CHOICES: readonly SendModeChoice[] = [
  { mode: "steer", title: "Steer now", subtitle: "Joins the running turn" },
  { mode: "queue", title: "Queue", subtitle: "Sends when this turn ends" },
];

/** The label of the capsule of `mode`. */
export function capsuleLabel(mode: SendMode): string {
  return mode === "steer" ? "Steer" : "Queue";
}
