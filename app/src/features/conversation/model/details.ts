// The transcript details: what the screens show next to the streamed text, built from
// the same `conversation.subscribe` items as the stream reducer (src/lib/stream/reducer.ts).
// The stream reducer holds the text of the assistant messages, the reasoning and the
// tool output; this reducer holds the rest:
//
// - the order of the parts of each turn (messages, tool calls, approvals, steers), by
//   first sight, because the stream reducer keeps them in maps by kind;
// - the prompt of each turn, how the turn ended and how full the context was then;
// - the input summary and the result of each tool call;
// - the approval questions with their state (pending, answered, expired);
// - the `ctx N%` of the running turn from its `context` drafts, and `compacting`.
//
// The store runs both reducers on each item. The input is the item as JSON (the wire
// shape of efr-protocol's `ConversationSubscribeItem`), and every member is checked
// where it is read, so any decoder of the app can feed it: a member that is missing or
// has another type leaves its field out. Like the stream reducer, it is pure, keeps
// every part that did not change, and skips an event at or below the last sequence
// number.

import type { ContextUse } from "./format";

/** The key of one part of a turn, in the order the parts came. */
export type PartKey =
  | { readonly kind: "message"; readonly index: number }
  | { readonly kind: "tool"; readonly callId: string }
  | { readonly kind: "approval"; readonly callId: string }
  | { readonly kind: "steer"; readonly seq: number };

/** The prompt of a turn. */
export interface PromptView {
  readonly text: string;
  /** The surface that sent it, such as `terminal` or `phone`. */
  readonly origin: string;
}

/** How a turn ended. */
export type TurnOutcome = "completed" | "failed" | "interrupted" | "cancelled" | "withdrawn";

/** The end of a turn. */
export interface TurnEnd {
  readonly outcome: TurnOutcome;
  readonly context?: ContextUse;
  /** The error's sentence, for a failed turn. */
  readonly error?: string;
  /** The stop reason of a completed turn, when the model stopped for another reason. */
  readonly stop?: string;
  /** When the turn ended, in ms since the epoch. */
  readonly at?: number;
}

/** A tool call, as its events tell it. */
export interface ToolDetails {
  readonly tool: string;
  /** One line about the input, such as the command of a `shell` call. */
  readonly summary: string;
  readonly done: boolean;
  readonly isError?: boolean;
  readonly exitCode?: number;
  /** Why efr refused the call before it ran. */
  readonly refusal?: string;
  /** The unified diff of a file tool's call. */
  readonly diff?: string;
  /** The command still runs in the hidden shell after the call answered. */
  readonly leftRunning?: boolean;
}

/** The state of an approval question. */
export type ApprovalState = "pending" | "resolved" | "expired";

/** An approval question of a tool call. */
export interface ApprovalView {
  readonly callId: string;
  readonly turnId: string;
  readonly summary: string;
  readonly diffPreview?: string;
  /** The call may wait for input at the terminal, such as a `sudo` password. */
  readonly interactive: boolean;
  /** Only the user's own machine may answer: the phone shows the question but no buttons. */
  readonly localOnly: boolean;
  /** The model's reason for an exit of the sandbox. */
  readonly reason?: string;
  /** When efrd asked, in ms since the epoch. */
  readonly askedAt?: number;
  /** When the question stops waiting, in ms since the epoch, for a fixed limit. */
  readonly expiresAt?: number;
  readonly state: ApprovalState;
  /** The answer: `allow`, `deny` or `allow_always`. */
  readonly decision?: string;
  /** The surface that answered. */
  readonly answeredBy?: string;
  /** The question expired while the user was away. */
  readonly away: boolean;
}

/** One turn. */
export interface TurnDetails {
  readonly turnId: string;
  readonly prompt?: PromptView;
  /** True once the turn began running; a queued prompt has not. */
  readonly started: boolean;
  readonly end?: TurnEnd;
  readonly parts: readonly PartKey[];
  readonly tools: ReadonlyMap<string, ToolDetails>;
  readonly approvals: ReadonlyMap<string, ApprovalView>;
  /** The text of each steer, by its sequence number. */
  readonly steers: ReadonlyMap<number, string>;
}

/** The details of one conversation. */
export interface TranscriptDetails {
  readonly lastSeq: number;
  readonly turns: ReadonlyMap<string, TurnDetails>;
  /** Turn ids, oldest first. */
  readonly order: readonly string[];
  /** How full the context is: from the running turn's drafts, else the last turn end. */
  readonly context?: ContextUse;
  /** The running turn compacts the context now. */
  readonly compacting: boolean;
}

