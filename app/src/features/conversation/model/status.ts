// The words of a turn's state: the status line of the running turn and the line at the
// end of a turn, as efr's terminal says them (`writing apply_patch 2.3 KB`, `compacting
// context`, `done, ctx 43%`). Pure, so each state has a test.

import type { TurnView } from "@/lib/stream/reducer";
import type { TranscriptDetails, TurnDetails, TurnEnd } from "./details";
import { type ContextGauge, contextGauge, size } from "./format";

/** The status line of a running turn. */
export interface TurnStatus {
  readonly text: string;
  /** The context gauge, while the turn has a count. */
  readonly gauge?: ContextGauge;
}

/** What the running turn does now. */
export function turnStatus(
  stream: TurnView | undefined,
  turn: TurnDetails | undefined,
  details: TranscriptDetails,
): TurnStatus {
  const gauge = details.context === undefined ? undefined : contextGauge(details.context);
  const withGauge = (text: string): TurnStatus =>
    gauge === undefined ? { text } : { text, gauge };
  if (turn !== undefined && !turn.started) {
    return { text: "waiting in the queue" };
  }
  if (details.compacting) {
    return withGauge("compacting context");
  }
  for (const approval of turn?.approvals.values() ?? []) {
    if (approval.state === "pending") {
      return withGauge(approval.localOnly ? "waiting for you on your computer" : "waiting for you");
    }
  }
  const inputs = [...(stream?.toolInputs.values() ?? [])];
  const input = inputs[inputs.length - 1];
  if (input !== undefined) {
    return withGauge(`writing ${input.tool} ${size(input.bytes)}`);
  }
  for (const [callId, call] of stream?.toolCalls ?? []) {
    if (!call.done) {
      const tool = turn?.tools.get(callId)?.tool ?? call.tool;
      return withGauge(`running ${tool === "" ? "a tool" : tool}`);
    }
  }
  for (const message of stream?.messages.values() ?? []) {
    if (!message.final) {
      return withGauge("writing");
    }
  }
  if (stream !== undefined && stream.reasoning.text.length > 0) {
    return withGauge(stream.title === undefined ? "thinking" : `thinking: ${stream.title}`);
  }
  return withGauge("working");
}

/** The line at the end of a turn. */
export interface EndLine {
  readonly text: string;
  readonly tone: "muted" | "error";
  readonly gauge?: ContextGauge;
  /** `(89k of 206k tokens)`, after the gauge. */
  readonly detail?: string;
}

/** The line of `end`. */
export function endLine(end: TurnEnd): EndLine {
  const gauge = end.context === undefined ? undefined : contextGauge(end.context);
  const extra = gauge === undefined ? {} : { gauge, detail: `(${gauge.detail})` };
  if (end.outcome === "completed") {
    return {
      text: end.stop === undefined ? "done" : `done: the model stopped (${end.stop})`,
      tone: "muted",
      ...extra,
    };
  }
  if (end.outcome === "failed") {
    return {
      text: end.error === undefined ? "failed" : `failed: ${end.error}`,
      tone: "error",
      ...extra,
    };
  }
  if (end.outcome === "interrupted") {
    return { text: "stopped", tone: "muted", ...extra };
  }
  if (end.outcome === "cancelled") {
    return { text: "cancelled: efrd stopped before the turn ran to its end", tone: "error" };
  }
  return { text: "withdrawn: the prompt did not run", tone: "muted" };
}
