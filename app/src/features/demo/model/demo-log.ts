// The log of one demo conversation: the steps of its script that the demo server sent,
// as the wire items of `conversation.subscribe`. Each event gets the next `seq` and the
// time it went out; a draft names the last `seq` before it. The log also knows what a
// summary of the conversation says: its status, its turns and its last time. Pure.

import type {
  ConversationStatus,
  ConversationSummary,
  DraftPart,
  Event,
} from "@/lib/protocol/generated";
import type { DemoStep } from "./demo-script";

/** An event as efrd sends it inside a snapshot. */
export interface WireEnvelope {
  readonly seq: number;
  readonly conversation_id: string;
  readonly at: string;
  readonly event: Event;
}

/** An item of `conversation.subscribe`, as efrd sends it. */
export type WireItem =
  | ({ readonly kind: "event" } & WireEnvelope)
  | {
      readonly kind: "draft";
      readonly turn_id: string;
      readonly after_seq: number;
      readonly draft: DraftPart;
    }
  | {
      readonly kind: "snapshot";
      readonly conversation: ConversationSummary;
      readonly events: readonly WireEnvelope[];
      readonly hwm: number;
    };

/** What a conversation is about, for its summary. */
export interface DemoConversationFacts {
  readonly id: string;
  readonly title: string;
  readonly cwd: string;
  /** When it started, in ms since the epoch. */
  readonly createdAt: number;
}

/** The sent part of one conversation. */
export class DemoLog {
  readonly facts: DemoConversationFacts;
  private readonly sent: WireEnvelope[] = [];
  private drafts: WireItem[] = [];
  private seq = 0;
  private running: string | undefined;
  private readonly asking = new Set<string>();
  private turnCount = 0;
  private updated: number;

  constructor(facts: DemoConversationFacts) {
    this.facts = facts;
    this.updated = facts.createdAt;
  }

  /** The largest `seq` sent. */
  get lastSeq(): number {
    return this.seq;
  }

  /** The turn that runs now, if any. */
  get runningTurn(): string | undefined {
    return this.running;
  }

  /** The questions that wait for an answer, by call id. */
  get openQuestions(): ReadonlySet<string> {
    return this.asking;
  }

  /** The turns that started. */
  get turns(): number {
    return this.turnCount;
  }

  /** Sends one event or draft step at `now` and gives its wire item. */
  send(step: Exclude<DemoStep, { kind: "hold" }>, now: number): WireItem {
    this.updated = now;
    if (step.kind === "draft") {
      const draft: WireItem = {
        kind: "draft",
        turn_id: step.turnId,
        after_seq: this.seq,
        draft: step.draft,
      };
      if (step.turnId === this.running) {
        this.drafts.push(draft);
      }
      return draft;
    }
    this.seq += 1;
    const envelope: WireEnvelope = {
      seq: this.seq,
      conversation_id: this.facts.id,
      at: new Date(now).toISOString(),
      event: step.event,
    };
    this.sent.push(envelope);
    this.follow(step.event);
    return { kind: "event", ...envelope };
  }

  /** The snapshot of a new subscription: every event so far. */
  snapshot(): WireItem {
    return {
      kind: "snapshot",
      conversation: this.summary(),
      events: [...this.sent],
      hwm: this.seq,
    };
  }

  /** The events after `afterSeq`, as items, for a subscription that resumes. */
  eventsAfter(afterSeq: number): WireItem[] {
    return this.sent
      .filter((envelope) => envelope.seq > afterSeq)
      .map((envelope): WireItem => ({
        kind: "event",
        seq: envelope.seq,
        conversation_id: envelope.conversation_id,
        at: envelope.at,
        event: envelope.event,
      }));
  }

  /**
   * The drafts of the running turn so far. A new or resumed subscription gets them after
   * its events, so it has the text of the turn from its first byte.
   */
  liveDrafts(): readonly WireItem[] {
    return this.drafts;
  }

  /** The status of the conversation, as a summary gives it. */
  status(): ConversationStatus {
    if (this.asking.size > 0) {
      return "awaiting_approval";
    }
    return this.running === undefined ? "idle" : "running";
  }

  /** The summary of the conversation now. */
  summary(): ConversationSummary {
    return {
      id: this.facts.id,
      title: this.facts.title,
      status: this.status(),
      cwd: this.facts.cwd,
      created_at: new Date(this.facts.createdAt).toISOString(),
      updated_at: new Date(this.updated).toISOString(),
      last_seq: this.seq,
    };
  }

  private follow(event: Event): void {
    switch (event.kind) {
      case "turn_started":
        this.running = event.turn_id;
        this.turnCount += 1;
        this.drafts = [];
        break;
      case "turn_completed":
      case "turn_failed":
      case "turn_interrupted":
      case "turn_cancelled":
        if (this.running === event.turn_id) {
          this.running = undefined;
          this.drafts = [];
        }
        break;
      case "approval_requested":
        this.asking.add(event.call_id);
        break;
      case "approval_resolved":
      case "approval_expired":
        this.asking.delete(event.call_id);
        break;
      default:
        break;
    }
  }
}

/**
 * The items of `steps` up to the first hold, sent at the times of the script from
 * `start`: a conversation as a subscription that joins later sees it. The boards of the
 * gallery reduce these items.
 */
export function sendAll(
  facts: DemoConversationFacts,
  steps: readonly DemoStep[],
  start: number,
): { readonly log: DemoLog; readonly items: readonly WireItem[] } {
  const log = new DemoLog(facts);
  const items: WireItem[] = [];
  let now = start;
  for (const step of steps) {
    if (step.kind === "hold") {
      break;
    }
    now += step.wait;
    items.push(log.send(step, now));
  }
  return { log, items };
}
