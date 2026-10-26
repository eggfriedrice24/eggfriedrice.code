// The outbox of prompts (plan section 2.2). A prompt that the user sends goes into the
// outbox first, and the drain of its machine (outbox-drain.ts) sends it with
// `prompt.send`. So the pending prompt shows at once, a dropped socket or a killed app
// loses no prompt, and the prompts of one conversation go to efrd in order.
//
// - `enqueue` changes the store at once and writes to the disk after it (t3code PR #4882:
//   a pending row that waits for the disk shows late).
// - The command id is made once per entry. `prompt.send` is idempotent by command id, so
//   a resend after a lost answer gives the first result again and never a second prompt.
// - A prompt for a new conversation has the target `{ kind: "new", localId }`. The first
//   entry of that target sends `new_conversation: true`. Its result names the
//   conversation, and the outbox keeps the alias from the local id to it. The entries
//   after it wait for the alias. Then the outbox gives them the conversation as their
//   target, also on the disk, and a later prompt for the local id goes there too.
// - An entry stays after its result (`sent`) until the stream shows its echo, the
//   `prompt_queued` event with its command id (`acknowledge`, or `settle` by cursor).
//
// Pure TypeScript on TanStack Store. Time and ids come from the caller.

import { createStore, type Readable } from "@tanstack/store";

import type {
  CommandId,
  ConversationId,
  DaemonId,
  PromptSend,
  PromptSendResult,
} from "@/lib/protocol/generated";

/** Where an entry goes: a known conversation, or a new one that its first send makes. */
export type OutboxTarget =
  | { readonly kind: "conversation"; readonly conversationId: ConversationId }
  | { readonly kind: "new"; readonly localId: string };

/**
 * Where an entry is:
 * - `pending`: waiting for the drain (or for `retryAt`, or for the alias of its target);
 * - `sending`: `prompt.send` is in flight;
 * - `sent`: efrd took it; the entry waits for its echo in the stream;
 * - `failed`: efrd refused it. The user can retry it or withdraw it.
 */
export type OutboxEntryState = "pending" | "sending" | "sent" | "failed";

/** Why the last attempt of an entry did not get a result. */
export interface OutboxError {
  /** The error code of efrd, or "connection" when the connection went down. */
  readonly code: string;
  /** One sentence for a person, from efrd or the client. */
  readonly message: string;
}

/** What efrd answered to the send of an entry. */
export interface OutboxSent {
  readonly conversationId: ConversationId;
  readonly turnId: string;
  /** The sequence number of the `prompt_queued` event of the entry. */
  readonly seq: number;
  /** True when the prompt waits behind a running turn. */
  readonly queued: boolean;
}

/** One prompt in the outbox. Absent values are null, so the JSON on the disk has them. */
export interface OutboxEntry {
  readonly commandId: CommandId;
  /** The daemon id of the machine. */
  readonly machine: DaemonId;
  readonly target: OutboxTarget;
  readonly text: string;
  /** Wall-clock time of `enqueue`, in milliseconds since the epoch. */
  readonly createdAt: number;
  /** The order of the entries: it only goes up, also across launches of the app. */
  readonly order: number;
  /** The failed attempts that count against `MAX_SEND_ATTEMPTS`. */
  readonly attempts: number;
  readonly state: OutboxEntryState;
  /** The drain does not send the entry before this time of its clock; 0 is at once. */
  readonly retryAt: number;
  readonly error: OutboxError | null;
  readonly sent: OutboxSent | null;
}

/** The state of the outbox. */
export interface OutboxSnapshot {
  /** Every entry, by `order`. */
  readonly entries: readonly OutboxEntry[];
  /** The conversation of each new target that efrd made, by `aliasKey`. */
  readonly aliases: ReadonlyMap<string, ConversationId>;
}

/** The outbox before anything was sent. */
export const EMPTY_OUTBOX: OutboxSnapshot = { entries: [], aliases: new Map() };

/**
 * The failed attempts after which a retryable error of efrd (`busy`, `internal`) fails
 * the entry. A lost connection does not count: the drain sends again on the next ready.
 */
export const MAX_SEND_ATTEMPTS = 5;

