// The words of a turn's state: the status line of the running turn and the line at the
// end of a turn, as efr's terminal says them (`writing apply_patch 2.3 KB`, `compacting
// context`, `done, ctx 43%`). Pure, so each state has a test.

import type { StreamState, TurnEnd, TurnView } from "@/lib/stream/reducer";
import { type ContextGauge, contextGauge, size } from "./format";

/** The status line of a running turn. */
export interface TurnStatus {
  readonly text: string;
  /** The context gauge, while the turn has a count. */
  readonly gauge: ContextGauge | undefined;
}

/** What `turn` does now, in the conversation `state`. */
export function turnStatus(turn: TurnView | undefined, state: StreamState): TurnStatus {
  const gauge = state.context === undefined ? undefined : contextGauge(state.context);
  const withGauge = (text: string): TurnStatus => ({ text, gauge });
  if (turn !== undefined && !turn.started) {
    return { text: "waiting in the queue", gauge: undefined };
  }
  if (state.compacting) {
    return withGauge("compacting context");
  }
  for (const approval of turn?.approvals.values() ?? []) {
    if (approval.status === "pending") {
      return withGauge(approval.localOnly ? "waiting for you on your computer" : "waiting for you");
    }
  }
  const inputs = [...(turn?.toolInputs.values() ?? [])];
  const input = inputs[inputs.length - 1];
  if (input !== undefined) {
    return withGauge(`writing ${input.tool} ${size(input.bytes)}`);
  }
  for (const call of turn?.tools.values() ?? []) {
    if (!call.done) {
      return withGauge(`running ${call.tool === "" ? "a tool" : call.tool}`);
    }
  }
  for (const message of turn?.messages.values() ?? []) {
    if (!message.final) {
      return withGauge("writing");
    }
  }
  if (turn !== undefined && turn.reasoning.text.length > 0) {
    return withGauge(turn.title === undefined ? "thinking" : `thinking: ${turn.title}`);
  }
  return withGauge("working");
}

/** The line at the end of a turn. */
export interface EndLine {
  readonly text: string;
  readonly tone: "muted" | "error";
  readonly gauge: ContextGauge | undefined;
  /** `(89k of 206k tokens)`, after the gauge. */
  readonly detail: string | undefined;
}

/** The line of `end`. */
export function endLine(end: TurnEnd): EndLine {
  const gauge = end.context === undefined ? undefined : contextGauge(end.context);
  const detail = gauge === undefined ? undefined : `(${gauge.detail})`;
  const line = (text: string, tone: EndLine["tone"]): EndLine => ({ text, tone, gauge, detail });
  switch (end.outcome) {
    case "completed":
      return line(
        end.stop === undefined ? "done" : `done: the model stopped (${end.stop})`,
        "muted",
      );
    case "failed":
      return line(end.error === undefined ? "failed" : `failed: ${end.error}`, "error");
    case "interrupted":
      return line("stopped", "muted");
    case "cancelled":
      return {
        text: "cancelled: efrd stopped before the turn ran to its end",
        tone: "error",
        gauge: undefined,
        detail: undefined,
      };
    case "withdrawn":
      return {
        text: "withdrawn: the prompt did not run",
        tone: "muted",
        gauge: undefined,
        detail: undefined,
      };
    default:
      return line("done", "muted");
  }
}
