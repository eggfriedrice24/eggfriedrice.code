// The scripts of the demo machine: what efrd sends for a conversation, step by step, with
// the wait before each step. The demo server (demo-server.ts) plays them on a clock, and
// the boards of the gallery reduce them at once into a stream state.
//
// The steps carry the generated protocol types, so a step that efrd could not send is a
// type error. The server gives each event its `seq` and its time when it sends it.
//
// The timing follows efrd: a text draft every 16 ms with the UTF-8 byte offset of its
// delta, an `assistant_message_updated` every 200 ms with the text since the last one,
// then `assistant_message_completed` with the whole text. Pure.

import type { DraftPart, Event, ExitInfo } from "@/lib/protocol/generated";
import { utf8Length } from "@/lib/stream/utf8";

/** The event of kind `K`. */
export type EventOf<K extends Event["kind"]> = Extract<Event, { kind: K }>;

/** One step of a script. */
export type DemoStep =
  | { readonly kind: "event"; readonly wait: number; readonly event: Event }
  | {
      readonly kind: "draft";
      readonly wait: number;
      readonly turnId: string;
      readonly draft: DraftPart;
    }
  /**
   * The script waits here until the phone answers the question `callId` (its
   * `approval_requested` came before). The server then sends `approval_resolved` and
   * plays the steps of the answer before the rest.
   */
  | {
      readonly kind: "hold";
      readonly wait: number;
      readonly turnId: string;
      readonly callId: string;
      readonly allow: readonly DemoStep[];
      readonly deny: readonly DemoStep[];
    };

/** A source of fresh ids for the turns and the calls of a script. */
export type NewId = () => string;

/** The working directory of the demo conversations. */
export const DEMO_CWD = "/home/me/code/efr";

/** The model of the demo turns. */
export const DEMO_MODEL = "gpt-6.1-sol";

/** The time of one draft, as efrd sends them. */
export const DRAFT_MS = 16;

/** The time between two updates of a message. */
export const UPDATE_MS = 200;

/** The size of the context window of the demo model. */
const WINDOW = { limit: 206_720, window: 272_000 };

/** The options of `DemoScript.say`. */
export interface SayOptions {
  /** About how many characters each draft carries. */
  readonly piece?: number;
  /** The wait before each draft, in ms. */
  readonly every?: number;
}

/** The fields of a question, besides its ids. */
export interface AskOptions {
  readonly summary: string;
  readonly reason?: string;
  readonly diffPreview?: string;
  readonly interactive?: boolean;
  readonly localOnly?: boolean;
  readonly expiresInSecs?: number;
  readonly exit?: ExitInfo;
}

/** Writes the steps of one conversation. */
export class DemoScript {
  private readonly out: DemoStep[] = [];
  private pending = 0;

  /** Waits `ms` before the next step. */
  wait(ms: number): this {
    this.pending += ms;
    return this;
  }

  /** One event. */
  event(event: Event): this {
    this.out.push({ kind: "event", wait: this.take(), event });
    return this;
  }

  /** One draft of turn `turnId`. */
  draft(turnId: string, draft: DraftPart): this {
    this.out.push({ kind: "draft", wait: this.take(), turnId, draft });
    return this;
  }

  /** The conversation starts, from a shell. */
  created(): this {
    return this.event({
      kind: "conversation_created",
      origin: "shell",
      route: { kind: "outside" },
    });
  }

  /** A prompt and the start of its turn. */
  prompt(
    turnId: string,
    text: string,
    commandId: string,
    origin: "shell" | "phone" = "shell",
  ): this {
    return this.queued(turnId, text, commandId, origin).start(turnId);
  }

  /** A prompt that waits in the queue. */
  queued(
    turnId: string,
    text: string,
    commandId: string,
    origin: "shell" | "phone" = "shell",
  ): this {
    return this.event({
      kind: "prompt_queued",
      turn_id: turnId,
      command_id: commandId,
      text,
      origin,
    });
  }

  /** The start of turn `turnId`. */
  start(turnId: string): this {
    return this.event({
      kind: "turn_started",
      turn_id: turnId,
      cwd: DEMO_CWD,
      scope: { kind: "machine" },
      settings: { mode: "cautious", model: DEMO_MODEL },
    });
  }

  /** Reasoning drafts of `text`, with an optional title. */
  reason(turnId: string, text: string, title?: string, options: SayOptions = {}): this {
    let offset = 0;
    for (const delta of pieces(text, options.piece ?? 18)) {
      this.wait(options.every ?? DRAFT_MS);
      const part: DraftPart =
        title === undefined
          ? { kind: "reasoning", offset, delta }
          : { kind: "reasoning", offset, delta, title };
      this.draft(turnId, part);
      offset += utf8Length(delta);
    }
    return this;
  }

  /**
   * Streams message `index` of turn `turnId`: drafts, an update every 200 ms of script
   * time, then the completed text.
   */
  say(turnId: string, index: number, text: string, options: SayOptions = {}): this {
    const every = options.every ?? DRAFT_MS;
    let offset = 0;
    let persisted = 0;
    let sent = "";
    let persistedText = "";
    let sinceUpdate = 0;
    for (const delta of pieces(text, options.piece ?? 12)) {
      this.wait(every);
      this.draft(turnId, { kind: "text", index, offset, delta });
      offset += utf8Length(delta);
      sent += delta;
      sinceUpdate += every;
      if (sinceUpdate >= UPDATE_MS) {
        sinceUpdate = 0;
        const update = sent.slice(persistedText.length);
        this.event({
          kind: "assistant_message_updated",
          turn_id: turnId,
          index,
          offset: persisted,
          delta: update,
        });
        persisted += utf8Length(update);
        persistedText = sent;
      }
    }
    this.wait(every);
    return this.event({ kind: "assistant_message_completed", turn_id: turnId, index, text });
  }

