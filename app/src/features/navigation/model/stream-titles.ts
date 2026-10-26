// The titles of the conversations in the stack, from their streams. The header of a
// conversation shows its title, and the title is also the option `title` of its route,
// which the native back menu lists. The stream has the newest title (the summary of its
// last snapshot); a list of the machine can be older.
//
// A watch holds the stream ("keep": in memory, no subscription) while a header shows it,
// and gives it back when the header goes. One object per runtime counts the watches of
// each conversation. Pure.

import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { conversationKey } from "@/features/machines/model/machine-ref";
import type { StreamHold } from "@/features/machines/model/machine-supervisor";
import type { Runtime } from "@/features/machines/model/runtime";

/** The part of the runtime that the titles use. */
export type TitleSource = Pick<Runtime, "hold">;

interface Watch {
  readonly hold: StreamHold;
  count: number;
}

/** The titles of the streams of one runtime. */
export class StreamTitles {
  private readonly runtime: TitleSource;
  private readonly watches = new Map<string, Watch>();

  constructor(runtime: TitleSource) {
    this.runtime = runtime;
  }

  /**
   * Holds the stream of `ref` and calls `onChange` when its state changes. Returns the
   * function that stops it; the stream goes back when its last watch stops.
   */
  subscribe(ref: ConversationRef, onChange: () => void): () => void {
    const key = conversationKey(ref);
    let watch = this.watches.get(key);
    if (watch === undefined) {
      const hold = this.runtime.hold(ref, "keep");
      if (hold === undefined) {
        return () => undefined;
      }
      watch = { hold, count: 0 };
      this.watches.set(key, watch);
    }
    watch.count += 1;
    const held = watch;
    const subscription = held.hold.stream.store.subscribe(() => onChange());
    let stopped = false;
    return () => {
      if (stopped) {
        return;
      }
      stopped = true;
      subscription.unsubscribe();
      held.count -= 1;
      if (held.count === 0) {
        held.hold.release();
        this.watches.delete(key);
      }
    };
  }

  /** The title of the stream of `ref`, while a watch holds it and it has one. */
  title(ref: ConversationRef): string | undefined {
    const title = this.watches.get(conversationKey(ref))?.hold.stream.store.get().summary?.title;
    return title === null || title === "" ? undefined : title;
  }
}

const TITLES = new WeakMap<TitleSource, StreamTitles>();

/** The titles of the streams of `runtime`. One object per runtime. */
export function streamTitles(runtime: TitleSource): StreamTitles {
  let titles = TITLES.get(runtime);
  if (titles === undefined) {
    titles = new StreamTitles(runtime);
    TITLES.set(runtime, titles);
  }
  return titles;
}