/** Where the outbox keeps its entries. */
export interface OutboxStorage {
  /** Every valid entry, by order. */
  load(): Promise<readonly OutboxEntry[]>;
  /** Adds an entry, or replaces the one with the same command id. */
  put(entry: OutboxEntry): Promise<void>;
  remove(commandId: CommandId): Promise<void>;
  /** Removes every entry of a machine. */
  removeMachine(machine: DaemonId): Promise<void>;
}

/** What the outbox needs from the app. */
export interface OutboxDeps {
  /** The disk. Without it, the outbox lives in memory only. */
  readonly storage?: OutboxStorage;
  /** Wall-clock time in milliseconds since the epoch, such as `Date.now`. */
  readonly now: () => number;
  /** A new command id or local id, such as a v4 UUID of expo-crypto. */
  readonly newId: () => string;
  /** Called when a write or the load fails; the outbox goes on in memory. */
  readonly onStorageError?: (error: unknown) => void;
}

/** What `enqueue` takes. */
export interface OutboxInput {
  readonly machine: DaemonId;
  readonly target: OutboxTarget;
  readonly text: string;
}

/** The outbox of all machines. */
export interface Outbox {
  /** The state, for `useSelector` and the drains. */
  readonly store: Readable<OutboxSnapshot>;
  /**
   * Reads the entries of the last launch. An entry that was in flight is pending again,
   * and every wait ends, so the drains send at once. Entries that `enqueue` added before
   * stay after the loaded ones.
   */
  hydrate(): Promise<void>;
  /** A new target for the first prompt of a new conversation. */
  newConversation(): OutboxTarget;
  /** Adds a prompt. The store has it when this returns; the disk write follows. */
  enqueue(input: OutboxInput): OutboxEntry;
  /** The drain starts to send a pending entry. Returns false when it is not pending. */
  begin(commandId: CommandId): boolean;
  /** efrd took the entry. */
  succeed(commandId: CommandId, result: PromptSendResult): void;
  /** The connection went down during the send: the entry is pending again. */
  release(commandId: CommandId, error: OutboxError): void;
  /** A retryable error: the entry is pending until `retryAt`, or fails at the limit. */
  retryLater(commandId: CommandId, error: OutboxError, retryAt: number): void;
  /** efrd refused the entry. */
  fail(commandId: CommandId, error: OutboxError): void;
  /**
   * The user sends a failed entry again. It gets a new command id, because efrd already
   * answered the old one with an error, and goes after the other entries.
   */
  retry(commandId: CommandId): OutboxEntry | undefined;
  /** Removes an entry that is not in flight, such as a failed one that the user drops. */
  discard(commandId: CommandId): void;
  /** The stream showed the echo of the entry with this command id: removes it. */
  acknowledge(commandId: CommandId): void;
  /** The stream of a conversation reached `cursor`: removes its sent entries up to it. */
  settle(machine: DaemonId, conversationId: ConversationId, cursor: number): void;
  /** Removes every entry and alias of a forgotten machine, also on the disk. */
  removeMachine(machine: DaemonId): Promise<void>;
  /** Resolves when every disk write so far is done. */
  flush(): Promise<void>;
}

/** The key of an alias: the machine and the local id of a new target. */
export function aliasKey(machine: DaemonId, localId: string): string {
  return `${machine}:${localId}`;
}

/**
 * The conversation that `id` names on `machine`: the conversation of the alias when `id`
 * is the local id of a new target that efrd made, else undefined.
 */
export function resolveAlias(
  snapshot: OutboxSnapshot,
  machine: DaemonId,
  id: string,
): ConversationId | undefined {
  return snapshot.aliases.get(aliasKey(machine, id));
}

/**
 * The lane of an entry: the entries of one lane go to efrd in order, one at a time. A
 * new target whose alias is known is in the lane of that conversation.
 */
export function laneOf(snapshot: OutboxSnapshot, entry: OutboxEntry): string {
  if (entry.target.kind === "conversation") {
    return `${entry.machine}:${entry.target.conversationId}`;
  }
  const alias = resolveAlias(snapshot, entry.machine, entry.target.localId);
  if (alias !== undefined) {
    return `${entry.machine}:${alias}`;
  }
  return `${entry.machine}:new:${entry.target.localId}`;
}

