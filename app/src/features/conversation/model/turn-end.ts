// The turn end line of the transcript (design 3.9 and the C-Failed board). It sits
// between two hairlines, in mono 11 text_subtle:
//
// - completed: `14:32 · 3m 08s · 18k tokens`;
// - interrupted: `■ interrupted by you · 14:31 · 1m 02s`, or `interrupted in a terminal
//   on desktop` when the stop came from the computer. The cut text stays as it is;
// - failed: `✗ turn failed · 14:40 · 22s`, then the reason in one mono line, then the
//   two plain buttons "retry" and "copy error". Retry sends the last prompt again as a
//   new prompt (decision D8). A turn that efrd cancelled reads the same way;
// - withdrawn: `withdrawn · the prompt did not run`.
//
// The tokens are the turn's input plus output tokens, in the terminal's format.

import type { Origin, StopReason } from "@/lib/protocol/generated";
import type { TurnEnd } from "@/lib/stream/reducer";
import { between, clockTime } from "./durations";
import { tokens } from "./format";

/** The parts of a turn end line. */
export interface TurnEndView {
  /** The glyph before the words: ■ for an interrupt, ✗ for a failure. */
  readonly glyph: "■" | "✗" | undefined;
  /** The words after the glyph, in a brighter colour for a failure: "turn failed". */
  readonly lead: string | undefined;
  /** The quiet rest: time, duration, tokens. */
  readonly detail: string;
  /** The reason of a failure, in a line of its own. */
  readonly reason: string | undefined;
  /** True when the line has "retry" and "copy error". */
  readonly canRetry: boolean;
  /** The VoiceOver label of the whole line. */
  readonly label: string;
}

const STOPS: Readonly<Record<StopReason, string | undefined>> = {
  end_turn: undefined,
  max_tokens: "the model hit its token limit",
  refusal: "the model refused",
  content_filter: "the provider filtered the answer",
  pause_turn: "the model paused",
};

const TERMINAL_ORIGINS: ReadonlySet<Origin> = new Set(["shell", "cli", "proxy"]);

/** The line of `end`. `startedAt` is the start of the turn; `machine` names the computer. */
export function turnEndView(
  end: TurnEnd,
  startedAt: number | undefined,
  interruptedBy: Origin | undefined,
  machine: string,
): TurnEndView {
  const parts: string[] = [];
  if (end.at !== undefined) {
    parts.push(clockTime(end.at));
  }
  const took = between(startedAt, end.at);
  if (took !== undefined) {
    parts.push(took);
  }
  switch (end.outcome) {
    case "completed": {
      if (end.usage !== undefined) {
        parts.push(`${tokens(end.usage.input_tokens + end.usage.output_tokens)} tokens`);
      }
      const stop = end.stop === undefined ? undefined : STOPS[end.stop];
      if (stop !== undefined) {
        parts.push(stop);
      }
      const detail = parts.join(" · ");
      return quiet(undefined, detail, `Turn done, ${detail}`);
    }
    case "interrupted": {
      const lead =
        interruptedBy === "phone"
          ? "interrupted by you"
          : interruptedBy !== undefined && TERMINAL_ORIGINS.has(interruptedBy)
            ? `interrupted in a terminal on ${machine}`
            : "interrupted";
      const detail = [lead, ...parts].join(" · ");
      return quiet("■", detail, `Turn ${detail}`);
    }
    case "withdrawn":
      return quiet(
        undefined,
        "withdrawn · the prompt did not run",
        "Prompt withdrawn, it did not run",
      );
    case "failed":
    case "cancelled": {
      const lead = end.outcome === "failed" ? "turn failed" : "turn cancelled";
      const reason =
        end.outcome === "failed" ? end.error : "efrd stopped before the turn ran to its end";
      const detail = parts.length === 0 ? "" : `· ${parts.join(" · ")}`;
      return {
        glyph: "✗",
        lead,
        detail,
        reason,
        canRetry: true,
        label: [`Turn ${lead.replace("turn ", "")}`, ...parts, reason].filter(Boolean).join(", "),
      };
    }
    default:
      return quiet(undefined, parts.join(" · "), "Turn ended");
  }
}

function quiet(glyph: TurnEndView["glyph"], detail: string, label: string): TurnEndView {
  return { glyph, lead: undefined, detail, reason: undefined, canRetry: false, label };
}

/** The text that "copy error" copies: the reason, or the line when there is none. */
export function errorText(view: TurnEndView): string {
  return view.reason ?? [view.lead, view.detail].filter(Boolean).join(" ");
}
