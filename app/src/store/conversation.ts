// The store of one conversation: the stream reducer (src/core/reducer.ts), its
// approvals, the prompts of its turns and its summary, fed by a subscription of the
// client and by the event cache.
//
// The store lives outside React. A view reads it with `useSyncExternalStore(store.subscribe,
// store.getState)` and a selector: the state and each part of it keep their identity
// until they change, so a settled row never renders again (R4c.4 step 5).
//
// Order on open: `hydrate` first (the cached events, so a cold start shows the transcript
// at once), then the subscription, which resumes with `after_seq` from the cache.

import { decodeEventEnvelope, isJsonObject } from "../core/envelope";
import { EMPTY_STREAM, reduce, type StreamState } from "../core/reducer";
import type { EventEnvelope, SubscribeItem } from "../core/wire";
import type { SubscriptionSink, SubscriptionStatus } from "../net/client";
import type { UnaryMethodName } from "../net/connection";
import type {
  ApprovalDecision,
  ConversationSummary,
  MethodParams,
  MethodResults,
  Origin,
} from "../protocol/generated";
import {
  EMPTY_APPROVALS,
  applyApprovalEvent,
  applyApprovalSnapshot,
  phoneDecisions,
  type ApprovalsState,
} from "./approvals";
import type { CachedEvent, EventCache } from "./event_cache";

/** The prompt that started a turn. */
export interface PromptView {
  readonly turnId: string;
  readonly text: string;
  readonly origin: Origin;
}

/** The state of one conversation. */
export interface ConversationView {
  readonly conversationId: string;
  /** The summary of the last snapshot; absent before one came. */
  readonly summary: ConversationSummary | undefined;
  readonly stream: StreamState;
  readonly approvals: ApprovalsState;
  /** The prompt of each turn, by turn id. */
  readonly prompts: ReadonlyMap<string, PromptView>;
  readonly subscription: SubscriptionStatus["state"];
  /** True once the cache or efrd gave a state. */
  readonly loaded: boolean;
}

/** What the store needs of the client to answer an approval. */
export interface Caller {
  call<M extends UnaryMethodName>(method: M, params: MethodParams[M]): Promise<MethodResults[M]>;
}

/** Why the phone cannot give an answer. */
export class ApprovalRefused extends Error {
  readonly reason: "unknown" | "not_pending" | "local_only" | "decision";

  constructor(reason: ApprovalRefused["reason"], message: string) {
    super(message);
    this.name = "ApprovalRefused";
    this.reason = reason;
  }
}

/** The settings of a conversation store. */
export interface ConversationStoreOptions {
  readonly cache?: EventCache;
  /** Called with the summary of each snapshot, so the list can show it. */
  readonly onSummary?: (summary: ConversationSummary) => void;
  /** Called when the cache fails; the store goes on without it. */
  readonly onCacheError?: (error: unknown) => void;
  /**
   * Called with each item that the store applies, as JSON: the cached events (as
   * `event` items) and the items of the subscription. A view can run a reducer of its
   * own over them, such as the details of the transcript.
   */
  readonly onItem?: (item: unknown) => void;
}

/** The store of one conversation, and the sink of its subscription. */
export class ConversationStore implements SubscriptionSink {
  private readonly options: ConversationStoreOptions;
  private readonly listeners = new Set<() => void>();
  private view: ConversationView;
  private writes: Promise<void> = Promise.resolve();

  constructor(conversationId: string, options: ConversationStoreOptions = {}) {
    this.options = options;
    this.view = {
      conversationId,
      summary: undefined,
      stream: EMPTY_STREAM,
      approvals: EMPTY_APPROVALS,
      prompts: new Map(),
      subscription: "waiting",
      loaded: false,
    };
  }

  /** The state now. It keeps its identity until it changes. */
  readonly getState = (): ConversationView => this.view;

  /** Calls `listener` after each change. Returns the function that stops it. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** Shows the cached events. Call it once, before the subscription starts. */
  async hydrate(): Promise<void> {
    const cache = this.options.cache;
    if (cache === undefined) {
      return;
    }
    let rows: readonly CachedEvent[];
    try {
      rows = await cache.load(this.view.conversationId);
    } catch (error) {
      this.options.onCacheError?.(error);
      return;
    }
    let { stream, approvals, prompts } = this.view;
    const applied: unknown[] = [];
    for (const row of rows) {
      const parsed = parseEnvelope(row.json);
      if (parsed !== undefined) {
        const { envelope, raw } = parsed;
        stream = reduce(stream, { kind: "event", ...envelope });
        approvals = applyApprovalEvent(approvals, envelope);
        prompts = withPrompt(prompts, envelope);
        applied.push({ ...raw, kind: "event" });
      }
    }
    this.commit({
      ...this.view,
      stream,
      approvals,
      prompts,
      loaded: rows.length > 0 || this.view.loaded,
    });
    for (const item of applied) {
      this.options.onItem?.(item);
    }
  }

