// What the conversation screen reads from the outbox (plan sections 2.2 and 2.5):
//
// - The prompts on their way: the entries of the conversation that efrd did not echo yet.
//   The transcript shows them after the rows of the stream, and a failed one with resend.
//   A prompt with files shows their tiles with the state of the upload, and can go back
//   until its `prompt.send` goes.
// - Which conversation the route names. The first send on the blank root pushes the
//   screen with the local id of a new outbox target. Until efrd made the conversation,
//   the screen has no stream and shows only the prompt on its way. When the send result
//   names the conversation, the outbox keeps the alias, and the screen follows the real
//   conversation and sets it in its route.
//
// Pure.

import type { Readable } from "@tanstack/store";

import type { ConversationId, MachineId } from "@/features/machines/model/machine-ref";
import { pendingTiles, sameTiles, type TileView } from "./attachments-tiles";
import { discardable, entriesFor, resolveAlias, type OutboxSnapshot } from "./outbox";

/** One prompt on its way, as the transcript shows it. */
export interface OutboxPrompt {
  /** The command id of the entry. */
  readonly key: string;
  readonly text: string;
  /** True when efrd refused it: the row offers a resend. */
  readonly failed: boolean;
  /** The files of the prompt with the state of their upload. */
  readonly tiles: readonly TileView[];
  /** The sentence of a failed upload, for the tag of the row. */
  readonly reason: string | undefined;
  /** True while the prompt with files can go back: its `prompt.send` did not go yet. */
  readonly withdrawable: boolean;
}

/** The prompts of conversation `id` on `machine` that efrd did not echo yet, by order. */
export function outboxPrompts(
  snapshot: OutboxSnapshot,
  machine: MachineId,
  id: string,
): readonly OutboxPrompt[] {
  return entriesFor(snapshot, machine, id).map((entry) => {
    const failed = entry.state === "failed";
    const hasFiles = (entry.attachments?.length ?? 0) > 0;
    return {
      key: entry.commandId,
      text: entry.text,
      failed,
      tiles: pendingTiles(entry),
      reason: failed && entry.error?.code === "upload" ? entry.error.message : undefined,
      withdrawable: hasFiles && entry.state !== "sent" && discardable(entry),
    };
  });
}

/** True when the two lists show the same prompts. */
export function sameOutboxPrompts(a: readonly OutboxPrompt[], b: readonly OutboxPrompt[]): boolean {
  return (
    a === b ||
    (a.length === b.length &&
      a.every((prompt, i) => {
        const other = b[i];
        return (
          other !== undefined &&
          other.key === prompt.key &&
          other.text === prompt.text &&
          other.failed === prompt.failed &&
          other.reason === prompt.reason &&
          other.withdrawable === prompt.withdrawable &&
          sameTiles(other.tiles, prompt.tiles)
        );
      }))
  );
}

/** The conversation that the route of a screen names. */
export type RouteConversation =
  /** A conversation of efrd: the screen holds its stream. */
  | { readonly kind: "conversation"; readonly id: ConversationId }
  /** The local id of a new conversation that efrd did not make yet: no stream yet. */
  | { readonly kind: "new"; readonly localId: string };

/**
 * What the id `id` of a route on `machine` names: the conversation of its alias, a new
 * conversation that the outbox still sends, or else a conversation of efrd.
 */
export function routeConversation(
  snapshot: OutboxSnapshot,
  machine: MachineId,
  id: string,
): RouteConversation {
  const alias = resolveAlias(snapshot, machine, id);
  if (alias !== undefined) {
    return { kind: "conversation", id: alias };
  }
  const local = snapshot.entries.some(
    (entry) =>
      entry.machine === machine && entry.target.kind === "new" && entry.target.localId === id,
  );
  return local ? { kind: "new", localId: id } : { kind: "conversation", id };
}

/**
 * Calls `onAlias` once, when the outbox knows the conversation that efrd made for the
 * local id `id` on `machine`: at once when it knows it now. Returns the function that
 * stops the watch.
 */
export function followAlias(
  store: Readable<OutboxSnapshot>,
  machine: MachineId,
  id: string,
  onAlias: (conversation: ConversationId) => void,
): () => void {
  let told = false;
  const check = (): void => {
    if (told) {
      return;
    }
    const alias = resolveAlias(store.get(), machine, id);
    if (alias !== undefined) {
      told = true;
      onAlias(alias);
    }
  };
  const subscription = store.subscribe(check);
  check();
  return () => subscription.unsubscribe();
}
