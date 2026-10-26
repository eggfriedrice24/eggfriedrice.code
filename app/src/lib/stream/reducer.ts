// The stream reducer: the state of one conversation, built from the items of its
// `conversation.subscribe` stream. It is pure and lives outside React; the store
// subscribes the live row to it. Each function returns a new state and shares every
// part that did not change, so a selector sees a changed object only where the change
// is.
//
// The rules (R4c.4 step 3 of the research):
//
// - Messages are keyed by turn and index. Drafts and `assistant_message_updated` merge
//   into one text buffer per message (see text_buffer.ts).
// - `assistant_message_completed` is the truth: it replaces the text and makes the
//   message final. When the completed text does not extend the text so far, the
//   message's `generation` grows, so a view knows to remount instead of append.
// - The end of a turn makes its messages final. Drafts of an ended turn and of a final
//   message are dropped.
// - Reasoning comes only in drafts and the snapshot: a buffer per turn, with its title.
// - Tool input drafts show while the model writes; `tool_call_started` clears them, and a
//   draft made before it is dropped.
// - An event with a sequence number at or below the last one seen is a duplicate of a
//   replay and changes nothing.

import { applyDelta, bufferOf, EMPTY_BUFFER, seed, type TextBuffer } from "./text-buffer";
import type {
  Draft,
  Event,
  EventEnvelope,
  LiveTurn,
  SnapshotView,
  SubscribeItem,
} from "@/lib/protocol/wire";

/** One assistant message of a turn. */
export interface MessageView {
  readonly index: number;
  readonly buffer: TextBuffer;
  /** True once the text cannot change: completed, or its turn ended. */
  readonly final: boolean;
  /** Grows each time the text changed in a way that is not an append. */
  readonly generation: number;
}

/** What the model writes now as the input of a tool call. */
export interface ToolInputView {
  readonly tool: string;
  readonly bytes: number;
}

/** A tool call of a turn. */
export interface ToolCallView {
  readonly tool: string;
  /** The end of the output so far, bounded by efrd. */
  readonly tail: string;
  /** The size of the whole output so far, in bytes. */
  readonly bytes: number;
  readonly done: boolean;
}

/** One turn. */
export interface TurnView {
  readonly turnId: string;
  readonly ended: boolean;
  readonly messages: ReadonlyMap<number, MessageView>;
  readonly reasoning: TextBuffer;
  readonly title: string | undefined;
  /** Tool input drafts by the call's position in the model's answer. */
  readonly toolInputs: ReadonlyMap<number, ToolInputView>;
  /** The sequence number of the last `tool_call_started`; older tool input drafts drop. */
  readonly toolStartedSeq: number;
  readonly toolCalls: ReadonlyMap<string, ToolCallView>;
}

/** The state of one conversation's stream. */
export interface StreamState {
  /** The largest sequence number seen: the `after_seq` of the next subscribe. */
  readonly lastSeq: number;
  readonly turns: ReadonlyMap<string, TurnView>;
  /** Turn ids, oldest first. */
  readonly order: readonly string[];
}

/** The state before the first item. */
export const EMPTY_STREAM: StreamState = { lastSeq: 0, turns: new Map(), order: [] };

/** Applies one item of the subscribe stream. */
export function reduce(state: StreamState, item: SubscribeItem): StreamState {
  switch (item.kind) {
    case "event":
      return applyEvent(state, item);
    case "draft":
      return applyDraft(state, item);
    case "snapshot":
      return applySnapshot(state, item);
    default:
      return state;
  }
}

/** Applies one event. A duplicate (its seq at or below `lastSeq`) changes nothing. */
export function applyEvent(state: StreamState, envelope: EventEnvelope): StreamState {
  if (envelope.seq <= state.lastSeq) {
    return state;
  }
  const next = applyEventBody(state, envelope.event, envelope.seq);
  return { ...next, lastSeq: envelope.seq };
}