/** The details before the first item. */
export const EMPTY_DETAILS: TranscriptDetails = {
  lastSeq: 0,
  turns: new Map(),
  order: [],
  compacting: false,
};

type Json = Readonly<Record<string, unknown>>;

const OUTCOMES = {
  turn_completed: "completed",
  turn_failed: "failed",
  turn_interrupted: "interrupted",
  turn_cancelled: "cancelled",
} as const satisfies Record<string, TurnOutcome>;

/** Applies one item of the subscribe stream, as JSON. */
export function reduceDetails(state: TranscriptDetails, item: unknown): TranscriptDetails {
  if (!isObject(item)) {
    return state;
  }
  switch (item["kind"]) {
    case "event":
      return applyEnvelope(state, item);
    case "draft":
      return applyDraft(state, item);
    case "snapshot":
      return applySnapshot(state, item);
    default:
      return state;
  }
}

/** The approvals of the conversation that wait for an answer, oldest first. */
export function pendingApprovals(state: TranscriptDetails): ApprovalView[] {
  const pending: ApprovalView[] = [];
  for (const turnId of state.order) {
    for (const approval of state.turns.get(turnId)?.approvals.values() ?? []) {
      if (approval.state === "pending") {
        pending.push(approval);
      }
    }
  }
  return pending;
}

/** The turn that runs now: started and not ended. */
export function runningTurn(state: TranscriptDetails): TurnDetails | undefined {
  for (let i = state.order.length - 1; i >= 0; i--) {
    const turn = state.turns.get(state.order[i] ?? "");
    if (turn !== undefined && turn.started && turn.end === undefined) {
      return turn;
    }
  }
  return undefined;
}

function applyEnvelope(state: TranscriptDetails, envelope: Json): TranscriptDetails {
  const seq = num(envelope["seq"]);
  if (seq === undefined || seq <= state.lastSeq) {
    return state;
  }
  const event = envelope["event"];
  const at = time(envelope["at"]);
  const next = isObject(event) ? applyEvent(state, event, seq, at) : state;
  return { ...next, lastSeq: seq };
}

function applyEvent(
  state: TranscriptDetails,
  event: Json,
  seq: number,
  at: number | undefined,
): TranscriptDetails {
  const turnId = str(event["turn_id"]);
  if (turnId === undefined) {
    return state;
  }
  const callId = str(event["call_id"]);
  switch (event["kind"]) {
    case "prompt_queued": {
      const text = str(event["text"]);
      return updateTurn(state, turnId, (turn) =>
        text === undefined || turn.prompt !== undefined
          ? turn
          : { ...turn, prompt: { text, origin: str(event["origin"]) ?? "" } },
      );
    }
    case "prompt_withdrawn":
      return updateTurn(state, turnId, (turn) =>
        endTurn(turn, { outcome: "withdrawn", ...atField(at) }),
      );
    case "turn_started":
      return updateTurn(state, turnId, (turn) =>
        turn.started ? turn : { ...turn, started: true },
      );
    case "assistant_message_updated":
    case "assistant_message_completed": {
      const index = num(event["index"]);
      return index === undefined
        ? state
        : updateTurn(state, turnId, (turn) => addPart(turn, { kind: "message", index }));
    }
    case "tool_call_started": {
      if (callId === undefined) {
        return state;
      }
      const tool = str(event["tool"]) ?? "";
      return updateTurn(state, turnId, (turn) => {
        const withPart = addPart(turn, { kind: "tool", callId });
        if (withPart.tools.has(callId)) {
          return withPart;
        }
        const tools = new Map(withPart.tools);
        tools.set(callId, { tool, summary: inputSummary(tool, event["input"]), done: false });
        return { ...withPart, tools };
      });
    }
    case "tool_call_completed":
      return callId === undefined
        ? state
        : updateTurn(state, turnId, (turn) => completeTool(turn, callId, event));
    case "approval_requested":
      return callId === undefined
        ? state
        : updateTurn(state, turnId, (turn) => askApproval(turn, turnId, callId, event, at));
    case "approval_resolved":
      return callId === undefined
        ? state
        : updateTurn(state, turnId, (turn) =>
            updateApproval(turn, callId, (approval) => ({
              ...approval,
              state: "resolved",
              ...optional("decision", str(event["decision"])),
              ...optional("answeredBy", str(event["origin"])),
            })),
          );
    case "approval_expired":
      return callId === undefined
        ? state
        : updateTurn(state, turnId, (turn) =>
            updateApproval(turn, callId, (approval) =>
              approval.state === "resolved"
                ? approval
                : { ...approval, state: "expired", away: event["away"] === true },
            ),
          );
    case "turn_steered": {
      const text = str(event["text"]);
      return text === undefined
        ? state
        : updateTurn(state, turnId, (turn) => {
            const steers = new Map(turn.steers);
            steers.set(seq, text);
            return addPart({ ...turn, steers }, { kind: "steer", seq });
          });
    }
    case "turn_completed":
    case "turn_failed":
    case "turn_interrupted":
    case "turn_cancelled": {
      const outcome = OUTCOMES[event["kind"]];
      const context = contextUse(event["context"]);
      const error = isObject(event["error"]) ? str(event["error"]["message"]) : undefined;
      const stop = stopReason(event["stop"]);
      const next = updateTurn(state, turnId, (turn) =>
        endTurn(turn, {
          outcome,
          ...optional("context", context),
          ...optional("error", error),
          ...optional("stop", stop),
          ...atField(at),
        }),
      );
      return {
        ...next,
        compacting: false,
        ...(context === undefined ? {} : { context }),
      };
    }
    case "conversation_compacted":
      return state.compacting ? { ...state, compacting: false } : state;
    default:
      return state;
  }
}

