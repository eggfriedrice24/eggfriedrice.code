// A stream state from a demo script at once, for the component sheets of the gallery:
// the items that a subscription would get after the script ran (the snapshot and the
// drafts of the running turn), through the decoder and the reducer of the app. Pure.

import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import { EMPTY_STREAM, reduce, type StreamState } from "@/lib/stream/reducer";
import { sendAll } from "./demo-log";
import { DEMO_CWD, type DemoStep } from "./demo-script";

/** The conversation id of the sheets. */
export const SHEET_CONVERSATION = "0199d3e0-de33-7000-8000-000000000001";

/** The stream state after `steps`, which end at `end` (ms since the epoch). */
export function staticState(steps: readonly DemoStep[], end: number, title = "demo"): StreamState {
  const duration = steps.reduce((sum, step) => sum + step.wait, 0);
  const { log } = sendAll(
    { id: SHEET_CONVERSATION, title, cwd: DEMO_CWD, createdAt: end - duration - 60_000 },
    steps,
    end - duration,
  );
  let state = EMPTY_STREAM;
  for (const item of [log.snapshot(), ...log.liveDrafts()]) {
    const decoded = decodeSubscribeItem(item);
    if (decoded.ok) {
      state = reduce(state, decoded.value);
    }
  }
  return state;
}

/** The running turn of `state`: the newest turn that started and did not end. */
export function runningTurnOf(state: StreamState): string | undefined {
  for (let i = state.order.length - 1; i >= 0; i -= 1) {
    const id = state.order[i];
    const turn = id === undefined ? undefined : state.turns.get(id);
    if (turn !== undefined && turn.started && turn.end === undefined) {
      return id;
    }
  }
  return undefined;
}
