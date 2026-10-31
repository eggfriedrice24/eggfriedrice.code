// The stream reducer: the state of one conversation, built from the typed items of its
// `conversation.subscribe` stream. It is the one model of a conversation: the text of
// the messages, the reasoning, the tool calls, the approvals, the prompts, the steers,
// the context and the end of each turn. It is pure and lives outside React. Each
// function returns a new state and shares every part that did not change, so a
// selector sees a changed object only where the change is.
//
// The rules (R4c.4 step 3 of the research):
//
// - Messages are keyed by turn and index. Drafts and `assistant_message_updated` merge
//   into one text buffer per message (see text-buffer.ts).
// - `assistant_message_completed` is the truth: it replaces the text and makes the
//   message final. When the completed text does not extend the text so far, the
//   message's `generation` grows, so a view knows to remount instead of append.
// - The end of a turn makes its messages final and closes its open questions. Drafts of
//   an ended turn and of a final message are dropped.
// - Reasoning comes only in drafts and the snapshot: a buffer per turn, with its title.
// - Tool input drafts show while the model writes; `tool_call_started` clears them, and a
//   draft made before it is dropped.
// - The parts of a turn (messages, tool calls, approvals, steers) keep the order of
//   their first sight.
// - The first answer to an approval wins, from any client. A question that is open when
//   its turn ends expires. A question that a snapshot does not hold any more, while the
//   conversation waits for no answer, is `settled`: the app does not know its answer.
// - An event with a sequence number at or below the last one seen is a duplicate of a
//   replay and changes nothing. An event of a kind that this build does not know only
//   moves the cursor on.

import type {
  ApprovalDecision,
  ContextUse,
  ConversationSummary,
  ExitKind,
  Mode,
  Origin,
  StopReason,
  Usage,
} from "@/lib/protocol/generated";
import type {
  Draft,
  Event,
  EventEnvelope,
  EventOf,
  LiveTurn,
  Snapshot,
  SubscribeItem,
} from "@/lib/protocol/wire";
import { applyDelta, bufferOf, EMPTY_BUFFER, seed, type TextBuffer } from "./text-buffer";

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
  readonly callId: string;
  readonly tool: string;
  /** One line about the input, such as the command of a `shell` call. */
  readonly summary: string;
  /** The end of the output so far, bounded by efrd. */
  readonly tail: string;
  /** The size of the whole output so far, in bytes. */
  readonly bytes: number;
  readonly done: boolean;
  /** True when the tool failed or the call was denied. */
  readonly isError: boolean;
  readonly exitCode: number | undefined;
  /** The output that the model saw, truncated in the middle when it is long. */
  readonly output: string | undefined;
  /** Why efr refused the call before it ran. */
  readonly refusal: string | undefined;
  /** The unified diff of a file tool's call. */
  readonly diff: string | undefined;
  /** The command still runs in the hidden shell after the call answered. */
  readonly leftRunning: boolean;
  /** When the call started and ended, in ms since the epoch. */
  readonly startedAt: number | undefined;
  readonly endedAt: number | undefined;
}

/** What became of an approval question. */
export type ApprovalStatus =
  | "pending"
  | "allowed"
  | "denied"
  /** Nobody answered in time, or the turn ended first. The call did not run. */
  | "expired"
  /** It expired while the user was away. */
  | "expired_away"
  /** It was pending, and a later snapshot showed that the conversation waits no more. */
  | "settled";

/** An approval question of a tool call. */
export interface ApprovalView {
  readonly callId: string;
  readonly turnId: string;
  readonly summary: string;
  readonly diffPreview: string | undefined;
  /** Why the turn's permission mode asks, in one line. */
  readonly reason: string | undefined;
  /** The model's own reason for an exit of the sandbox. */
  readonly modelReason: string | undefined;
  /** The exits of the call, each kind once; empty when the question is not about an exit. */
  readonly exitKinds: readonly ExitKind[];
  /** The rule that an "always" answer saves, in words, when one may cover the call. */
  readonly always: string | undefined;
  /** Only the user's own machine may answer: efrd refuses an answer from a phone. */
  readonly localOnly: boolean;
  /** The call may wait for input at the terminal, such as a `sudo` password. */
  readonly interactive: boolean;
  /** When efrd asked, in ms since the epoch. */
  readonly askedAt: number | undefined;
  /** When the question stops waiting, in ms since the epoch, for a fixed limit. */
  readonly expiresAt: number | undefined;
  readonly status: ApprovalStatus;
  /** The answer, once resolved. */
  readonly decision: ApprovalDecision | undefined;
  /** The surface that answered, once resolved. */
  readonly answeredBy: Origin | undefined;
  /** When it was answered or expired, in ms since the epoch. */
  readonly closedAt: number | undefined;
}

