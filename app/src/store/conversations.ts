// The conversation list: pages of `conversations.list`, the most recently updated first,
// and the summaries that the snapshots of open conversations bring.

import { decodeConversationSummary } from "@/lib/protocol/envelope";
import type { ConversationSummary } from "@/lib/protocol/generated";
import type { Caller } from "./conversation";

/** The state of the list. */
export interface ConversationListState {
  /** The conversations, the most recently updated first. */
  readonly conversations: readonly ConversationSummary[];
  /** The cursor of the next page; absent at the end. */
  readonly nextCursor: string | undefined;
  readonly loading: boolean;
  /** Why the last load failed; absent after a load that worked. */
  readonly error: Error | undefined;
}

/** The size of a page. */
export const PAGE_SIZE = 50;

/** The conversation list. Read it like a conversation store. */
export class ConversationListStore {
  private readonly listeners = new Set<() => void>();
  private view: ConversationListState = {
    conversations: [],
    nextCursor: undefined,
    loading: false,
    error: undefined,
  };

  /** The state now. It keeps its identity until it changes. */
  readonly getState = (): ConversationListState => this.view;

  /** Calls `listener` after each change. Returns the function that stops it. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** Loads the first page again. The summaries that it holds replace the list. */
  async refresh(caller: Caller): Promise<void> {
    await this.load(caller, undefined);
  }

  /** Loads the next page, if there is one. */
  async loadMore(caller: Caller): Promise<void> {
    const cursor = this.view.nextCursor;
    if (cursor !== undefined && !this.view.loading) {
      await this.load(caller, cursor);
    }
  }

  /** Puts in or updates one summary, such as from a snapshot. */
  upsert(summary: ConversationSummary): void {
    const others = this.view.conversations.filter((held) => held.id !== summary.id);
    this.commit({ ...this.view, conversations: sorted([...others, summary]) });
  }

  /** Takes a conversation out of the list. */
  remove(conversationId: string): void {
    const conversations = this.view.conversations.filter((held) => held.id !== conversationId);
    if (conversations.length !== this.view.conversations.length) {
      this.commit({ ...this.view, conversations });
    }
  }

  private async load(caller: Caller, cursor: string | undefined): Promise<void> {
    this.commit({ ...this.view, loading: true });
    try {
      const result = await caller.call(
        "conversations.list",
        cursor === undefined ? { limit: PAGE_SIZE } : { limit: PAGE_SIZE, cursor },
      );
      const page = (Array.isArray(result.conversations) ? result.conversations : []).flatMap(
        (value: unknown) => {
          const decoded = decodeConversationSummary(value);
          return decoded.ok ? [decoded.value] : [];
        },
      );
      const ids = new Set(page.map((summary) => summary.id));
      const kept =
        cursor === undefined ? [] : this.view.conversations.filter((held) => !ids.has(held.id));
      this.commit({
        conversations: sorted([...kept, ...page]),
        nextCursor: result.next_cursor ?? undefined,
        loading: false,
        error: undefined,
      });
    } catch (error) {
      this.commit({
        ...this.view,
        loading: false,
        error: error instanceof Error ? error : new Error("the list could not load"),
      });
    }
  }

  private commit(view: ConversationListState): void {
    this.view = view;
    for (const listener of this.listeners) {
      listener();
    }
  }
}

/** The most recently updated first; equal times keep the order of the ids. */
function sorted(conversations: readonly ConversationSummary[]): ConversationSummary[] {
  return [...conversations].sort((a, b) => time(b) - time(a) || a.id.localeCompare(b.id));
}

function time(summary: ConversationSummary): number {
  const at = Date.parse(summary.updated_at);
  return Number.isNaN(at) ? 0 : at;
}
