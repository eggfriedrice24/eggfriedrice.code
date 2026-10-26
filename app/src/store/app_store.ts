// The stores of the app, wired to the client: the conversation list, which loads again
// on each new connection, and one store per open conversation, which shows the cache and
// then follows the conversation while a screen holds it.

import type { Client } from "../net/client";
import { ConversationStore } from "./conversation";
import { ConversationListStore } from "./conversations";
import type { EventCache } from "./event_cache";

/** The settings of the app's stores. */
export interface AppStoreOptions {
  readonly client: Client;
  readonly cache?: EventCache;
  /** Called when the cache fails; the stores go on without it. */
  readonly onCacheError?: (error: unknown) => void;
  /** Called with each item that the store of a conversation applies, as JSON. */
  readonly onItem?: (conversationId: string, item: unknown) => void;
}

/** A conversation store that a screen holds; `release` lets it go. */
export interface HeldConversation {
  readonly store: ConversationStore;
  release(): void;
}

interface Open {
  readonly store: ConversationStore;
  holders: number;
  close: (() => void) | undefined;
  released: boolean;
}

/** The stores of the app. */
export class AppStore {
  readonly conversations = new ConversationListStore();
  private readonly options: AppStoreOptions;
  private readonly open = new Map<string, Open>();
  private readonly stopListening: () => void;

  constructor(options: AppStoreOptions) {
    this.options = options;
    this.stopListening = options.client.onState((state) => {
      if (state.state === "ready") {
        void this.conversations.refresh(options.client);
      }
    });
    if (options.client.state.state === "ready") {
      void this.conversations.refresh(options.client);
    }
  }

  /**
   * Holds the store of a conversation. The first holder shows the cached events and
   * then subscribes; the last `release` ends the subscription.
   */
  hold(conversationId: string): HeldConversation {
    let open = this.open.get(conversationId);
    if (open === undefined) {
      const store = new ConversationStore(conversationId, this.storeOptions(conversationId));
      const entry: Open = { store, holders: 0, close: undefined, released: false };
      open = entry;
      this.open.set(conversationId, entry);
      void this.follow(conversationId, entry);
    }
    const entry = open;
    entry.holders += 1;
    let done = false;
    return {
      store: entry.store,
      release: () => {
        if (done) {
          return;
        }
        done = true;
        entry.holders -= 1;
        if (entry.holders === 0) {
          entry.released = true;
          entry.close?.();
          this.open.delete(conversationId);
        }
      },
    };
  }

  /** Stops following the client and every open conversation. */
  dispose(): void {
    this.stopListening();
    for (const entry of this.open.values()) {
      entry.released = true;
      entry.close?.();
    }
    this.open.clear();
  }

  private async follow(conversationId: string, entry: Open): Promise<void> {
    await entry.store.hydrate();
    if (!entry.released) {
      const subscription = this.options.client.subscribe(conversationId, entry.store);
      entry.close = () => subscription.close();
    }
  }

  private storeOptions(conversationId: string): ConstructorParameters<typeof ConversationStore>[1] {
    const { cache, onCacheError, onItem } = this.options;
    return {
      ...(cache === undefined ? {} : { cache }),
      ...(onCacheError === undefined ? {} : { onCacheError }),
      ...(onItem === undefined ? {} : { onItem: (item: unknown) => onItem(conversationId, item) }),
      onSummary: (summary) => this.conversations.upsert(summary),
    };
  }
}
