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
// The tokens are the turn's input plus output tokens, in the terminal's format. A turn
// that searched the web ends its line with the count of its searches (`· 5 searches`,
// `, stored copy` for the provider's stored copy of the web).

import type { Origin, StopReason } from "@/lib/protocol/generated";
import type { TurnEnd, TurnWebView } from "@/lib/stream/reducer";
import { between, clockTime } from "./durations";
import { tokens } from "./format";
import { webPage } from "./tool-block";

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

/**
 * The count of the searches of a turn, last on its end line as in the terminal (TM-Web
 * 13): `5 searches`, `1 search`, with `, stored copy` when they read the provider's
 * stored copy of the web; `undefined` without a search.
 */
export function searchCount(web: TurnWebView | undefined): string | undefined {
  if (web === undefined || web.searches <= 0) {
    return undefined;
  }
  const count = web.searches === 1 ? "1 search" : `${String(web.searches)} searches`;
  return web.storedCopy ? `${count}, stored copy` : count;
}

/** One source of an answer, as its row shows it. */
export interface SourceRow {
  /** `[1]`: the mark of the answer that cites it. */
  readonly mark: string;
  /** The page without `https://` and `www.`, such as `protondb.com/app/2384710`. */
  readonly page: string;
  readonly title: string | undefined;
  /** The whole address, which a tap opens after the question. */
  readonly url: string;
}

/**
 * The sources that the answer of a turn cites, in the order of their first citation
 * (TM-Web 10 to 12): `[1] areweanticheatyet.com/game/rob-run  ROB RUN · Are We Anti-Cheat
 * Yet?`. A title is one line.
 */
export function sourceRows(web: TurnWebView | undefined): SourceRow[] {
  return (web?.sources ?? []).map((source, index) => {
    const title = source.title?.replace(/\s+/g, " ").trim();
    return {
      mark: `[${String(index + 1)}]`,
      page: webPage(source.url),
      title: title === undefined || title === "" ? undefined : title,
      url: source.url.trim(),
    };
  });
}

/** The quiet line under the end of the first turn of a conversation with a stored copy. */
export const STORED_COPY_NOTE = "a stored copy of the web can be some days old";

/**
 * The line of `end`. `startedAt` is the start of the turn; `machine` names the computer;
 * `web` is the web search of the turn.
 */
export function turnEndView(
  end: TurnEnd,
  startedAt: number | undefined,
  interruptedBy: Origin | undefined,
  machine: string,
  web?: TurnWebView,
): TurnEndView {
  const parts: string[] = [];
  if (end.at !== undefined) {
    parts.push(clockTime(end.at));
  }
  const took = between(startedAt, end.at);
  if (took !== undefined) {
    parts.push(took);
  }
  const searches = searchCount(web);
  switch (end.outcome) {
    case "completed": {
      if (end.usage !== undefined) {
        parts.push(`${tokens(end.usage.input_tokens + end.usage.output_tokens)} tokens`);
      }
      const stop = end.stop === undefined ? undefined : STOPS[end.stop];
      if (stop !== undefined) {
        parts.push(stop);
      }
      if (searches !== undefined) {
        parts.push(searches);
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
      if (searches !== undefined) {
        parts.push(searches);
      }
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
      if (searches !== undefined) {
        parts.push(searches);
      }
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

/**
 * `view` of a failed turn with `reason` in place of its own reason, in the line and in
 * the VoiceOver label. The end of a turn whose account is out reads its reason from
 * `account_out`, so VoiceOver never reads the message of the provider.
 */
export function withReason(view: TurnEndView, reason: string): TurnEndView {
  const old = view.reason ?? "";
  const tail = `, ${old}`;
  const head =
    old !== "" && view.label.endsWith(tail) ? view.label.slice(0, -tail.length) : view.label;
  return { ...view, reason, label: reason === "" ? head : `${head}, ${reason}` };
}

function quiet(glyph: TurnEndView["glyph"], detail: string, label: string): TurnEndView {
  return { glyph, lead: undefined, detail, reason: undefined, canRetry: false, label };
}

/** The text that "copy error" copies: the reason, or the line when there is none. */
export function errorText(view: TurnEndView): string {
  return view.reason ?? [view.lead, view.detail].filter(Boolean).join(" ");
}