/**
 * The entries that the transcript of a conversation shows as pending prompts, by order.
 * `id` is a conversation id, or the local id of a new target before efrd made it.
 * Failed entries are in the list too, so the user sees them and can retry them.
 */
export function entriesFor(
  snapshot: OutboxSnapshot,
  machine: DaemonId,
  id: string,
): readonly OutboxEntry[] {
  const conversationId = resolveAlias(snapshot, machine, id) ?? id;
  const lane = `${machine}:${conversationId}`;
  const newLane = `${machine}:new:${id}`;
  return snapshot.entries.filter((entry) => {
    if (entry.machine !== machine) {
      return false;
    }
    const key = laneOf(snapshot, entry);
    return key === lane || key === newLane;
  });
}

/** What the drain of a machine does now. */
export interface DrainPlan {
  /** The entries to send now, at most one per lane. */
  readonly due: readonly OutboxEntry[];
  /** The earliest `retryAt` of a lane that waits, or undefined when none waits. */
  readonly wakeAt: number | undefined;
}

/**
 * The entries of `machine` to send at `now`. In each lane, sent and failed entries do
 * not block; the first other entry is the head. A head in flight holds its lane. A
 * pending head is due when its wait is over, else its lane waits. So the entries of one
 * lane reach efrd in order.
 */
export function drainPlan(snapshot: OutboxSnapshot, machine: DaemonId, now: number): DrainPlan {
  const heads = new Map<string, OutboxEntry>();
  for (const entry of snapshot.entries) {
    if (entry.machine !== machine || entry.state === "sent" || entry.state === "failed") {
      continue;
    }
    const lane = laneOf(snapshot, entry);
    if (!heads.has(lane)) {
      heads.set(lane, entry);
    }
  }
  const due: OutboxEntry[] = [];
  let wakeAt: number | undefined;
  for (const head of heads.values()) {
    if (head.state !== "pending") {
      continue;
    }
    if (head.retryAt <= now) {
      due.push(head);
    } else if (wakeAt === undefined || head.retryAt < wakeAt) {
      wakeAt = head.retryAt;
    }
  }
  return { due, wakeAt };
}

/**
 * The params of `prompt.send` for an entry: its conversation, the conversation of the
 * alias of its new target, or `new_conversation` when efrd did not make it yet.
 */
export function sendParams(snapshot: OutboxSnapshot, entry: OutboxEntry): PromptSend {
  if (entry.target.kind === "conversation") {
    return {
      command_id: entry.commandId,
      text: entry.text,
      conversation_id: entry.target.conversationId,
    };
  }
  const alias = resolveAlias(snapshot, entry.machine, entry.target.localId);
  if (alias !== undefined) {
    return { command_id: entry.commandId, text: entry.text, conversation_id: alias };
  }
  return { command_id: entry.commandId, text: entry.text, new_conversation: true };
}

/** The aliases that the sent entries of `entries` name. */
function aliasesOf(entries: readonly OutboxEntry[]): Map<string, ConversationId> {
  const aliases = new Map<string, ConversationId>();
  for (const entry of entries) {
    if (entry.target.kind === "new" && entry.sent !== null) {
      aliases.set(aliasKey(entry.machine, entry.target.localId), entry.sent.conversationId);
    }
  }
  return aliases;
}

function byOrder(a: OutboxEntry, b: OutboxEntry): number {
  return a.order - b.order;
}

