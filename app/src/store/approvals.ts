// The approvals of one conversation, built from its events. It is pure, like the stream
// reducer: each function returns a new state and shares what did not change.
//
// efr's rules (R3.10 of the research): the first answer wins, from any client. The phone
// never answers a question that only the user's own machine may answer (`local_only`),
// and never answers with `allow_always`. An approval can expire: `approval_expired`
// says so, with `away` when the user was away. The app shows both in clear words.

import type { EventEnvelope, EventOf, Snapshot } from "../core/wire";
import type { ApprovalDecision, Origin } from "../protocol/generated";

/** What became of an approval. */
export type ApprovalStatus =
  | "pending"
  | "allowed"
  | "denied"
  | "allowed_always"
  | "expired"
  /** It expired while the user was away. */
  | "expired_away"
  /** It was pending, and a snapshot showed later that the conversation waits no more. */
  | "settled";

/** One approval question of a turn. */
export interface ApprovalView {
  readonly callId: string;
  readonly turnId: string;
  readonly summary: string;
  readonly diffPreview: string | undefined;
  /** Only the user's own machine may answer it; the phone shows it but cannot answer. */
  readonly localOnly: boolean;
  /** A person must answer it. */
  readonly interactive: boolean;
  /** When efrd recorded the question (an RFC 3339 time). */
  readonly requestedAt: string;
  /** How long efrd waits for an answer, from `requestedAt`; absent when it waits. */
  readonly expiresInSecs: number | undefined;
  readonly status: ApprovalStatus;
  /** The surface that answered, once resolved. */
  readonly decidedBy: Origin | undefined;
}

/** The approvals of one conversation. */
export interface ApprovalsState {
  /** The largest sequence number applied; older events are duplicates. */
  readonly lastSeq: number;
  readonly byCall: ReadonlyMap<string, ApprovalView>;
  /** Call ids in the order of their questions. */
  readonly order: readonly string[];
}

export const EMPTY_APPROVALS: ApprovalsState = { lastSeq: 0, byCall: new Map(), order: [] };

const RESOLVED: Readonly<Record<ApprovalDecision, ApprovalStatus>> = {
  allow: "allowed",
  deny: "denied",
  allow_always: "allowed_always",
};

/** Applies one event. Kinds other than the approval events change nothing. */
export function applyApprovalEvent(state: ApprovalsState, envelope: EventEnvelope): ApprovalsState {
  if (envelope.seq <= state.lastSeq) {
    return state;
  }
  const event = envelope.event;
  let next: ApprovalsState;
  switch (event.kind) {
    case "approval_requested":
      next = put(state, requested(event, envelope.at));
      break;
    case "approval_resolved":
      next = update(state, event.call_id, (view) => ({
        ...view,
        status: RESOLVED[event.decision],
        decidedBy: event.origin,
      }));
      break;
    case "approval_expired":
      next = update(state, event.call_id, (view) =>
        view.status === "pending"
          ? { ...view, status: event.away === true ? "expired_away" : "expired" }
          : view,
      );
      break;
    default:
      return { ...state, lastSeq: envelope.seq };
  }
  return { ...next, lastSeq: envelope.seq };
}

/**
 * Applies a snapshot: the approvals are rebuilt from its events. Approvals of the
 * previous state that the snapshot window does not hold stay; a pending one becomes
 * `settled` when the conversation no longer waits for an approval.
 */
export function applyApprovalSnapshot(
  state: ApprovalsState,
  snapshot: Pick<Snapshot, "events" | "hwm" | "conversation">,
): ApprovalsState {
  let rebuilt = EMPTY_APPROVALS;
  for (const envelope of snapshot.events) {
    rebuilt = applyApprovalEvent(rebuilt, envelope);
  }
  const waits = snapshot.conversation.status === "awaiting_approval";
  const byCall = new Map(rebuilt.byCall);
  const older: string[] = [];
  for (const callId of state.order) {
    const held = state.byCall.get(callId);
    if (held === undefined || byCall.has(callId)) {
      continue;
    }
    byCall.set(callId, held.status === "pending" && !waits ? { ...held, status: "settled" } : held);
    older.push(callId);
  }
  return {
    lastSeq: Math.max(state.lastSeq, rebuilt.lastSeq, snapshot.hwm),
    byCall,
    order: [...older, ...rebuilt.order],
  };
}

/** The pending approvals, oldest first. */
export function pendingApprovals(state: ApprovalsState): ApprovalView[] {
  return state.order
    .map((callId) => state.byCall.get(callId))
    .filter((view): view is ApprovalView => view?.status === "pending");
}

/** The answers that the phone may give to `view`: none when it is not pending or local only. */
export function phoneDecisions(view: ApprovalView): readonly ApprovalDecision[] {
  return view.status === "pending" && !view.localOnly ? ["allow", "deny"] : [];
}

/** When efrd stops waiting for an answer to `view`, in ms since the epoch, if it does. */
export function expiresAt(view: ApprovalView): number | undefined {
  if (view.expiresInSecs === undefined) {
    return undefined;
  }
  const at = Date.parse(view.requestedAt);
  return Number.isNaN(at) ? undefined : at + view.expiresInSecs * 1000;
}

function requested(event: EventOf<"approval_requested">, at: string): ApprovalView {
  return {
    callId: event.call_id,
    turnId: event.turn_id,
    summary: event.summary,
    diffPreview: event.diff_preview ?? undefined,
    localOnly: event.local_only === true,
    interactive: event.interactive === true,
    requestedAt: at,
    expiresInSecs: event.expires_in_secs ?? undefined,
    status: "pending",
    decidedBy: undefined,
  };
}

function put(state: ApprovalsState, view: ApprovalView): ApprovalsState {
  const byCall = new Map(state.byCall);
  const known = byCall.has(view.callId);
  byCall.set(view.callId, view);
  return { ...state, byCall, order: known ? state.order : [...state.order, view.callId] };
}

function update(
  state: ApprovalsState,
  callId: string,
  change: (view: ApprovalView) => ApprovalView,
): ApprovalsState {
  const view = state.byCall.get(callId);
  if (view === undefined) {
    // The question is older than what the app holds.
    return state;
  }
  const next = change(view);
  if (next === view) {
    return state;
  }
  const byCall = new Map(state.byCall);
  byCall.set(callId, next);
  return { ...state, byCall };
}