/** What became of a steer. */
export type SteerStatus = "sent" | "delivered" | "withdrawn";

/** A steer: guidance for the running turn. */
export interface SteerView {
  readonly seq: number;
  readonly text: string;
  readonly at: number | undefined;
  readonly status: SteerStatus;
  /** An interrupt sent it again as a prompt; the client shows that prompt instead. */
  readonly resent: boolean;
}

/** The prompt of a turn. */
export interface PromptView {
  readonly text: string;
  /** The surface that sent it. */
  readonly origin: Origin;
  readonly commandId: string;
  readonly at: number | undefined;
}

/** How a turn ended. */
export type TurnOutcome = "completed" | "failed" | "interrupted" | "cancelled" | "withdrawn";

/** The end of a turn. */
export interface TurnEnd {
  readonly outcome: TurnOutcome;
  /** When it ended, in ms since the epoch. */
  readonly at: number | undefined;
  readonly context: ContextUse | undefined;
  /** The error's sentence, for a failed turn. */
  readonly error: string | undefined;
  /** Why the model stopped, for a completed turn that did not stop at its end. */
  readonly stop: StopReason | undefined;
  readonly usage: Usage | undefined;
}

/** The key of one part of a turn, in the order the parts came. */
export type PartKey =
  | { readonly kind: "message"; readonly index: number }
  | { readonly kind: "tool"; readonly callId: string }
  | { readonly kind: "approval"; readonly callId: string }
  | { readonly kind: "steer"; readonly seq: number };

/** One turn. */
export interface TurnView {
  readonly turnId: string;
  readonly prompt: PromptView | undefined;
  /** True once the turn began running; a queued prompt has not. */
  readonly started: boolean;
  readonly startedAt: number | undefined;
  /** The working directory and the mode that the turn runs with. */
  readonly cwd: string | undefined;
  readonly mode: Mode | undefined;
  /** The surface that asked to stop the turn. */
  readonly interruptedBy: Origin | undefined;
  readonly end: TurnEnd | undefined;
  readonly parts: readonly PartKey[];
  readonly messages: ReadonlyMap<number, MessageView>;
  readonly reasoning: TextBuffer;
  /** The title of the newest reasoning section. */
  readonly title: string | undefined;
  /**
   * When the first thought of the turn ended, in ms since the epoch: the time of the
   * first answer text or tool call after reasoning came. The thought row shows how long
   * it thought.
   */
  readonly thoughtEndedAt: number | undefined;
  /** Tool input drafts by the call's position in the model's answer. */
  readonly toolInputs: ReadonlyMap<number, ToolInputView>;
  /** The sequence number of the last `tool_call_started`; older tool input drafts drop. */
  readonly toolStartedSeq: number;
  readonly tools: ReadonlyMap<string, ToolCallView>;
  readonly approvals: ReadonlyMap<string, ApprovalView>;
  readonly steers: ReadonlyMap<number, SteerView>;
}

/** What the subscription of the stream does (the states of `SubscriptionStatus`). */
export type LinkState =
  | "waiting"
  | "subscribing"
  | "live"
  | "retrying"
  | "ended"
  | "failed"
  | "closed";