/** Applies one draft. Drafts of an ended turn, of a final message or older than a tool call start drop. */
export function applyDraft(state: StreamState, draft: Draft): StreamState {
  const existing = state.turns.get(draft.turn_id);
  if (existing?.ended === true) {
    return state;
  }
  const part = draft.draft;
  switch (part.kind) {
    case "text":
      return updateTurn(state, draft.turn_id, (turn) =>
        updateMessage(turn, part.index, (message) =>
          appendToMessage(message, part.offset, part.delta),
        ),
      );
    case "reasoning":
      return updateTurn(state, draft.turn_id, (turn) => {
        const reasoning = applyDelta(turn.reasoning, part.offset, part.delta);
        const title = part.title ?? turn.title;
        return reasoning === turn.reasoning && title === turn.title
          ? turn
          : { ...turn, reasoning, title };
      });
    case "tool_input":
      return updateTurn(state, draft.turn_id, (turn) => {
        if (draft.after_seq < turn.toolStartedSeq) {
          return turn;
        }
        const current = turn.toolInputs.get(part.call);
        if (current !== undefined && current.bytes >= part.bytes && current.tool === part.tool) {
          return turn;
        }
        const toolInputs = new Map(turn.toolInputs);
        toolInputs.set(part.call, { tool: part.tool, bytes: part.bytes });
        return { ...turn, toolInputs };
      });
    default:
      return state;
  }
}

/**
 * Applies a snapshot: the state is rebuilt from its events, then the live part seeds the
 * running message and reasoning. Text the previous state held beyond the rebuilt text
 * is kept when it agrees with it, so a reconnect does not lose what the app showed.
 * Turns of the previous state with no event in the snapshot stay, before the others.
 */
export function applySnapshot(state: StreamState, snapshot: SnapshotView): StreamState {
  let next: StreamState = EMPTY_STREAM;
  for (const envelope of snapshot.events) {
    next = applyEvent(next, envelope);
  }
  next = { ...next, lastSeq: Math.max(next.lastSeq, snapshot.hwm) };
  if (snapshot.live !== undefined && snapshot.live !== null) {
    next = applyLive(next, snapshot.live);
  }
  return keepPrevious(state, next);
}

/** Seeds the running message and the reasoning from the snapshot's live part. */
export function applyLive(state: StreamState, live: LiveTurn): StreamState {
  return updateTurn(state, live.turn_id, (turn) => {
    if (turn.ended) {
      return turn;
    }
    let next = updateMessage(turn, live.index, (message) => seedMessage(message, live.text));
    if (live.reasoning !== undefined && live.reasoning !== null) {
      const reasoning = seed(next.reasoning, live.reasoning);
      if (reasoning !== next.reasoning) {
        next = { ...next, reasoning };
      }
    }
    const title = live.reasoning_title ?? undefined;
    if (title !== undefined && title !== next.title) {
      next = { ...next, title };
    }
    return next;
  });
}

function applyEventBody(state: StreamState, event: Event, seq: number): StreamState {
  switch (event.kind) {
    case "turn_started":
      return updateTurn(state, event.turn_id, (turn) => turn);
    case "assistant_message_updated":
      return updateTurn(state, event.turn_id, (turn) =>
        updateMessage(turn, event.index, (message) =>
          appendToMessage(message, event.offset, event.delta),
        ),
      );
    case "assistant_message_completed":
      return updateTurn(state, event.turn_id, (turn) =>
        updateMessage(turn, event.index, (message) => complete(message, event.text)),
      );
    case "tool_call_started":
      return updateTurn(state, event.turn_id, (turn) => {
        const toolCalls = new Map(turn.toolCalls);
        toolCalls.set(event.call_id, { tool: event.tool, tail: "", bytes: 0, done: false });
        return { ...turn, toolCalls, toolInputs: new Map(), toolStartedSeq: seq };
      });
    case "tool_call_output_updated":
      return updateTurn(state, event.turn_id, (turn) =>
        updateToolCall(turn, event.call_id, (call) => ({
          ...call,
          tail: event.tail,
          bytes: event.bytes,
        })),
      );
    case "tool_call_completed":
      return updateTurn(state, event.turn_id, (turn) =>
        updateToolCall(turn, event.call_id, (call) => ({ ...call, done: true })),
      );
    case "turn_completed":
    case "turn_failed":
    case "turn_interrupted":
    case "turn_cancelled":
      return updateTurn(state, event.turn_id, endTurn);
    default:
      return state;
  }
}

function appendToMessage(message: MessageView, offset: number, delta: string): MessageView {
  if (message.final) {
    return message;
  }
  const buffer = applyDelta(message.buffer, offset, delta);
  return buffer === message.buffer ? message : { ...message, buffer };
}