function applyDraft(state: TranscriptDetails, draft: Json): TranscriptDetails {
  const turnId = str(draft["turn_id"]);
  const part = draft["draft"];
  if (turnId === undefined || !isObject(part)) {
    return state;
  }
  const turn = state.turns.get(turnId);
  if (turn?.end !== undefined) {
    return state;
  }
  switch (part["kind"]) {
    case "text": {
      const index = num(part["index"]);
      return index === undefined
        ? state
        : updateTurn(state, turnId, (current) => addPart(current, { kind: "message", index }));
    }
    case "context": {
      const context = contextUse(part);
      return context === undefined ? state : { ...state, context, compacting: false };
    }
    case "compacting":
      return state.compacting ? state : { ...state, compacting: true };
    default:
      return state;
  }
}

/**
 * A snapshot rebuilds the details from its events. Turns of the previous state that the
 * snapshot does not reach stay, before the others, and a rebuilt turn keeps what the
 * previous state knew of its start (the prompt and the first parts), because the
 * snapshot's window can begin in the middle of a turn.
 */
function applySnapshot(state: TranscriptDetails, snapshot: Json): TranscriptDetails {
  let next = EMPTY_DETAILS;
  const events = Array.isArray(snapshot["events"]) ? snapshot["events"] : [];
  for (const envelope of events) {
    if (isObject(envelope)) {
      next = applyEnvelope(next, envelope);
    }
  }
  const hwm = num(snapshot["hwm"]);
  if (hwm !== undefined && hwm > next.lastSeq) {
    next = { ...next, lastSeq: hwm };
  }
  const live = snapshot["live"];
  if (isObject(live)) {
    const turnId = str(live["turn_id"]);
    const index = num(live["index"]);
    if (turnId !== undefined) {
      next = updateTurn(next, turnId, (turn) => {
        const started = turn.started ? turn : { ...turn, started: true };
        return index === undefined || str(live["text"]) === ""
          ? started
          : addPart(started, { kind: "message", index });
      });
    }
  }
  return merge(state, next);
}

function merge(previous: TranscriptDetails, next: TranscriptDetails): TranscriptDetails {
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
      continue;
    }
    const parts = [...old.parts];
    for (const part of rebuilt.parts) {
      if (!parts.some((seen) => samePart(seen, part))) {
        parts.push(part);
      }
    }
    turns.set(turnId, {
      ...rebuilt,
      started: rebuilt.started || old.started,
      parts,
      tools: new Map([...old.tools, ...rebuilt.tools]),
      approvals: new Map([...old.approvals, ...rebuilt.approvals]),
      steers: new Map([...old.steers, ...rebuilt.steers]),
      ...optional("prompt", rebuilt.prompt ?? old.prompt),
    });
  }
  const context = next.context ?? previous.context;
  return {
    lastSeq: Math.max(previous.lastSeq, next.lastSeq),
    turns,
    order: [...older, ...next.order],
    compacting: next.compacting,
    ...optional("context", context),
  };
}

