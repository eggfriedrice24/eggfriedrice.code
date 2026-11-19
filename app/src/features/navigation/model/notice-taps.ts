// The taps on a push notice of efrd, on their way to the router (spec 7.2). A tap opens
// the conversation of the notice as a link to it does: `+native-intent.ts` gives each
// path to the gate of the links, so it waits for the Face ID unlock.
//
// - The data of a notice names the daemon (`d`) and the conversation (`c`). The app
//   trusts nothing else of a notice, and drops a tap for a machine that it does not know.
// - The tap that started the app is in the source when the router subscribes, and the
//   listener of the source can give it again. Each tap goes once, by its id.
// - The router can subscribe before the services attach the source; the taps then start
//   at the attach.
//
// Pure TypeScript.

import type { ConversationRef, MachineId } from "@/features/machines/model/machine-ref";
import { noticeDataSchema } from "./route-schemas";
import { conversationPath } from "./routes";

/**
 * One tap on a notice: the id of that one notice (the native adapter adds its date,
 * because the notices of one conversation share their id) and its data.
 */
export interface NoticeTap {
  readonly id: string;
  readonly data: unknown;
}

/** Where the taps come from: expo-notifications, or a fake in the tests. */
export interface NoticeSource {
  /** The tap that started the app, or undefined. */
  launch(): NoticeTap | undefined;
  /** Calls `listener` with each later tap. Returns its stop. */
  follow(listener: (tap: NoticeTap) => void): () => void;
  /** Forgets the tap that started the app, so a reload does not open it again. */
  clear(): void;
}

/** The taps of the app. */
export interface NoticeTaps {
  /** Calls `listener` with the path of the conversation of each tap. Returns its stop. */
  subscribe(listener: (path: string) => void): () => void;
  /**
   * Gives the taps their source, and the test of a machine that the phone knows. The
   * services call it one time.
   */
  attach(source: NoticeSource, knows: (machine: MachineId) => Promise<boolean>): void;
}

/** Makes the taps; they start at `attach`. */
export function createNoticeTaps(): NoticeTaps {
  const listeners = new Set<(path: string) => void>();
  let wired:
    | { readonly source: NoticeSource; readonly knows: (machine: MachineId) => Promise<boolean> }
    | undefined;
  const handled = new Set<string>();
  let stop: (() => void) | undefined;

  const open = (tap: NoticeTap): void => {
    if (wired === undefined || handled.has(tap.id)) {
      return;
    }
    handled.add(tap.id);
    wired.source.clear();
    const parsed = noticeDataSchema.safeParse(tap.data);
    if (!parsed.success) {
      return;
    }
    void route(wired.knows, { machine: parsed.data.d, id: parsed.data.c }).catch(() => undefined);
  };

  const route = async (
    knows: (machine: MachineId) => Promise<boolean>,
    ref: ConversationRef,
  ): Promise<void> => {
    if (!(await knows(ref.machine))) {
      return;
    }
    const path = conversationPath(ref);
    for (const listener of listeners) {
      listener(path);
    }
  };

  const start = (): void => {
    if (wired === undefined || stop !== undefined || listeners.size === 0) {
      return;
    }
    stop = wired.source.follow(open);
    const launch = wired.source.launch();
    if (launch !== undefined) {
      open(launch);
    }
  };

  return {
    subscribe(listener: (path: string) => void): () => void {
      listeners.add(listener);
      start();
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0) {
          stop?.();
          stop = undefined;
        }
      };
    },
    attach(source: NoticeSource, knows: (machine: MachineId) => Promise<boolean>): void {
      wired = { source, knows };
      start();
    },
  };
}

/** The taps of the app. `+native-intent.ts` and the root layout share them. */
export const noticeTaps: NoticeTaps = createNoticeTaps();