/** The state of one conversation's stream. */
export interface StreamState {
  /** The largest sequence number seen: the `after_seq` of the next subscribe. */
  readonly lastSeq: number;
  readonly turns: ReadonlyMap<string, TurnView>;
  /** Turn ids, oldest first. */
  readonly order: readonly string[];
  /** How full the context is: from the running turn's drafts, else the last turn end. */
  readonly context: ContextUse | undefined;
  /** The running turn compacts the context now. */
  readonly compacting: boolean;
  /** The summary of the last snapshot. */
  readonly summary: ConversationSummary | undefined;
  /** True once the cache or efrd gave events. */
  readonly loaded: boolean;
  readonly link: LinkState;
  /**
   * Where the older history begins, from the last snapshot: the cursor of
   * `conversation.history` for the events before its window, or null when the window
   * starts at the first event. Undefined before a snapshot, as after a resume from the
   * cache.
   */
  readonly historyCursor: string | null | undefined;
}

/** The state before the first item. */
export const EMPTY_STREAM: StreamState = {
  lastSeq: 0,
  turns: new Map(),
  order: [],
  context: undefined,
  compacting: false,
  summary: undefined,
  loaded: false,
  link: "waiting",
  historyCursor: undefined,
};

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

/** The state with the subscription in `link`. */
export function withLink(state: StreamState, link: LinkState): StreamState {
  return state.link === link ? state : { ...state, link };
}

/**
 * Applies older events under `current`, as when the cache answered after the
 * subscription gave items, or a page of `conversation.history` came: what `current`
 * holds wins, and turns that it does not hold come first.
 */
export function underlay(current: StreamState, cached: readonly EventEnvelope[]): StreamState {
  let older = EMPTY_STREAM;
  for (const envelope of cached) {
    older = applyEvent(older, envelope);
  }
  if (older.order.length === 0) {
    return current;
  }
  if (current.order.length === 0 && current.lastSeq === 0) {
    return {
      ...older,
      link: current.link,
      summary: current.summary,
      historyCursor: current.historyCursor,
    };
  }
  return { ...keepPrevious(older, current, true), link: current.link, loaded: true };
}

/** Applies one event. A duplicate (its seq at or below `lastSeq`) changes nothing. */
function applyEvent(state: StreamState, envelope: EventEnvelope): StreamState {
  if (envelope.seq <= state.lastSeq) {
    return state;
  }
  const at = time(envelope.at);
  const next = endThought(
    applyEventBody(state, envelope.event, envelope.seq, at),
    envelope.event,
    at,
  );
  return { ...next, lastSeq: envelope.seq, loaded: true };
}

/** Marks the end of the first thought of a turn at the first answer after reasoning. */
function endThought(state: StreamState, event: Event, at: number | undefined): StreamState {
  if (
    at === undefined ||
    (event.kind !== "assistant_message_updated" &&
      event.kind !== "assistant_message_completed" &&
      event.kind !== "tool_call_started")
  ) {
    return state;
  }
  const turn = state.turns.get(event.turn_id);
  if (turn === undefined || turn.thoughtEndedAt !== undefined || turn.reasoning.bytes === 0) {
    return state;
  }
  return updateTurn(state, event.turn_id, (found) => ({ ...found, thoughtEndedAt: at }));
}