function askApproval(
  turn: TurnDetails,
  turnId: string,
  callId: string,
  event: Json,
  at: number | undefined,
): TurnDetails {
  if (turn.approvals.has(callId)) {
    return turn;
  }
  const expiresIn = num(event["expires_in_secs"]);
  const exit = event["exit"];
  const reason = isObject(exit) ? str(exit["model_reason"]) : undefined;
  const approval: ApprovalView = {
    callId,
    turnId,
    summary: str(event["summary"]) ?? "",
    interactive: event["interactive"] === true,
    localOnly: event["local_only"] === true,
    state: "pending",
    away: false,
    ...optional("diffPreview", str(event["diff_preview"])),
    ...optional("reason", reason),
    ...optional("askedAt", at),
    ...optional(
      "expiresAt",
      at === undefined || expiresIn === undefined ? undefined : at + expiresIn * 1000,
    ),
  };
  const approvals = new Map(turn.approvals);
  approvals.set(callId, approval);
  return addPart({ ...turn, approvals }, { kind: "approval", callId });
}

function updateApproval(
  turn: TurnDetails,
  callId: string,
  update: (approval: ApprovalView) => ApprovalView,
): TurnDetails {
  const existing = turn.approvals.get(callId);
  if (existing === undefined) {
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

function completeTool(turn: TurnDetails, callId: string, event: Json): TurnDetails {
  const existing = turn.tools.get(callId) ?? { tool: "", summary: "", done: false };
  const tools = new Map(turn.tools);
  tools.set(callId, {
    ...existing,
    done: true,
    isError: event["is_error"] === true,
    ...optional("exitCode", num(event["exit_code"], true)),
    ...optional("refusal", str(event["refusal"])),
    ...optional("diff", str(event["diff"])),
    ...(event["left_running"] === true ? { leftRunning: true } : {}),
  });
  return addPart({ ...turn, tools }, { kind: "tool", callId });
}

function endTurn(turn: TurnDetails, end: TurnEnd): TurnDetails {
  if (turn.end !== undefined) {
    return turn;
  }
  // A question that is still open when its turn ends can never be answered.
  let expired: Map<string, ApprovalView> | undefined;
  for (const [callId, approval] of turn.approvals) {
    if (approval.state === "pending") {
      expired ??= new Map(turn.approvals);
      expired.set(callId, { ...approval, state: "expired" });
    }
  }
  return { ...turn, end, approvals: expired ?? turn.approvals };
}

function addPart(turn: TurnDetails, part: PartKey): TurnDetails {
  if (turn.parts.some((seen) => samePart(seen, part))) {
    return turn;
  }
  return { ...turn, parts: [...turn.parts, part] };
}

/** True when `a` and `b` name the same part. */
export function samePart(a: PartKey, b: PartKey): boolean {
  if (a.kind === "message" || b.kind === "message") {
    return a.kind === "message" && b.kind === "message" && a.index === b.index;
  }
  if (a.kind === "steer" || b.kind === "steer") {
    return a.kind === "steer" && b.kind === "steer" && a.seq === b.seq;
  }
  return a.kind === b.kind && a.callId === b.callId;
}

function newTurn(turnId: string): TurnDetails {
  return {
    turnId,
    started: false,
    parts: [],
    tools: new Map(),
    approvals: new Map(),
    steers: new Map(),
  };
}

function updateTurn(
  state: TranscriptDetails,
  turnId: string,
  update: (turn: TurnDetails) => TurnDetails,
): TranscriptDetails {
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

/** One line about a tool call's input: the command, the path, or the first line of text. */
export function inputSummary(tool: string, input: unknown): string {
  if (typeof input === "string") {
    return firstLine(input);
  }
  if (isObject(input)) {
    for (const key of ["command", "cmd", "path", "file_path", "pattern", "query", "url"]) {
      const value = input[key];
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

function contextUse(value: unknown): ContextUse | undefined {
  if (!isObject(value)) {
    return undefined;
  }
  const tokens = num(value["tokens"]);
  const limit = num(value["limit"]);
  const window = num(value["window"]);
  return tokens === undefined || limit === undefined || window === undefined
    ? undefined
    : { tokens, limit, window };
}

function stopReason(value: unknown): string | undefined {
  if (typeof value === "string") {
    return value;
  }
  return isObject(value) ? str(value["kind"]) : undefined;
}

function atField(at: number | undefined): { at?: number } {
  return at === undefined ? {} : { at };
}

/** `{ [key]: value }`, or `{}` for an `undefined` value, to spread into an object. */
function optional<K extends string, V>(key: K, value: V | undefined): Partial<Record<K, V>> {
  const out: Partial<Record<K, V>> = {};
  if (value !== undefined) {
    out[key] = value;
  }
  return out;
}

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function str(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function num(value: unknown, signed = false): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && (signed || value >= 0)
    ? value
    : undefined;
}

function time(value: unknown): number | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? undefined : ms;
}
