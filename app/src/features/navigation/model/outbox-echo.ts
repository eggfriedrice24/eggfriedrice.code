// Joins the outbox to the streams of the conversations (plan section 2.2, "Outbox for
// prompts"). An entry of the outbox stays after efrd took it, so the pending row shows,
// until the stream shows its echo: the `prompt_queued` event with its command id. This
// wrapper of the stream factory tells the outbox about each echo:
//
// - an event `prompt_queued` acknowledges its command id;
// - a snapshot acknowledges the `prompt_queued` events that it holds, and settles the
//   sent entries of the conversation up to its `hwm`, for an echo that is older than the
//   events of the snapshot.
//
// The outbox changes in the same display frame as the stream store, so the pending row
// and its echo never show together or both go away for a frame. Pure.

import type { EventEnvelope, SubscribeItem } from "@/lib/protocol/wire";
import type { Outbox } from "@/features/conversation/model/outbox";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import type {
  ConversationStreamFactory,
  StreamDeps,
  SupervisedStream,
} from "@/features/machines/model/stream-factory";

/** The parts of the outbox that the echo uses. */
export type EchoOutbox = Pick<Outbox, "acknowledge" | "settle">;

function echoOf(envelope: EventEnvelope): string | undefined {
  return envelope.event.kind === "prompt_queued" ? envelope.event.command_id : undefined;
}

/** What `item` tells the outbox, or undefined when it tells nothing. */
function echoes(
  item: SubscribeItem,
): { readonly commandIds: readonly string[]; readonly hwm?: number } | undefined {
  if (item.kind === "event") {
    const commandId = echoOf(item);
    return commandId === undefined ? undefined : { commandIds: [commandId] };
  }
  if (item.kind === "snapshot") {
    const commandIds: string[] = [];
    for (const envelope of item.events) {
      const commandId = echoOf(envelope);
      if (commandId !== undefined) {
        commandIds.push(commandId);
      }
    }
    return { commandIds, hwm: item.hwm };
  }
  return undefined;
}

function tell(
  outbox: EchoOutbox,
  ref: ConversationRef,
  echo: { readonly commandIds: readonly string[]; readonly hwm?: number },
): void {
  for (const commandId of echo.commandIds) {
    outbox.acknowledge(commandId);
  }
  if (echo.hwm !== undefined) {
    outbox.settle(ref.machine, ref.id, echo.hwm);
  }
}

/** `factory`, with streams that tell `outbox` about the echo of each sent prompt. */
export function withOutboxEcho<S extends SupervisedStream>(
  factory: ConversationStreamFactory<S>,
  outbox: EchoOutbox,
): ConversationStreamFactory<S> {
  return (ref: ConversationRef, deps: StreamDeps): S => {
    const stream = factory(ref, deps);
    const item = stream.item.bind(stream);
    // NOTE: The stream is the factory's own object, so the wrapper sets its `item` on it
    // and keeps every other member and the prototype. The outbox is for all machines,
    // so an echo still counts when the stream goes away before the frame.
    return Object.assign(stream, {
      item(decoded: SubscribeItem, raw: unknown): void {
        item(decoded, raw);
        const echo = echoes(decoded);
        if (echo !== undefined) {
          deps.schedule(() => tell(outbox, ref, echo));
        }
      },
    });
  };
}
