// The drafts of the composer (plan section 2.2): the text that the user typed and did
// not send yet, per conversation of each machine, and one for the blank root of each
// machine. Only the composer reads them, with a selector of its own key, so typing
// renders the composer and never the screen (t3code PR #21261).
//
// - The store changes at once on each key press.
// - The disk write waits until the typing stops for 200 ms (a Debouncer of TanStack
//   Pacer), and then writes all drafts as one JSON value to the key-value store.
// - The store reads the saved drafts once, when it is made, so the composer shows the
//   draft in its first frame.
//
// Pure TypeScript on TanStack Store and Pacer.

import { Debouncer } from "@tanstack/pacer";
import { createStore, type Readable } from "@tanstack/store";

import type { ConversationId, DaemonId } from "@/lib/protocol/generated";
import type { KeyValueStorage } from "@/lib/store/key-value";
import { savedDraftsSchema } from "./drafts-schemas";

/** The key-value key of the drafts. */
export const DRAFTS_KEY = "efr.drafts";

/** How long the typing must stop before the drafts go to the disk. */
export const DRAFT_SAVE_MS = 200;

/** The conversation part of the draft key of the blank root of a machine. */
export const BLANK_ROOT = "new";

/** The key of a draft: a machine and a conversation, or BLANK_ROOT for the blank root. */
export type DraftKey = `${DaemonId}:${ConversationId}`;

/** The key of the draft of `conversation` (a conversation id or BLANK_ROOT) on `machine`. */
export function draftKey(machine: DaemonId, conversation: ConversationId): DraftKey {
  return `${machine}:${conversation}`;
}

/** The drafts: the text of each key. A key without text is not in the map. */
export interface DraftsState {
  readonly texts: ReadonlyMap<string, string>;
}

/** The text of the draft of `key`, for `useSelector`. */
export function selectDraft(key: DraftKey): (state: DraftsState) => string {
  return (state) => state.texts.get(key) ?? "";
}

/** What the drafts need from the app. */
export interface DraftsDeps {
  readonly storage: KeyValueStorage;
  /** The wait before a save; DRAFT_SAVE_MS in the app. */
  readonly saveMs?: number;
  /** Called when the read or a save fails; the drafts go on in memory. */
  readonly onStorageError?: (error: unknown) => void;
}

/** The drafts of all composers. */
export interface Drafts {
  readonly store: Readable<DraftsState>;
  /** The text of a draft now. */
  text(key: DraftKey): string;
  /** Sets the text of a draft; an empty text removes it. The save follows. */
  set(key: DraftKey, text: string): void;
  /** Removes a draft, as after a send; the save follows. */
  clear(key: DraftKey): void;
  /** Removes every draft of a forgotten machine and saves at once. */
  removeMachine(machine: DaemonId): void;
  /** Saves at once when a save waits, as when the app goes to the background. */
  flush(): void;
  /** Resolves when every save so far is done. */
  settled(): Promise<void>;
}

/** Reads the saved drafts, or none when there are none or they are broken. */
function load(deps: DraftsDeps): Map<string, string> {
  let json: string | null;
  try {
    json = deps.storage.getItemSync(DRAFTS_KEY);
  } catch (error) {
    deps.onStorageError?.(error);
    return new Map();
  }
  if (json === null) {
    return new Map();
  }
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return new Map();
  }
  const parsed = savedDraftsSchema.safeParse(value);
  if (!parsed.success) {
    return new Map();
  }
  const texts = new Map<string, string>();
  for (const [key, text] of Object.entries(parsed.data)) {
    if (text !== "") {
      texts.set(key, text);
    }
  }
  return texts;
}

/** Makes the drafts and reads the saved ones. */
export function createDrafts(deps: DraftsDeps): Drafts {
  const store = createStore<DraftsState>({ texts: load(deps) });
  let saves: Promise<void> = Promise.resolve();

  const save = (): void => {
    const texts = store.state.texts;
    const json = JSON.stringify(Object.fromEntries(texts));
    const write =
      texts.size === 0
        ? () => deps.storage.removeItem(DRAFTS_KEY)
        : () => deps.storage.setItem(DRAFTS_KEY, json);
    saves = saves.then(write).catch((error: unknown) => deps.onStorageError?.(error));
  };
  const debouncer = new Debouncer(save, { wait: deps.saveMs ?? DRAFT_SAVE_MS });

  const update = (change: (texts: Map<string, string>) => boolean): boolean => {
    const texts = new Map(store.state.texts);
    if (!change(texts)) {
      return false;
    }
    store.setState(() => ({ texts }));
    return true;
  };

  return {
    store,
    text(key: DraftKey): string {
      return store.state.texts.get(key) ?? "";
    },
    set(key: DraftKey, text: string): void {
      if ((store.state.texts.get(key) ?? "") === text) {
        return;
      }
      update((texts) => {
        if (text === "") {
          texts.delete(key);
        } else {
          texts.set(key, text);
        }
        return true;
      });
      debouncer.maybeExecute();
    },
    clear(key: DraftKey): void {
      if (update((texts) => texts.delete(key))) {
        debouncer.maybeExecute();
      }
    },
    removeMachine(machine: DaemonId): void {
      const prefix = `${machine}:`;
      const changed = update((texts) => {
        let removed = false;
        for (const key of texts.keys()) {
          if (key.startsWith(prefix)) {
            texts.delete(key);
            removed = true;
          }
        }
        return removed;
      });
      if (changed) {
        // The drafts of a forgotten machine must not stay on the disk.
        debouncer.cancel();
        save();
      }
    },
    flush(): void {
      debouncer.flush();
    },
    settled(): Promise<void> {
      return saves;
    },
  };
}