function applyDraft(state: StreamState, draft: Draft): StreamState {
  const existing = state.turns.get(draft.turn_id);
  if (existing?.end !== undefined) {
    return state;
  }
  const part = draft.draft;
  switch (part.kind) {
    case "text":
      return updateTurn(state, draft.turn_id, (turn) =>
        addPart(
          updateMessage(begin(turn), part.index, (message) =>
            appendToMessage(message, part.offset, part.delta),
          ),
          { kind: "message", index: part.index },
        ),
      );
    case "reasoning":
      return updateTurn(state, draft.turn_id, (turn) => {
        const started = begin(turn);
        const reasoning = applyDelta(started.reasoning, part.offset, part.delta);
        const title = part.title ?? started.title;
        return reasoning === started.reasoning && title === started.title
          ? started
          : { ...started, reasoning, title };
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
        return { ...begin(turn), toolInputs };
      });
    case "context": {
      const context = { tokens: part.tokens, limit: part.limit, window: part.window };
      if (sameContext(state.context, context) && !state.compacting) {
        return state;
      }
      return { ...state, context, compacting: false };
    }
    case "compacting":
      return state.compacting ? state : { ...state, compacting: true };
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
function applySnapshot(state: StreamState, snapshot: Snapshot): StreamState {
  let next: StreamState = EMPTY_STREAM;
  for (const envelope of snapshot.events) {
    next = applyEvent(next, envelope);
  }
  next = { ...next, lastSeq: Math.max(next.lastSeq, snapshot.hwm) };
  if (snapshot.live !== undefined && snapshot.live !== null) {
    next = applyLive(next, snapshot.live);
  }
  const waits = snapshot.conversation.status === "awaiting_approval";
  const merged = keepPrevious(state, next, waits);
  return {
    ...merged,
    summary: snapshot.conversation,
    loaded: true,
    link: state.link,
    historyCursor: snapshot.history_cursor ?? null,
  };
}

/** Seeds the running message and the reasoning from the snapshot's live part. */
function applyLive(state: StreamState, live: LiveTurn): StreamState {
  return updateTurn(state, live.turn_id, (turn) => {
    if (turn.end !== undefined) {
      return turn;
    }
    let next = begin(turn);
    if (live.text !== "") {
      next = addPart(
        updateMessage(next, live.index, (message) => seedMessage(message, live.text)),
        { kind: "message", index: live.index },
      );
    }
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

function applyEventBody(
  state: StreamState,
  event: Event,
  seq: number,
  at: number | undefined,
): StreamState {
  switch (event.kind) {
    case "prompt_queued":
      return promptQueued(state, event, at);
    case "prompt_withdrawn":
      return updateTurn(state, event.turn_id, (turn) => endTurn(turn, endOf("withdrawn", at)));
    case "turn_started":
      return updateTurn(state, event.turn_id, (turn) => {
        const cwd = event.cwd;
        const mode = event.settings?.mode ?? turn.mode;
        if (turn.started && turn.cwd === cwd && turn.mode === mode && turn.startedAt === at) {
          return turn;
        }
        return { ...turn, started: true, startedAt: at ?? turn.startedAt, cwd, mode };
      });
    case "assistant_message_updated":
      return updateTurn(state, event.turn_id, (turn) =>
        addPart(
          updateMessage(turn, event.index, (message) =>
            appendToMessage(message, event.offset, event.delta),
          ),
          { kind: "message", index: event.index },
        ),
      );
    case "assistant_message_completed":
      return updateTurn(state, event.turn_id, (turn) =>
        addPart(
          updateMessage(turn, event.index, (message) => complete(message, event.text)),
          { kind: "message", index: event.index },
        ),
      );
    case "tool_call_started":
      return updateTurn(state, event.turn_id, (turn) => {
        const withPart = addPart(turn, { kind: "tool", callId: event.call_id });
        const tools = new Map(withPart.tools);
        const held = withPart.tools.get(event.call_id);
        tools.set(event.call_id, {
          ...(held ?? newTool(event.call_id)),
          tool: event.tool,
          summary: inputSummary(event.tool, event.input),
          startedAt: at,
        });
        return { ...withPart, tools, toolInputs: new Map(), toolStartedSeq: seq };
      });
    case "tool_call_output_updated":
      return updateTurn(state, event.turn_id, (turn) =>
        updateTool(turn, event.call_id, (call) =>
          call.tail === event.tail && call.bytes === event.bytes
            ? call
            : { ...call, tail: event.tail, bytes: event.bytes },
        ),
      );
    case "tool_call_completed":
      return updateTurn(state, event.turn_id, (turn) =>
        updateTool(addPart(turn, { kind: "tool", callId: event.call_id }), event.call_id, (call) =>
          completeTool(call, event, at),
        ),
      );
    case "approval_requested":
      return updateTurn(state, event.turn_id, (turn) => askApproval(turn, event, at));
    case "approval_resolved":
      return updateTurn(state, event.turn_id, (turn) =>
        updateApproval(turn, event.call_id, (approval) => ({
          ...approval,
          status: event.decision === "deny" ? "denied" : "allowed",
          decision: event.decision,
          answeredBy: event.origin,
          closedAt: at,
        })),
      );
    case "approval_expired":
      return updateTurn(state, event.turn_id, (turn) =>
        updateApproval(turn, event.call_id, (approval) =>
          approval.status === "pending" || approval.status === "settled"
            ? {
                ...approval,
                status: event.away === true ? "expired_away" : "expired",
                closedAt: at,
              }
            : approval,
        ),
      );
    case "turn_steered":
      return updateTurn(state, event.turn_id, (turn) => {
        if (turn.steers.has(seq)) {
          return turn;
        }
        const steers = new Map(turn.steers);
        steers.set(seq, { seq, text: event.text, at, status: "sent", resent: false });
        return addPart({ ...turn, steers }, { kind: "steer", seq });
      });
    case "steering_delivered":
      return updateTurn(state, event.turn_id, (turn) =>
        updateSteers(turn, event.steers, (steer) =>
          steer.status === "sent" ? { ...steer, status: "delivered" } : steer,
        ),
      );
    case "steering_withdrawn":
      return updateTurn(state, event.turn_id, (turn) =>
        updateSteers(turn, event.steers, (steer) =>
          steer.status === "withdrawn" ? steer : { ...steer, status: "withdrawn" },
        ),
      );
    case "turn_interrupt_requested":
      return updateTurn(state, event.turn_id, (turn) =>
        turn.interruptedBy === event.origin ? turn : { ...turn, interruptedBy: event.origin },
      );
    case "turn_completed": {
      const stop = event.stop?.reason;
      return turnEnded(state, event.turn_id, {
        ...endOf("completed", at),
        context: event.context ?? undefined,
        stop: stop === "end_turn" ? undefined : stop,
        usage: event.usage ?? undefined,
      });
    }
    case "turn_failed":
      return turnEnded(state, event.turn_id, {
        ...endOf("failed", at),
        context: event.context ?? undefined,
        error: event.error.message,
        usage: event.usage ?? undefined,
      });
    case "turn_interrupted":
      return turnEnded(state, event.turn_id, {
        ...endOf("interrupted", at),
        context: event.context ?? undefined,
        usage: event.usage ?? undefined,
      });
    case "turn_cancelled":
      return turnEnded(state, event.turn_id, endOf("cancelled", at));
    case "conversation_compacted":
      return state.compacting ? { ...state, compacting: false } : state;
    default:
      return state;
  }
}

function promptQueued(
  state: StreamState,
  event: EventOf<"prompt_queued">,
  at: number | undefined,
): StreamState {
  const next = updateTurn(state, event.turn_id, (turn) =>
    turn.prompt !== undefined
      ? turn
      : {
          ...turn,
          prompt: { text: event.text, origin: event.origin, commandId: event.command_id, at },
        },
  );
  const carried = event.steers;
  if (carried === undefined || carried.length === 0) {
    return next;
  }
  // The prompt carries steers that an interrupt sent again: it shows them from now on.
  let out = next;
  for (const turnId of next.order) {
    out = updateTurn(out, turnId, (turn) =>
      updateSteers(turn, carried, (steer) => (steer.resent ? steer : { ...steer, resent: true })),
    );
  }
  return out;
}

function turnEnded(state: StreamState, turnId: string, end: TurnEnd): StreamState {
  const next = updateTurn(state, turnId, (turn) => endTurn(turn, end));
  if (next === state) {
    return state;
  }
  const context = end.context ?? next.context;
  return { ...next, compacting: false, context };
}

function endOf(outcome: TurnOutcome, at: number | undefined): TurnEnd {
  return {
    outcome,
    at,
    context: undefined,
    error: undefined,
    stop: undefined,
    usage: undefined,
  };
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

function endTurn(turn: TurnView, end: TurnEnd): TurnView {
  if (turn.end !== undefined) {
    return turn;
  }
  const messages = new Map<number, MessageView>();
  for (const [index, message] of turn.messages) {
    messages.set(index, message.final ? message : { ...message, final: true });
  }
  // A question that is still open when its turn ends can never be answered.
  let expired: Map<string, ApprovalView> | undefined;
  for (const [callId, approval] of turn.approvals) {
    if (approval.status === "pending") {
      expired ??= new Map(turn.approvals);
      expired.set(callId, { ...approval, status: "expired", closedAt: end.at });
    }
  }
  const approvals = expired ?? turn.approvals;
  return { ...turn, end, messages, approvals, toolInputs: new Map() };
}

function completeTool(
  call: ToolCallView,
  event: EventOf<"tool_call_completed">,
  at: number | undefined,
): ToolCallView {
  if (call.done) {
    return call;
  }
  return {
    ...call,
    done: true,
    isError: event.is_error,
    exitCode: event.exit_code ?? undefined,
    output: event.output,
    refusal: event.refusal ?? undefined,
    diff: event.diff ?? undefined,
    leftRunning: event.left_running === true,
    endedAt: at,
  };
}

function askApproval(
  turn: TurnView,
  event: EventOf<"approval_requested">,
  at: number | undefined,
): TurnView {
  if (turn.approvals.has(event.call_id)) {
    return turn;
  }
  const expiresIn = event.expires_in_secs ?? undefined;
  const exit = event.exit ?? undefined;
  const approval: ApprovalView = {
    callId: event.call_id,
    turnId: event.turn_id,
    summary: event.summary,
    diffPreview: event.diff_preview ?? undefined,
    reason: event.reason ?? undefined,
    modelReason: exit?.model_reason ?? undefined,
    exitKinds: exit?.kinds ?? [],
    always: exit?.always ?? undefined,
    localOnly: event.local_only === true,
    interactive: event.interactive === true,
    askedAt: at,
    expiresAt: at === undefined || expiresIn === undefined ? undefined : at + expiresIn * 1000,
    status: "pending",
    decision: undefined,
    answeredBy: undefined,
    closedAt: undefined,
  };
  const approvals = new Map(turn.approvals);
  approvals.set(event.call_id, approval);
  return addPart({ ...turn, approvals }, { kind: "approval", callId: event.call_id });
}

function keepPrevious(previous: StreamState, next: StreamState, waits: boolean): StreamState {
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
      turns.set(turnId, waits ? old : settle(old));
      older.push(turnId);
    } else {
      turns.set(turnId, mergeTurn(old, rebuilt, waits));
    }
  }
  return {
    ...next,
    lastSeq: Math.max(previous.lastSeq, next.lastSeq),
    turns,
    order: [...older, ...next.order],
    context: next.context ?? previous.context,
  };
}

function mergeTurn(old: TurnView, rebuilt: TurnView, waits: boolean): TurnView {
  const messages = new Map(rebuilt.messages);
  for (const [index, held] of old.messages) {
    const message = rebuilt.messages.get(index);
    if (message === undefined) {
      // The snapshot window starts after this message's events.
      messages.set(
        index,
        rebuilt.end !== undefined && !held.final ? { ...held, final: true } : held,
      );
      continue;
    }
    let merged = message;
    if (!message.final && held.buffer.text.startsWith(message.buffer.text)) {
      merged = { ...message, buffer: seed(message.buffer, held.buffer.text) };
    }
    // The generation counts from what the app showed: the held message.
    const appends = merged.buffer.text.startsWith(held.buffer.text);
    const generation = appends ? held.generation : held.generation + 1;
    // The same message keeps its object, so its row does not render again.
    const same =
      merged.final === held.final &&
      generation === held.generation &&
      merged.buffer.text === held.buffer.text &&
      merged.buffer.pending.length === 0 &&
      held.buffer.pending.length === 0;
    messages.set(index, same ? held : { ...merged, generation });
  }
  const reasoning =
    rebuilt.end !== undefined || !old.reasoning.text.startsWith(rebuilt.reasoning.text)
      ? rebuilt.reasoning
      : seed(rebuilt.reasoning, old.reasoning.text);
  const parts = [...old.parts];
  for (const part of rebuilt.parts) {
    if (!parts.some((seen) => samePart(seen, part))) {
      parts.push(part);
    }
  }
  // Approvals that only the old state holds come first: the window starts after their
  // question.
  const approvals = new Map<string, ApprovalView>();
  for (const [callId, approval] of old.approvals) {
    if (!rebuilt.approvals.has(callId)) {
      approvals.set(callId, waits ? approval : settled(approval));
    }
  }
  for (const [callId, approval] of rebuilt.approvals) {
    approvals.set(callId, approval);
  }
  return {
    ...rebuilt,
    prompt:
      rebuilt.prompt !== undefined &&
      old.prompt !== undefined &&
      sameValues(old.prompt, rebuilt.prompt)
        ? old.prompt
        : (rebuilt.prompt ?? old.prompt),
    started: rebuilt.started || old.started,
    startedAt: rebuilt.startedAt ?? old.startedAt,
    cwd: rebuilt.cwd ?? old.cwd,
    mode: rebuilt.mode ?? old.mode,
    interruptedBy: rebuilt.interruptedBy ?? old.interruptedBy,
    parts,
    messages,
    reasoning,
    title: rebuilt.title ?? old.title,
    thoughtEndedAt: rebuilt.thoughtEndedAt ?? old.thoughtEndedAt,
    tools: mergeParts(old.tools, rebuilt.tools),
    approvals: mergeParts(old.approvals, approvals),
    steers: mergeParts(old.steers, rebuilt.steers),
  };
}

/**
 * The parts of `old` and then those of `rebuilt`, where a rebuilt part that holds the
 * same values as the old one keeps the old object, so its row does not render again.
 */
function mergeParts<K, V extends object>(
  old: ReadonlyMap<K, V>,
  rebuilt: ReadonlyMap<K, V>,
): ReadonlyMap<K, V> {
  const out = new Map(old);
  for (const [key, part] of rebuilt) {
    const held = old.get(key);
    out.set(key, held !== undefined && sameValues(held, part) ? held : part);
  }
  return out;
}

function sameValues(a: object, b: object): boolean {
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) {
    return false;
  }
  return keys.every((key) => {
    const x: unknown = Reflect.get(a, key);
    const y: unknown = Reflect.get(b, key);
    return Object.is(x, y) || (Array.isArray(x) && Array.isArray(y) && sameArray(x, y));
  });
}

function sameArray(a: readonly unknown[], b: readonly unknown[]): boolean {
  return a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
}

/** `turn` with its pending approvals settled. */
function settle(turn: TurnView): TurnView {
  let approvals: Map<string, ApprovalView> | undefined;
  for (const [callId, approval] of turn.approvals) {
    if (approval.status === "pending") {
      approvals ??= new Map(turn.approvals);
      approvals.set(callId, settled(approval));
    }
  }
  return approvals === undefined ? turn : { ...turn, approvals };
}

function settled(approval: ApprovalView): ApprovalView {
  return approval.status === "pending" ? { ...approval, status: "settled" } : approval;
}

function addPart(turn: TurnView, part: PartKey): TurnView {
  if (turn.parts.some((seen) => samePart(seen, part))) {
    return turn;
  }
  return { ...turn, parts: [...turn.parts, part] };
}

/** True when `a` and `b` name the same part. */
export function samePart(a: PartKey, b: PartKey): boolean {
  switch (a.kind) {
    case "message":
      return b.kind === "message" && a.index === b.index;
    case "steer":
      return b.kind === "steer" && a.seq === b.seq;
    case "tool":
      return b.kind === "tool" && a.callId === b.callId;
    case "approval":
      return b.kind === "approval" && a.callId === b.callId;
    default:
      return false;
  }
}

/** A draft only comes for a running turn: it began even when its start is not seen. */
function begin(turn: TurnView): TurnView {
  return turn.started ? turn : { ...turn, started: true };
}

function newTurn(turnId: string): TurnView {
  return {
    turnId,
    prompt: undefined,
    started: false,
    startedAt: undefined,
    cwd: undefined,
    mode: undefined,
    interruptedBy: undefined,
    end: undefined,
    parts: [],
    messages: new Map(),
    reasoning: EMPTY_BUFFER,
    title: undefined,
    thoughtEndedAt: undefined,
    toolInputs: new Map(),
    toolStartedSeq: 0,
    tools: new Map(),
    approvals: new Map(),
    steers: new Map(),
  };
}

function newTool(callId: string): ToolCallView {
  return {
    callId,
    tool: "",
    summary: "",
    tail: "",
    bytes: 0,
    done: false,
    isError: false,
    exitCode: undefined,
    output: undefined,
    refusal: undefined,
    diff: undefined,
    leftRunning: false,
    startedAt: undefined,
    endedAt: undefined,
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
    existing ?? { index, buffer: EMPTY_BUFFER, final: turn.end !== undefined, generation: 0 },
  );
  if (message === existing) {
    return turn;
  }
  const messages = new Map(turn.messages);
  messages.set(index, message);
  return { ...turn, messages };
}

function updateTool(
  turn: TurnView,
  callId: string,
  update: (call: ToolCallView) => ToolCallView,
): TurnView {
  const existing = turn.tools.get(callId);
  const call = update(existing ?? newTool(callId));
  if (call === existing) {
    return turn;
  }
  const tools = new Map(turn.tools);
  tools.set(callId, call);
  return { ...turn, tools };
}

function updateApproval(
  turn: TurnView,
  callId: string,
  update: (approval: ApprovalView) => ApprovalView,
): TurnView {
  const existing = turn.approvals.get(callId);
  if (existing === undefined) {
    // The question is older than what the app holds.
    return turn;
  }
  const approval = update(existing);
  if (approval === existing) {
    return turn;
  }
  const approvals = new Map(turn.approvals);
  approvals.set(callId, approval);
  return { ...turn, approvals };
}

function updateSteers(
  turn: TurnView,
  seqs: readonly number[],
  update: (steer: SteerView) => SteerView,
): TurnView {
  let steers: Map<number, SteerView> | undefined;
  for (const seq of seqs) {
    const existing = turn.steers.get(seq);
    if (existing === undefined) {
      continue;
    }
    const steer = update(existing);
    if (steer !== existing) {
      steers ??= new Map(turn.steers);
      steers.set(seq, steer);
    }
  }
  return steers === undefined ? turn : { ...turn, steers };
}

function sameContext(a: ContextUse | undefined, b: ContextUse): boolean {
  return a !== undefined && a.tokens === b.tokens && a.limit === b.limit && a.window === b.window;
}

/** The turn that runs now: started and not ended. */
export function runningTurn(state: StreamState): TurnView | undefined {
  for (let i = state.order.length - 1; i >= 0; i--) {
    const turn = state.turns.get(state.order[i] ?? "");
    if (turn !== undefined && turn.started && turn.end === undefined) {
      return turn;
    }
  }
  return undefined;
}

/** True when the turn's prompt waits in the queue: sent, not started and not ended. */
export function isQueued(turn: TurnView): boolean {
  return turn.prompt !== undefined && !turn.started && turn.end === undefined;
}

/** The approvals of the conversation that wait for an answer, oldest first. */
export function pendingApprovals(state: StreamState): ApprovalView[] {
  const pending: ApprovalView[] = [];
  for (const turnId of state.order) {
    for (const approval of state.turns.get(turnId)?.approvals.values() ?? []) {
      if (approval.status === "pending") {
        pending.push(approval);
      }
    }
  }
  return pending;
}

/** The approval of `callId`, in any turn. */
export function findApproval(state: StreamState, callId: string): ApprovalView | undefined {
  for (const turnId of state.order) {
    const approval = state.turns.get(turnId)?.approvals.get(callId);
    if (approval !== undefined) {
      return approval;
    }
  }
  return undefined;
}

/** One line about a tool call's input: the command, the path, or the first line of text. */
export function inputSummary(tool: string, input: unknown): string {
  if (typeof input === "string") {
    return firstLine(input);
  }
  if (typeof input === "object" && input !== null && !Array.isArray(input)) {
    for (const key of ["command", "cmd", "path", "file_path", "pattern", "query", "url"]) {
      const value: unknown = Reflect.get(input, key);
      if (typeof value === "string" && value.length > 0) {
        return firstLine(value);
      }
      if (Array.isArray(value) && value.every((part) => typeof part === "string")) {
        return firstLine(value.join(" "));
      }
    }
  }
  return tool;
}

function firstLine(text: string): string {
  const line = text.trimStart().split("\n", 1)[0] ?? "";
  return line.length > 200 ? `${line.slice(0, 199)}…` : line;
}

function time(value: string): number | undefined {
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? undefined : ms;
}