function complete(message: MessageView, text: string): MessageView {
  const held = message.buffer.text;
  if (message.final && held === text) {
    return message;
  }
  const appends = text.startsWith(held);
  return {
    ...message,
    buffer: bufferOf(text),
    final: true,
    generation: appends ? message.generation : message.generation + 1,
  };
}

function seedMessage(message: MessageView, text: string): MessageView {
  if (message.final) {
    return message;
  }
  const buffer = seed(message.buffer, text);
  if (buffer === message.buffer) {
    return message;
  }
  const appends = text.startsWith(message.buffer.text);
  return {
    ...message,
    buffer,
    generation: appends ? message.generation : message.generation + 1,
  };
}

function endTurn(turn: TurnView): TurnView {
  const messages = new Map<number, MessageView>();
  for (const [index, message] of turn.messages) {
    messages.set(index, message.final ? message : { ...message, final: true });
  }
  return { ...turn, ended: true, messages, toolInputs: new Map() };
}

function keepPrevious(previous: StreamState, next: StreamState): StreamState {
  if (previous.order.length === 0) {
    return next;
  }
  const turns = new Map(next.turns);
  const older: string[] = [];
  for (const turnId of previous.order) {
    const old = previous.turns.get(turnId);
    if (old === undefined) {
      continue;
    }
    const rebuilt = turns.get(turnId);
    if (rebuilt === undefined) {
      turns.set(turnId, old);
      older.push(turnId);
    } else {
      turns.set(turnId, mergeTurn(old, rebuilt));
    }
  }
  return {
    lastSeq: Math.max(previous.lastSeq, next.lastSeq),
    turns,
    order: [...older, ...next.order],
  };
}

function mergeTurn(old: TurnView, rebuilt: TurnView): TurnView {
  const messages = new Map(rebuilt.messages);
  for (const [index, held] of old.messages) {
    const message = rebuilt.messages.get(index);
    if (message === undefined) {
      // The snapshot window starts after this message's events.
      messages.set(index, rebuilt.ended && !held.final ? { ...held, final: true } : held);
      continue;
    }
    let merged = message;
    if (!message.final && held.buffer.text.startsWith(message.buffer.text)) {
      merged = { ...message, buffer: seed(message.buffer, held.buffer.text) };
    }
    // The generation counts from what the app showed: the held message.
    const appends = merged.buffer.text.startsWith(held.buffer.text);
    messages.set(index, {
      ...merged,
      generation: appends ? held.generation : held.generation + 1,
    });
  }
  const reasoning =
    rebuilt.ended || !old.reasoning.text.startsWith(rebuilt.reasoning.text)
      ? rebuilt.reasoning
      : seed(rebuilt.reasoning, old.reasoning.text);
  return {
    ...rebuilt,
    messages,
    reasoning,
    title: rebuilt.title ?? old.title,
  };
}

function newTurn(turnId: string): TurnView {
  return {
    turnId,
    ended: false,
    messages: new Map(),
    reasoning: EMPTY_BUFFER,
    title: undefined,
    toolInputs: new Map(),
    toolStartedSeq: 0,
    toolCalls: new Map(),
  };
}

function updateTurn(
  state: StreamState,
  turnId: string,
  update: (turn: TurnView) => TurnView,
): StreamState {
  const existing = state.turns.get(turnId);
  const turn = update(existing ?? newTurn(turnId));
  if (turn === existing) {
    return state;
  }
  const turns = new Map(state.turns);
  turns.set(turnId, turn);
  return {
    ...state,
    turns,
    order: existing === undefined ? [...state.order, turnId] : state.order,
  };
}

function updateMessage(
  turn: TurnView,
  index: number,
  update: (message: MessageView) => MessageView,
): TurnView {
  const existing = turn.messages.get(index);
  const message = update(
    existing ?? { index, buffer: EMPTY_BUFFER, final: turn.ended, generation: 0 },
  );
  if (message === existing) {
    return turn;
  }
  const messages = new Map(turn.messages);
  messages.set(index, message);
  return { ...turn, messages };
}

function updateToolCall(
  turn: TurnView,
  callId: string,
  update: (call: ToolCallView) => ToolCallView,
): TurnView {
  const existing = turn.toolCalls.get(callId);
  const call = update(existing ?? { tool: "", tail: "", bytes: 0, done: false });
  const toolCalls = new Map(turn.toolCalls);
  toolCalls.set(callId, call);
  return { ...turn, toolCalls };
}
