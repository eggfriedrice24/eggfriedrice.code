// The event cache: the events of each conversation that the app saw, kept on the phone so
// that a cold start shows the transcript at once and then resubscribes with `after_seq`
// (R4c.4 step 10 of the research). The app keeps it in expo-sqlite
// (src/lib/store/sqlite-cache.ts); tests use the memory cache below.
//
// The app can pair with more than one machine, and two machines can have conversations
// with the same id. So the cache keeps the events of each machine (by its daemon id)
// apart: `EventCaches.forMachine` gives the cache of one machine, and `removeMachine`
// deletes all events of a machine when the user forgets it.
//
// Each event is the JSON of its envelope as efrd sent it, so a newer app can read the
// kinds that this build skips. The cache is not the truth: efrd's log is. A missing or
// broken cache only makes the app wait for efrd.

/** One cached event: its sequence number and the JSON of its envelope. */
export interface CachedEvent {
  readonly seq: number;
  readonly json: string;
}

/** The cache of the events of each conversation of one machine. */
export interface EventCache {
  /** The cached events of a conversation, oldest first. */
  load(conversationId: string): Promise<readonly CachedEvent[]>;
  /** Adds events; an event with a sequence number that the cache has replaces it. */
  append(conversationId: string, events: readonly CachedEvent[]): Promise<void>;
  /** Replaces all events of a conversation, as after a snapshot. */
  replace(conversationId: string, events: readonly CachedEvent[]): Promise<void>;
  /** Removes a conversation. */
  remove(conversationId: string): Promise<void>;
}

/** The event caches of all machines, in one store. */
export interface EventCaches {
  /** The cache of the machine with the daemon id `daemonId`. */
  forMachine(daemonId: string): EventCache;
  /** Removes all events of a machine, as when the user forgets it. */
  removeMachine(daemonId: string): Promise<void>;
}

/** The cache of one conversation of one machine, as the stream of the conversation uses it. */
export interface ConversationCache {
  /** The cached events, oldest first. */
  load(): Promise<readonly CachedEvent[]>;
  /** Adds events; an event with a sequence number that the cache has replaces it. */
  append(events: readonly CachedEvent[]): Promise<void>;
  /** Replaces all events, as after a snapshot. */
  replace(events: readonly CachedEvent[]): Promise<void>;
}

/** The cache of conversation `conversationId` in the cache of its machine. */
export function conversationCache(cache: EventCache, conversationId: string): ConversationCache {
  return {
    load: () => cache.load(conversationId),
    append: (events) => cache.append(conversationId, events),
    replace: (events) => cache.replace(conversationId, events),
  };
}

/** The most events that the cache keeps per conversation; older ones go first. */
export const MAX_CACHED_EVENTS = 2000;

/** An event cache of one machine in memory. */
export class MemoryEventCache implements EventCache {
  private readonly conversations = new Map<string, Map<number, string>>();
  private readonly limit: number;

  constructor(limit = MAX_CACHED_EVENTS) {
    this.limit = limit;
  }

  load(conversationId: string): Promise<readonly CachedEvent[]> {
    const events = this.conversations.get(conversationId);
    if (events === undefined) {
      return Promise.resolve([]);
    }
    const sorted = [...events.entries()].sort((a, b) => a[0] - b[0]);
    return Promise.resolve(sorted.map(([seq, json]) => ({ seq, json })));
  }

  append(conversationId: string, events: readonly CachedEvent[]): Promise<void> {
    let held = this.conversations.get(conversationId);
    if (held === undefined) {
      held = new Map();
      this.conversations.set(conversationId, held);
    }
    for (const event of events) {
      held.set(event.seq, event.json);
    }
    if (held.size > this.limit) {
      const keep = [...held.keys()].sort((a, b) => a - b).slice(-this.limit);
      const floor = keep[0] ?? 0;
      for (const seq of held.keys()) {
        if (seq < floor) {
          held.delete(seq);
        }
      }
    }
    return Promise.resolve();
  }

  replace(conversationId: string, events: readonly CachedEvent[]): Promise<void> {
    this.conversations.delete(conversationId);
    return this.append(conversationId, events);
  }

  remove(conversationId: string): Promise<void> {
    this.conversations.delete(conversationId);
    return Promise.resolve();
  }

  /** Removes every conversation. */
  clear(): Promise<void> {
    this.conversations.clear();
    return Promise.resolve();
  }
}

/** The event caches of all machines in memory. */
export class MemoryEventCaches implements EventCaches {
  private readonly machines = new Map<string, MemoryEventCache>();
  private readonly limit: number;

  constructor(limit = MAX_CACHED_EVENTS) {
    this.limit = limit;
  }

  forMachine(daemonId: string): EventCache {
    let cache = this.machines.get(daemonId);
    if (cache === undefined) {
      cache = new MemoryEventCache(this.limit);
      this.machines.set(daemonId, cache);
    }
    return cache;
  }

  removeMachine(daemonId: string): Promise<void> {
    const cache = this.machines.get(daemonId);
    this.machines.delete(daemonId);
    if (cache === undefined) {
      return Promise.resolve();
    }
    // A store can still hold the cache of the forgotten machine. It then sees the cache
    // empty, as with SQLite.
    return cache.clear();
  }
}