  /** Message `index` at once, as a history event. */
  said(turnId: string, index: number, text: string): this {
    return this.event({ kind: "assistant_message_completed", turn_id: turnId, index, text });
  }

  /** A tool call with its input drafts, its start, its output lines and its end. */
  tool(
    turnId: string,
    callId: string,
    tool: string,
    input: unknown,
    output: string,
    options: { readonly isError?: boolean; readonly lineMs?: number } = {},
  ): this {
    const inputBytes = utf8Length(JSON.stringify(input));
    for (let bytes = 64; bytes < inputBytes; bytes += 64) {
      this.wait(DRAFT_MS);
      this.draft(turnId, { kind: "tool_input", call: 0, tool, bytes });
    }
    this.wait(DRAFT_MS);
    this.event({ kind: "tool_call_started", turn_id: turnId, call_id: callId, tool, input });
    return this.output(turnId, callId, output, options);
  }

  /** The output lines and the end of a started tool call. */
  output(
    turnId: string,
    callId: string,
    output: string,
    options: { readonly isError?: boolean; readonly lineMs?: number } = {},
  ): this {
    let tail = "";
    for (const line of output.split("\n")) {
      this.wait(options.lineMs ?? UPDATE_MS);
      tail = `${tail}${line}\n`.slice(-4096);
      this.event({
        kind: "tool_call_output_updated",
        turn_id: turnId,
        call_id: callId,
        tail,
        bytes: utf8Length(tail),
      });
    }
    this.wait(DRAFT_MS);
    const isError = options.isError === true;
    return this.event({
      kind: "tool_call_completed",
      turn_id: turnId,
      call_id: callId,
      output,
      truncated: false,
      is_error: isError,
      exit_code: isError ? 1 : 0,
    });
  }

  /** A tool call at once, as history: its start and its end. */
  toolDone(
    turnId: string,
    callId: string,
    tool: string,
    input: unknown,
    output: string,
    extra: Partial<Pick<EventOf<"tool_call_completed">, "diff" | "is_error" | "exit_code">> = {},
  ): this {
    this.event({ kind: "tool_call_started", turn_id: turnId, call_id: callId, tool, input });
    const completed: EventOf<"tool_call_completed"> = {
      kind: "tool_call_completed",
      turn_id: turnId,
      call_id: callId,
      output,
      truncated: false,
      is_error: extra.is_error ?? false,
      exit_code: extra.exit_code ?? 0,
    };
    if (extra.diff !== undefined) {
      completed.diff = extra.diff;
    }
    return this.event(completed);
  }

  /** The question of call `callId`: the call starts and asks. */
  ask(turnId: string, callId: string, tool: string, input: unknown, options: AskOptions): this {
    this.event({ kind: "tool_call_started", turn_id: turnId, call_id: callId, tool, input });
    const asked: EventOf<"approval_requested"> = {
      kind: "approval_requested",
      turn_id: turnId,
      call_id: callId,
      summary: options.summary,
      interactive: options.interactive ?? false,
      local_only: options.localOnly ?? false,
    };
    if (options.reason !== undefined) {
      asked.reason = options.reason;
    }
    if (options.diffPreview !== undefined) {
      asked.diff_preview = options.diffPreview;
    }
    if (options.expiresInSecs !== undefined) {
      asked.expires_in_secs = options.expiresInSecs;
    }
    if (options.exit !== undefined) {
      asked.exit = options.exit;
    }
    return this.event(asked);
  }

  /** Waits for the answer to `callId`, then plays `allow` or `deny`. */
  hold(turnId: string, callId: string, allow: DemoScript, deny: DemoScript): this {
    this.out.push({
      kind: "hold",
      wait: this.take(),
      turnId,
      callId,
      allow: allow.steps(),
      deny: deny.steps(),
    });
    return this;
  }

  /** The answer to `callId`, from `origin`, as history. */
  answered(
    turnId: string,
    callId: string,
    decision: "allow" | "deny",
    origin: "shell" | "phone",
  ): this {
    return this.event({
      kind: "approval_resolved",
      turn_id: turnId,
      call_id: callId,
      decision,
      origin,
    });
  }

  /** The end of turn `turnId`, with how full the context is. */
  done(turnId: string, tokens = 89_400): this {
    this.draft(turnId, { kind: "context", tokens, ...WINDOW });
    this.wait(DRAFT_MS);
    return this.event({
      kind: "turn_completed",
      turn_id: turnId,
      context: { tokens, ...WINDOW },
    });
  }

  /** Turn `turnId` fails with the error of the model provider. */
  fail(turnId: string, message: string): this {
    return this.event({
      kind: "turn_failed",
      turn_id: turnId,
      error: { code: "internal", message },
      context: { tokens: 61_000, ...WINDOW },
    });
  }

  /** Turn `turnId` stops on a request of `origin`. */
  interrupt(turnId: string, origin: "shell" | "phone" = "shell"): this {
    this.event({ kind: "turn_interrupt_requested", turn_id: turnId, origin });
    return this.event({
      kind: "turn_interrupted",
      turn_id: turnId,
      context: { tokens: 58_000, ...WINDOW },
    });
  }

  /** The steps as written. */
  steps(): DemoStep[] {
    return [...this.out];
  }

  private take(): number {
    const wait = this.pending;
    this.pending = 0;
    return wait;
  }
}

/** `text` in pieces of about `size` code points, cut at code point boundaries. */
export function pieces(text: string, size: number): string[] {
  const points = Array.from(text);
  const out: string[] = [];
  for (let i = 0; i < points.length; i += size) {
    out.push(points.slice(i, i + size).join(""));
  }
  return out;
}