  /** The `after_seq` of the next subscribe: the largest sequence number seen. */
  cursor(): number | undefined {
    const seq = this.view.stream.lastSeq;
    return seq > 0 ? seq : undefined;
  }

  /** One item of the subscription. */
  item(item: SubscribeItem, raw: unknown): void {
    const view = this.view;
    switch (item.kind) {
      case "event": {
        if (item.seq <= view.stream.lastSeq) {
          return;
        }
        this.commit({
          ...view,
          stream: reduce(view.stream, item),
          approvals: applyApprovalEvent(view.approvals, item),
          prompts: withPrompt(view.prompts, item),
          loaded: true,
        });
        this.options.onItem?.(raw);
        this.cache((cache) =>
          cache.append(view.conversationId, cachedEvents([withoutKind(raw)], [item.seq])),
        );
        return;
      }
      case "snapshot": {
        let prompts = view.prompts;
        for (const envelope of item.events) {
          prompts = withPrompt(prompts, envelope);
        }
        this.commit({
          ...view,
          summary: item.conversation,
          stream: reduce(view.stream, item),
          approvals: applyApprovalSnapshot(view.approvals, item),
          prompts,
          loaded: true,
        });
        this.options.onSummary?.(item.conversation);
        this.options.onItem?.(raw);
        const rawEvents = isJsonObject(raw) && Array.isArray(raw["events"]) ? raw["events"] : [];
        this.cache((cache) =>
          cache.replace(
            view.conversationId,
            cachedEvents(
              rawEvents,
              item.events.map((envelope) => envelope.seq),
            ),
          ),
        );
        return;
      }
      case "draft": {
        const stream = reduce(view.stream, item);
        if (stream !== view.stream) {
          this.commit({ ...view, stream });
        }
        this.options.onItem?.(raw);
        return;
      }
      default:
        return;
    }
  }

  /** The subscription changed its state. */
  status(status: SubscriptionStatus): void {
    if (status.state !== this.view.subscription) {
      this.commit({ ...this.view, subscription: status.state });
    }
  }

  /**
   * Answers approval `callId` with `decision` (`allow` or `deny`). The phone refuses an
   * approval that is not pending or that only the user's machine may answer, and never
   * sends `allow_always`. efrd answers the first answer from any client; the result
   * shows when its `approval_resolved` event comes on the subscription.
   */
  async respond(
    caller: Caller,
    callId: string,
    decision: ApprovalDecision,
    commandId: string,
  ): Promise<void> {
    const view = this.view.approvals.byCall.get(callId);
    if (view === undefined) {
      throw new ApprovalRefused("unknown", "the app does not know this approval");
    }
    if (view.status !== "pending") {
      throw new ApprovalRefused("not_pending", "the approval has an answer or expired");
    }
    if (view.localOnly) {
      throw new ApprovalRefused("local_only", "only your computer can answer this approval");
    }
    if (!phoneDecisions(view).includes(decision)) {
      throw new ApprovalRefused("decision", "the phone cannot give this answer");
    }
    await caller.call("approval.respond", {
      command_id: commandId,
      conversation_id: this.view.conversationId,
      call_id: callId,
      decision,
    });
  }

  /** Waits for the cache writes that are queued now. */
  settled(): Promise<void> {
    return this.writes;
  }

  private commit(view: ConversationView): void {
    this.view = view;
    for (const listener of this.listeners) {
      listener();
    }
  }

  private cache(write: (cache: EventCache) => Promise<void>): void {
    const cache = this.options.cache;
    if (cache === undefined) {
      return;
    }
    this.writes = this.writes.then(
      () => write(cache).catch((error: unknown) => this.options.onCacheError?.(error)),
      () => {},
    );
  }
}

function withPrompt(
  prompts: ReadonlyMap<string, PromptView>,
  envelope: EventEnvelope,
): ReadonlyMap<string, PromptView> {
  const event = envelope.event;
  if (event.kind !== "prompt_queued" || prompts.get(event.turn_id)?.text === event.text) {
    return prompts;
  }
  const next = new Map(prompts);
  next.set(event.turn_id, { turnId: event.turn_id, text: event.text, origin: event.origin });
  return next;
}

/** The raw event item without its `kind` member: the envelope as efrd sent it. */
function withoutKind(raw: unknown): unknown {
  if (!isJsonObject(raw)) {
    return raw;
  }
  const { kind: _kind, ...envelope } = raw;
  return envelope;
}

function cachedEvents(raws: readonly unknown[], seqs: readonly number[]): CachedEvent[] {
  return raws.flatMap((raw, index) => {
    const seq = seqs[index];
    return seq === undefined ? [] : [{ seq, json: JSON.stringify(raw) }];
  });
}

/** A cached envelope: decoded, and as JSON. */
function parseEnvelope(
  json: string,
):
  | { readonly envelope: EventEnvelope; readonly raw: Readonly<Record<string, unknown>> }
  | undefined {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return undefined;
  }
  const decoded = decodeEventEnvelope(value);
  return decoded.ok && isJsonObject(value) ? { envelope: decoded.value, raw: value } : undefined;
}