/** Makes the outbox. */
export function createOutbox(deps: OutboxDeps): Outbox {
  const store = createStore<OutboxSnapshot>(EMPTY_OUTBOX);
  let lastOrder = 0;
  let writes: Promise<void> = Promise.resolve();

  // The disk writes run in order, after the store changed, and a failed write does not
  // stop the next one.
  const write = (run: (storage: OutboxStorage) => Promise<void>): void => {
    const storage = deps.storage;
    if (storage === undefined) {
      return;
    }
    writes = writes
      .then(() => run(storage))
      .catch((error: unknown) => deps.onStorageError?.(error));
  };

  const nextOrder = (): number => {
    // Wall-clock time keeps the order across launches; the step keeps it rising when
    // two entries come in one millisecond or the clock goes back.
    lastOrder = Math.max(lastOrder + 1, deps.now());
    return lastOrder;
  };

  const find = (commandId: CommandId): OutboxEntry | undefined =>
    store.state.entries.find((entry) => entry.commandId === commandId);

  // Replaces the entry with this command id. `persist` writes the new entry.
  const change = (
    commandId: CommandId,
    update: (entry: OutboxEntry) => OutboxEntry | undefined,
    persist: boolean,
  ): OutboxEntry | undefined => {
    const old = find(commandId);
    if (old === undefined) {
      return undefined;
    }
    const next = update(old);
    if (next === undefined || next === old) {
      return undefined;
    }
    const state = store.state;
    let aliases = state.aliases;
    const retargeted: OutboxEntry[] = [];
    let entries: OutboxEntry[];
    if (next.target.kind === "new" && next.sent !== null && old.sent === null) {
      // efrd made the conversation. The other entries of that new target now name it,
      // also on the disk, so a restart after the echo of this entry cannot send them
      // with `new_conversation` again.
      const { localId } = next.target;
      const conversationId = next.sent.conversationId;
      const added = new Map(aliases);
      added.set(aliasKey(next.machine, localId), conversationId);
      aliases = added;
      entries = [];
      for (const entry of state.entries) {
        if (entry.commandId === commandId) {
          entries.push(next);
        } else if (
          entry.machine === next.machine &&
          entry.target.kind === "new" &&
          entry.target.localId === localId
        ) {
          const moved: OutboxEntry = { ...entry, target: { kind: "conversation", conversationId } };
          retargeted.push(moved);
          entries.push(moved);
        } else {
          entries.push(entry);
        }
      }
    } else {
      entries = state.entries.map((entry) => (entry.commandId === commandId ? next : entry));
    }
    store.setState(() => ({ entries, aliases }));
    if (persist) {
      write((disk) => disk.put(next));
    }
    for (const entry of retargeted) {
      write((disk) => disk.put(entry));
    }
    return next;
  };

  const removeWhere = (drop: (entry: OutboxEntry) => boolean): void => {
    const gone = store.state.entries.filter(drop);
    if (gone.length === 0) {
      return;
    }
    store.setState((state) => ({
      entries: state.entries.filter((entry) => !drop(entry)),
      // The alias stays, so a screen that still shows the local id finds the conversation.
      aliases: state.aliases,
    }));
    for (const entry of gone) {
      write((disk) => disk.remove(entry.commandId));
    }
  };

  return {
    store,

    async hydrate(): Promise<void> {
      const storage = deps.storage;
      if (storage === undefined) {
        return;
      }
      let loaded: readonly OutboxEntry[];
      try {
        loaded = await storage.load();
      } catch (error) {
        deps.onStorageError?.(error);
        return;
      }
      const held = new Set(store.state.entries.map((entry) => entry.commandId));
      const restored: OutboxEntry[] = [];
      for (const entry of loaded) {
        if (held.has(entry.commandId)) {
          continue;
        }
        // An entry in flight when the app stopped may have reached efrd or not. It is
        // pending again: the same command id makes the resend safe. The clock of the
        // drain starts again with the app, so an old wait means nothing.
        const state = entry.state === "sending" ? "pending" : entry.state;
        restored.push({ ...entry, state, retryAt: 0 });
      }
      if (restored.length === 0) {
        return;
      }
      // The entries of this launch stay after the loaded ones, in their own order. When
      // the clock went back, they get new orders above the loaded ones.
      const top = restored.reduce((max, entry) => Math.max(max, entry.order), 0);
      const moved = store.state.entries.some((entry) => entry.order <= top);
      const fresh: OutboxEntry[] = [];
      for (const entry of store.state.entries) {
        fresh.push(moved ? { ...entry, order: top + 1 + fresh.length } : entry);
      }
      lastOrder = Math.max(lastOrder, top + fresh.length);
      const entries = [...restored, ...fresh].sort(byOrder);
      const aliases = new Map(store.state.aliases);
      for (const [key, value] of aliasesOf(restored)) {
        aliases.set(key, value);
      }
      store.setState(() => ({ entries, aliases }));
      if (moved) {
        for (const entry of fresh) {
          write((disk) => disk.put(entry));
        }
      }
    },

    newConversation(): OutboxTarget {
      return { kind: "new", localId: deps.newId() };
    },

    enqueue(input: OutboxInput): OutboxEntry {
      // A prompt for a new target that efrd already made goes to that conversation.
      let target = input.target;
      if (target.kind === "new") {
        const alias = resolveAlias(store.state, input.machine, target.localId);
        if (alias !== undefined) {
          target = { kind: "conversation", conversationId: alias };
        }
      }
      const entry: OutboxEntry = {
        commandId: deps.newId(),
        machine: input.machine,
        target,
        text: input.text,
        createdAt: deps.now(),
        order: nextOrder(),
        attempts: 0,
        state: "pending",
        retryAt: 0,
        error: null,
        sent: null,
      };
      store.setState((state) => ({ entries: [...state.entries, entry], aliases: state.aliases }));
      write((disk) => disk.put(entry));
      return entry;
    },

    begin(commandId: CommandId): boolean {
      // In flight is not written: after a restart the entry is pending again anyway.
      const next = change(
        commandId,
        (entry) => (entry.state === "pending" ? { ...entry, state: "sending" } : undefined),
        false,
      );
      return next !== undefined;
    },

    succeed(commandId: CommandId, result: PromptSendResult): void {
      change(
        commandId,
        (entry) =>
          entry.state === "sending" || entry.state === "pending"
            ? {
                ...entry,
                state: "sent",
                error: null,
                retryAt: 0,
                sent: {
                  conversationId: result.conversation_id,
                  turnId: result.turn_id,
                  seq: result.seq,
                  queued: result.queued,
                },
              }
            : undefined,
        true,
      );
    },

    release(commandId: CommandId, error: OutboxError): void {
      change(
        commandId,
        (entry) =>
          entry.state === "sending" ? { ...entry, state: "pending", retryAt: 0, error } : undefined,
        false,
      );
    },

    retryLater(commandId: CommandId, error: OutboxError, retryAt: number): void {
      change(
        commandId,
        (entry) => {
          if (entry.state !== "sending") {
            return undefined;
          }
          const attempts = entry.attempts + 1;
          if (attempts >= MAX_SEND_ATTEMPTS) {
            return { ...entry, state: "failed", attempts, retryAt: 0, error };
          }
          return { ...entry, state: "pending", attempts, retryAt, error };
        },
        true,
      );
    },

    fail(commandId: CommandId, error: OutboxError): void {
      change(
        commandId,
        (entry) =>
          entry.state === "sending" || entry.state === "pending"
            ? { ...entry, state: "failed", retryAt: 0, error }
            : undefined,
        true,
      );
    },

    retry(commandId: CommandId): OutboxEntry | undefined {
      const old = find(commandId);
      if (old === undefined || old.state !== "failed") {
        return undefined;
      }
      const entry: OutboxEntry = {
        ...old,
        commandId: deps.newId(),
        order: nextOrder(),
        attempts: 0,
        state: "pending",
        retryAt: 0,
        error: null,
      };
      store.setState((state) => ({
        entries: [...state.entries.filter((held) => held.commandId !== commandId), entry],
        aliases: state.aliases,
      }));
      write(async (disk) => {
        await disk.remove(commandId);
        await disk.put(entry);
      });
      return entry;
    },

    discard(commandId: CommandId): void {
      removeWhere((entry) => entry.commandId === commandId && entry.state !== "sending");
    },

    acknowledge(commandId: CommandId): void {
      removeWhere((entry) => entry.commandId === commandId);
    },

    settle(machine: DaemonId, conversationId: ConversationId, cursor: number): void {
      removeWhere(
        (entry) =>
          entry.machine === machine &&
          entry.sent !== null &&
          entry.sent.conversationId === conversationId &&
          entry.sent.seq <= cursor,
      );
    },

    async removeMachine(machine: DaemonId): Promise<void> {
      store.setState((state) => {
        const aliases = new Map(state.aliases);
        for (const key of state.aliases.keys()) {
          if (key.startsWith(`${machine}:`)) {
            aliases.delete(key);
          }
        }
        return { entries: state.entries.filter((entry) => entry.machine !== machine), aliases };
      });
      write((disk) => disk.removeMachine(machine));
      await writes;
    },

    flush(): Promise<void> {
      return writes;
    },
  };
}
